use super::*;

impl HostService {
    pub(super) async fn operator_install_proof(
        &self,
        payload: &Value,
    ) -> Result<DatabaseTransaction> {
        let proof = &payload["_authority"];
        let authority: StoredAuthority = parsed(proof["proof"].clone())?;
        self.session_authority
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .begin_operator(
                proof["subject"].as_str().ok_or_else(error)?,
                proof["sessionID"].as_str().ok_or_else(error)?,
                &authority,
            )
            .await
    }
    // Release the lookup connection before acquiring canonical authority: auth -> worker/fence.
    pub(super) async fn install_authorization(&self, id: &str) -> Result<DatabaseTransaction> {
        let tx = self.controller().await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.plugin_operation WHERE id=$1 AND payload->>'kind' IN ('install','installRetry','update','updateRetry','rollback','rollbackRetry')",[id.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let payload: Value = row.try_get("", "payload").map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        self.operator_install_proof(&payload).await
    }
    pub(super) async fn block_unclaimed_install(&self, id: &str) -> Result<()> {
        let tx = self.controller().await?;
        fence(&tx, true).await?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation o SET status='blocked',phase='blocked',claim_token=NULL,lease_until=NULL,checkpoint=jsonb_build_object('sequence',coalesce((o.checkpoint->>'sequence')::bigint,0)+1,'condition','authority_changed'),updated_at=now() FROM webdock_auth.plugin_install_execution e,webdock_auth.plugin_installation p WHERE o.id=$1 AND e.operation_id=o.id AND e.plugin_id=p.id AND p.generation=o.generation AND o.status IN ('pending','running') AND (o.lease_until IS NULL OR o.lease_until<=now())",[id.into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }
    pub(super) async fn locked_install(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
        generation: i64,
    ) -> Result<Value> {
        fence(tx, true).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(o) AS operation,to_jsonb(e) AS execution,to_jsonb(p) AS installation,to_jsonb(c) AS configuration,to_jsonb(r) AS runtime,to_jsonb(l) AS release FROM webdock_auth.plugin_operation o JOIN webdock_auth.plugin_install_execution e ON e.operation_id=o.id JOIN webdock_auth.plugin_installation p ON p.id=e.plugin_id JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=e.runtime_generation JOIN webdock_auth.plugin_release l ON l.plugin_id=p.id AND l.version=r.version WHERE o.id=$1 AND o.generation=$2 AND p.generation=o.generation AND p.kind='external' AND p.state='disabled' AND p.desired_state='enabled' AND p.candidate_generation=e.runtime_generation AND c.revision=e.configuration_revision AND r.configuration_revision=c.revision AND r.image=l.image AND r.version=l.version FOR UPDATE OF p,o,e FOR SHARE OF c,r,l",[id.into(),generation.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let mut value = serde_json::Map::new();
        for key in [
            "operation",
            "execution",
            "installation",
            "configuration",
            "runtime",
            "release",
        ] {
            value.insert(key.into(), row.try_get::<Value>("", key).map_err(db_error)?);
        }
        let mut value = Value::Object(value);
        if digest(
            value["release"]["manifest_bytes"]
                .as_str()
                .ok_or_else(error)?,
        ) != value["release"]["manifest_sha256"]
        {
            return Err(PluginError::denied());
        }
        let e = &value["execution"];
        let p = &value["installation"];
        let catalog = &self.installer()?.catalog;
        let entry = catalog
            .entries
            .get(&(
                p["id"].as_str().ok_or_else(error)?.into(),
                value["runtime"]["version"]
                    .as_str()
                    .ok_or_else(error)?
                    .into(),
            ))
            .ok_or_else(PluginError::denied)?;
        if entry.manifest_sha256 != value["release"]["manifest_sha256"]
            || value["release"]["image"] != entry.manifest.as_json()["image"]
            || value["release"]["registry_url"] != entry.registry_url
            || value["release"]["signer_key_id"] != entry.kid
            || e["platform_id"] != catalog.platform_id
            || e["supported_features"] != json!(entry.supported_features)
            || entry
                .targets
                .get(e["target_id"].as_str().ok_or_else(error)?)
                .is_none_or(|(_, review)| *review != e["review_sha256"])
        {
            return Err(PluginError::denied());
        }
        let (target, _) = entry
            .targets
            .get(e["target_id"].as_str().ok_or_else(error)?)
            .ok_or_else(PluginError::denied)?;
        if e["cluster_id"] != target.cluster_id {
            return Err(PluginError::denied());
        }
        self.verified_release(
            tx,
            p["id"].as_str().ok_or_else(error)?,
            value["runtime"]["version"].as_str().ok_or_else(error)?,
            entry,
        )
        .await?;
        if e["configuration_snapshot"]["configuration"] != value["configuration"]["configuration"]
            || e["configuration_snapshot"]["approvedCapabilities"]
                != value["configuration"]["approved_capabilities"]
        {
            return Err(PluginError::denied());
        }
        let high = match p["schema_high_water"].as_i64() {
            Some(h) => h,
            None => {
                let h = self
                    .initial_schema_baseline(tx, p["id"].as_str().ok_or_else(error)?)
                    .await?;
                tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_installation SET schema_high_water=$2 WHERE id=$1 AND schema_high_water IS NULL",[p["id"].as_str().ok_or_else(error)?.into(),h.into()])).await.map_err(db_error)?;
                h
            }
        };
        if !readable(entry.manifest.as_json(), high) {
            return Err(PluginError::denied());
        }
        value["installation"]["schema_high_water"] = json!(high);
        Ok(value)
    }
    pub(super) async fn worker_install_guard(
        &self,
        tx: &DatabaseTransaction,
        claim: &InstallClaim,
        auth: &DatabaseTransaction,
    ) -> Result<Value> {
        let value = self.locked_install(tx, &claim.id, claim.generation).await?;
        let o = &value["operation"];
        if o["plugin_id"] != claim.plugin
            || o["status"] != "running"
            || o["claim_token"] != claim.token.to_string()
            || ![
                "preparing_storage",
                "preparing_configuration",
                "verifying_runtime",
            ]
            .contains(&o["phase"].as_str().unwrap_or(""))
        {
            return Err(PluginError::denied());
        }
        let current = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.plugin_operation WHERE id=$1 AND lease_until>now()",
                [claim.id.clone().into()],
            ))
            .await
            .map_err(db_error)?;
        if current.is_none() {
            return Err(PluginError::denied());
        }
        // Read with the already-held canonical auth transaction, then let the existing signed
        // worker function lock/recheck cluster -> agent -> enrollment in one transaction.
        let e = &value["execution"];
        let agent=auth.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT generation FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND NOT revoked AND generation>0",[e["cluster_id"].as_str().ok_or_else(error)?.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let enrolled = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.lock_plugin_install_agent($1,$2,$3) AS allowed",
                [
                    e["cluster_id"].as_str().ok_or_else(error)?.into(),
                    agent
                        .try_get::<i32>("", "generation")
                        .map_err(db_error)?
                        .into(),
                    e["platform_id"].as_str().ok_or_else(error)?.into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        if !enrolled.try_get::<bool>("", "allowed").map_err(db_error)? {
            return Err(PluginError::denied());
        }
        if lifecycle(&value)
            && !Self::lifecycle_capability(
                tx,
                e["cluster_id"].as_str().ok_or_else(error)?,
                agent.try_get("", "generation").map_err(db_error)?,
            )
            .await?
        {
            return Err(PluginError::denied());
        }
        Ok(value)
    }
    pub async fn claim_installation(&self, plugin: Option<&str>) -> Result<Option<InstallClaim>> {
        if plugin.is_some_and(|p| !crate::slug(p)) {
            return Err(error());
        }
        self.installer()?;
        let tx = self.controller().await?;
        fence(&tx, true).await?;
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT o.id,o.plugin_id,o.generation,o.phase FROM webdock_auth.plugin_operation o JOIN webdock_auth.plugin_install_execution e ON e.operation_id=o.id WHERE o.payload->>'kind' IN ('install','installRetry','update','updateRetry','rollback','rollbackRetry') AND o.status IN ('pending','running') AND o.phase IN ('requested','preparing_storage','preparing_configuration','verifying_runtime') AND o.available_at<=now() AND (o.lease_until IS NULL OR o.lease_until<=now()) AND ($1::text IS NULL OR o.plugin_id=$1) ORDER BY o.available_at,o.id FOR UPDATE OF o SKIP LOCKED LIMIT 1",[plugin.into()])).await.map_err(db_error)?;
        let Some(row) = row else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        let phase: String = row.try_get("", "phase").map_err(db_error)?;
        let claim = InstallClaim {
            id: row.try_get("", "id").map_err(db_error)?,
            plugin: row.try_get("", "plugin_id").map_err(db_error)?,
            generation: row.try_get("", "generation").map_err(db_error)?,
            phase: if phase == "requested" {
                "preparing_storage".into()
            } else {
                phase
            },
            token: uuid::Uuid::new_v4(),
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET status='running',phase=$2,claim_token=$3::uuid,lease_until=now()+interval '120 seconds',attempts=attempts+1,checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1),updated_at=now() WHERE id=$1",[claim.id.clone().into(),claim.phase.clone().into(),claim.token.to_string().into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        if self.guard_installation(&claim).await? {
            Ok(Some(claim))
        } else {
            self.fail_installation(&claim, "authority_changed", false)
                .await?;
            Ok(None)
        }
    }
    pub async fn guard_installation(&self, claim: &InstallClaim) -> Result<bool> {
        let auth = match self.install_authorization(&claim.id).await {
            Ok(auth) => auth,
            Err(e) if e.code == webdock_plugin_protocol::ErrorCode::PermissionDenied => {
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        let tx = self.controller().await?;
        match self.worker_install_guard(&tx, claim, &auth).await {
            Ok(_) => {}
            Err(e) if e.code == webdock_plugin_protocol::ErrorCode::PermissionDenied => {
                return Ok(false);
            }
            Err(e) => return Err(e),
        };
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET lease_until=now()+interval '120 seconds' WHERE id=$1",[claim.id.clone().into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)?;
        Ok(true)
    }
    pub async fn install_work(&self, claim: &InstallClaim) -> Result<InstallWork> {
        let auth = self.install_authorization(&claim.id).await?;
        let tx = self.controller().await?;
        let v = self.worker_install_guard(&tx, claim, &auth).await?;
        let e = &v["execution"];
        let runtime_generation = e["runtime_generation"].as_u64().ok_or_else(error)?;
        let private = e["private_components"].as_object().ok_or_else(error)?;
        let open = |name: &str| {
            open_component(
                &self.installer()?.cipher,
                &claim.plugin,
                runtime_generation,
                name,
                private.get(name).ok_or_else(error)?,
            )
        };
        let recipe = parsed(Value::String(open("catalog.recipe")?))
            .and_then(|raw: String| serde_json::from_str(&raw).map_err(|_| error()))?;
        let target_policy = serde_json::from_str(&open("catalog.target")?).map_err(|_| error())?;
        let mut components = BTreeMap::new();
        for name in private.keys().filter(|name| name.starts_with("seed.")) {
            components.insert(name.trim_start_matches("seed.").to_owned(), open(name)?);
        }
        let work = InstallWork {
            kind: operation_kind(v["operation"]["payload"]["kind"].as_str()).into(),
            source_runtime_generation: e["source_runtime_generation"].as_u64(),
            plugin_id: claim.plugin.clone(),
            platform_id: e["platform_id"].as_str().ok_or_else(error)?.into(),
            runtime_generation,
            configuration_revision: e["configuration_revision"].as_u64().ok_or_else(error)?,
            configuration: v["configuration"]["configuration"].clone(),
            manifest_bytes: v["release"]["manifest_bytes"]
                .as_str()
                .ok_or_else(error)?
                .into(),
            manifest_sha256: v["release"]["manifest_sha256"]
                .as_str()
                .ok_or_else(error)?
                .into(),
            image: v["release"]["image"].as_str().ok_or_else(error)?.into(),
            target_id: e["target_id"].as_str().ok_or_else(error)?.into(),
            cluster_id: e["cluster_id"].as_str().ok_or_else(error)?.into(),
            recipe,
            target_policy,
            components,
        };
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)?;
        Ok(work)
    }
    pub async fn persist_install_components(
        &self,
        claim: &InstallClaim,
        components: BTreeMap<String, String>,
    ) -> Result<()> {
        if components.is_empty()
            || components.len() > 16
            || components.keys().any(|k| {
                k.is_empty()
                    || k.len() > 64
                    || !k
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            })
        {
            return Err(error());
        }
        let auth = self.install_authorization(&claim.id).await?;
        let tx = self.controller().await?;
        let v = self.worker_install_guard(&tx, claim, &auth).await?;
        if v["operation"]["phase"] == "verifying_runtime" {
            return Err(PluginError::denied());
        }
        let generation = v["execution"]["runtime_generation"]
            .as_u64()
            .ok_or_else(error)?;
        let mut private = v["execution"]["private_components"]
            .as_object()
            .ok_or_else(error)?
            .clone();
        for (name, value) in components {
            let name = format!("seed.{name}");
            if let Some(existing) = private.get(&name) {
                if open_component(
                    &self.installer()?.cipher,
                    &claim.plugin,
                    generation,
                    &name,
                    existing,
                )? != value
                {
                    return Err(conflict("Prepared installation material is immutable"));
                }
            } else {
                private.insert(
                    name.clone(),
                    seal_component(
                        &self.installer()?.cipher,
                        &claim.plugin,
                        generation,
                        &name,
                        &value,
                    )?,
                );
            }
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_install_execution SET private_components=$2,updated_at=now() WHERE plugin_id=$1 AND runtime_generation=$3",[claim.plugin.clone().into(),Value::Object(private).into(),(generation as i64).into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET phase='preparing_configuration',checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1),updated_at=now() WHERE id=$1",[claim.id.clone().into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)
    }
    pub async fn fail_installation(
        &self,
        claim: &InstallClaim,
        condition: &str,
        reconciliation: bool,
    ) -> Result<()> {
        if condition.is_empty()
            || condition.len() > 80
            || !condition
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(error());
        }
        let tx = self.controller().await?;
        fence(&tx, true).await?;
        let count=tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation o SET status=$4,phase=$4,claim_token=NULL,lease_until=NULL,checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1,'condition',$5::text),updated_at=now() FROM webdock_auth.plugin_installation p WHERE o.id=$1 AND o.generation=$2 AND p.id=o.plugin_id AND p.generation=o.generation AND o.claim_token=$3::uuid AND o.lease_until>now() AND o.status='running'",[claim.id.clone().into(),claim.generation.into(),claim.token.to_string().into(),if reconciliation{"blocked"}else{"failed"}.into(),condition.into()])).await.map_err(db_error)?.rows_affected();
        if count != 1 {
            return Err(PluginError::denied());
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_installation SET runtime=CASE WHEN current_generation IS NULL THEN 'failed' ELSE runtime END,blockers=jsonb_build_array(jsonb_build_object('code',$2::text)),revision=revision+1,updated_at=now() WHERE id=$1",[claim.plugin.clone().into(),condition.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_runtime SET status='failed' WHERE plugin_id=$1 AND generation=(SELECT candidate_generation FROM webdock_auth.plugin_installation WHERE id=$1)",[claim.plugin.clone().into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }
}

impl HostService {
    pub async fn promote_installation(
        &self,
        claim: &InstallClaim,
        proof: RuntimeDescription,
    ) -> Result<InstallSnapshot> {
        let auth = self.install_authorization(&claim.id).await?;
        let tx = self.controller().await?;
        let v = self.worker_install_guard(&tx, claim, &auth).await?;
        let v = self.dispatch_install_value(&tx, v).await?;
        let e = &v["execution"];
        let binding: RuntimeBinding = parsed(e["binding"].clone())?;
        let spec: PluginRuntimeSpec = parsed(e["spec"].clone())?;
        let checkpoint: PluginRuntimeCheckpoint = parsed(e["checkpoint"].clone())?;
        if v["operation"]["phase"] != "verifying_runtime"
            || !checkpoint.complete()
            || proof.manifest
                != v["release"]["manifest_bytes"]
                    .as_str()
                    .ok_or_else(error)?
                    .as_bytes()
            || proof.plugin_id != claim.plugin
            || proof.version != v["release"]["version"]
            || proof.generation != spec.runtime_generation
            || proof.protocol_version != 1
            || !binding.permits(&proof.peer)
            || spec.fingerprint().map_err(|_| error())? != e["spec_sha256"]
            || spec.material_sha256 != self.material(&v)?.fingerprint().map_err(|_| error())?
        {
            return Err(PluginError::denied());
        }
        let agent_generation = e["agent_generation"].as_i64().ok_or_else(error)?;
        let current = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.lock_plugin_install_agent($1,$2,$3) AS allowed",
                [
                    spec.cluster_id.clone().into(),
                    (agent_generation as i32).into(),
                    spec.platform_id.into(),
                ],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(error)?;
        if !current.try_get::<bool>("", "allowed").map_err(db_error)? {
            return Err(PluginError::denied());
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_runtime SET status='ready',observed_at=now() WHERE plugin_id=$1 AND generation=$2",[claim.plugin.clone().into(),(spec.runtime_generation as i64).into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_installation SET previous_generation=current_generation,version=$2,image_digest=$3,current_generation=candidate_generation,candidate_generation=NULL,state='enabled',desired_state='enabled',runtime='ready',blockers='[]'::jsonb,revision=revision+1,updated_at=now() WHERE id=$1",[claim.plugin.clone().into(),v["release"]["version"].as_str().ok_or_else(error)?.into(),v["release"]["image"].as_str().ok_or_else(error)?.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_install_execution SET promoted_at=now() WHERE plugin_id=$1 AND runtime_generation=$2",[claim.plugin.clone().into(),(spec.runtime_generation as i64).into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET status='succeeded',phase='complete',claim_token=NULL,lease_until=NULL,checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1),updated_at=now() WHERE id=$1",[claim.id.clone().into()])).await.map_err(db_error)?;
        let result = self.install_snapshot_in(&tx, &claim.id).await?;
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)?;
        Ok(result)
    }
}
