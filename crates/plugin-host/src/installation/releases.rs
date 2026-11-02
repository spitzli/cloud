use super::*;

impl HostService {
    pub(super) async fn lifecycle_capability(
        tx: &DatabaseTransaction,
        cluster: &str,
        generation: i32,
    ) -> Result<bool> {
        let row = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT webdock_rls.lock_plugin_lifecycle_agent($1,$2) AS allowed",
                [cluster.into(), generation.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(error)?;
        row.try_get("", "allowed").map_err(db_error)
    }
    pub async fn request_update_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        plugin: &str,
        input: UpdateRequest,
    ) -> Result<InstallSnapshot> {
        let payload = json!({"kind":"update","request":input});
        self.request_release(
            actor,
            authority,
            plugin,
            payload,
            Some(input.version),
            None,
            input.review_sha256,
            input.expected_revision,
            input.approved_capabilities,
            input.idempotency_key,
        )
        .await
    }
    pub async fn request_rollback_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        plugin: &str,
        input: RollbackRequest,
    ) -> Result<InstallSnapshot> {
        if !crate::data::revision(&input.release_generation) {
            return Err(error());
        }
        let generation = input
            .release_generation
            .parse::<i64>()
            .map_err(|_| error())?;
        if generation <= 0 {
            return Err(error());
        }
        let payload = json!({"kind":"rollback","request":input});
        self.request_release(
            actor,
            authority,
            plugin,
            payload,
            None,
            Some(generation),
            input.review_sha256,
            input.expected_revision,
            input.approved_capabilities,
            input.idempotency_key,
        )
        .await
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "The two strict public request types share one atomic admission transaction"
    )]
    async fn request_release(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        plugin: &str,
        mut payload: Value,
        version: Option<String>,
        restore: Option<i64>,
        review: String,
        revision: String,
        approved: Vec<String>,
        key: String,
    ) -> Result<InstallSnapshot> {
        if !crate::slug(plugin)
            || !idempotency(&key)
            || !crate::data::revision(&revision)
            || approved.len() > 32
        {
            return Err(error());
        }
        payload["pluginID"] = json!(plugin);
        let auth = self.admin_guard(actor, Some(authority), true).await?;
        let tx = self.operator_tx(actor).await?;
        if let Some(row)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,plugin_id,status,phase,generation,checkpoint,payload,payload->>'kind' AS kind FROM webdock_auth.plugin_operation WHERE actor_id=$1 AND idempotency_key=$2",[actor.user_id.clone().into(),key.clone().into()])).await.map_err(db_error)? {
            let mut prior:Value=row.try_get("","payload").map_err(db_error)?;prior.as_object_mut().ok_or_else(error)?.remove("_authority");
            if prior!=payload{return Err(conflict("Key already used with a different command"));}
            let result=snapshot(&row)?;self.admin_commit(tx,actor,Some(authority),auth,true).await?;return Ok(result);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(p) AS installation,to_jsonb(e) AS execution,to_jsonb(c) AS configuration FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_install_execution e ON e.plugin_id=p.id AND e.runtime_generation=p.current_generation JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id WHERE p.id=$1 AND p.kind='external' AND p.state='disabled' FOR UPDATE OF p,e FOR SHARE OF c",[plugin.into()])).await.map_err(db_error)?.ok_or_else(||conflict("Disable and drain the managed installation first"))?;
        let p: Value = row.try_get("", "installation").map_err(db_error)?;
        let source: Value = row.try_get("", "execution").map_err(db_error)?;
        let c: Value = row.try_get("", "configuration").map_err(db_error)?;
        if p["revision"].as_i64().map(|n| n.to_string()).as_deref() != Some(&revision) {
            return Err(conflict(
                "Installation changed; review its current revision",
            ));
        }
        let busy=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,status,payload FROM webdock_auth.plugin_operation WHERE plugin_id=$1 AND status IN ('pending','running','blocked') FOR UPDATE",[plugin.into()])).await.map_err(db_error)?;
        // Only explicit rollback may supersede a terminal blocked lifecycle. Its runtime still
        // belongs in the custody plan below, even when it never reached promotion.
        for o in &busy {
            let body: Value = o.try_get("", "payload").map_err(db_error)?;
            if restore.is_none()
                || o.try_get::<String>("", "status").map_err(db_error)? != "blocked"
                || !matches!(operation_kind(body["kind"].as_str()), "update" | "rollback")
            {
                return Err(conflict("Resolve the current lifecycle operation first"));
            }
        }
        let version = match (version, restore) {
            (Some(version), None) => version,
            (None, Some(g)) => {
                let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT r.version FROM webdock_auth.plugin_runtime r JOIN webdock_auth.plugin_install_execution e ON e.plugin_id=r.plugin_id AND e.runtime_generation=r.generation WHERE r.plugin_id=$1 AND r.generation=$2 AND e.promoted_at IS NOT NULL",[plugin.into(),g.into()])).await.map_err(db_error)?.ok_or_else(||conflict("Release was not successfully installed"))?;
                row.try_get("", "version").map_err(db_error)?
            }
            _ => return Err(error()),
        };
        let config = self.installer()?;
        let entry = config
            .catalog
            .entries
            .get(&(plugin.into(), version.clone()))
            .ok_or_else(|| conflict("Catalog release unavailable"))?;
        let target_id = source["target_id"].as_str().ok_or_else(error)?;
        let (target, expected_review) = entry
            .targets
            .get(target_id)
            .ok_or_else(|| conflict("Reviewed target unavailable"))?;
        if review != *expected_review {
            return Err(conflict("Catalog review changed"));
        }
        let source_gen = source["runtime_generation"].as_u64().ok_or_else(error)?;
        let policy = open_component(
            &config.cipher,
            plugin,
            source_gen,
            "catalog.target",
            &source["private_components"]["catalog.target"],
        )?;
        if source["platform_id"] != config.catalog.platform_id
            || source["cluster_id"] != target.cluster_id
            || serde_json::from_str::<Value>(&policy).map_err(|_| error())? != target.policy
        {
            return Err(conflict("Runtime placement cannot change"));
        }
        let caps: std::collections::BTreeSet<_> = approved.iter().cloned().collect();
        let declared: std::collections::BTreeSet<_> = entry.manifest.as_json()["capabilities"]
            .as_array()
            .ok_or_else(error)?
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_owned())
            .collect();
        let retained: std::collections::BTreeSet<_> = c["approved_capabilities"]
            .as_array()
            .ok_or_else(error)?
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_owned())
            .collect();
        if caps.len() != approved.len() || caps != declared || caps != retained {
            return Err(conflict(
                "Lifecycle requires the retained approved capabilities",
            ));
        }
        webdock_plugin_protocol::validate_value(
            Some(&entry.manifest.as_json()["configuration"]),
            &c["configuration"],
        )?;
        if !Self::installation_target_enrolled(
            &tx,
            auth.as_ref().ok_or_else(PluginError::denied)?,
            target,
            &config.catalog.platform_id,
        )
        .await?
        {
            return Err(PluginError::denied());
        }
        let agent = auth
            .as_ref()
            .ok_or_else(PluginError::denied)?
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT generation FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
                [target.cluster_id.clone().into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        if !Self::lifecycle_capability(
            &tx,
            &target.cluster_id,
            agent.try_get("", "generation").map_err(db_error)?,
        )
        .await?
        {
            return Err(conflict("Agent lifecycle capability required"));
        }
        let high = match p["schema_high_water"].as_i64() {
            Some(h) => h,
            None => self.initial_schema_baseline(&tx, plugin).await?,
        };
        // The candidate must consume the existing state before it can advance that state.
        if !readable(entry.manifest.as_json(), high) {
            return Err(conflict("Release cannot read retained schema"));
        }
        let high = high.max(
            entry.manifest.as_json()["schemaVersion"]
                .as_i64()
                .ok_or_else(error)?,
        );
        if !readable(entry.manifest.as_json(), high) {
            return Err(conflict("Release cannot read retained schema"));
        }
        let executions=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(e) AS execution FROM webdock_auth.plugin_install_execution e WHERE plugin_id=$1 ORDER BY runtime_generation FOR UPDATE",[plugin.into()])).await.map_err(db_error)?;
        let mut quiesce = Vec::new();
        let mut runtime_generation = 0;
        for row in executions {
            let e: Value = row.try_get("", "execution").map_err(db_error)?;
            let g = e["runtime_generation"].as_i64().ok_or_else(error)?;
            runtime_generation = runtime_generation.max(g);
            if e["apply_dispatched"] == true && e["stopped"] != true {
                let checkpoint: PluginRuntimeCheckpoint =
                    parsed(e["checkpoint"].clone()).map_err(|_| {
                        conflict("Runtime custody incomplete; retry retained operation")
                    })?;
                if !checkpoint.complete() || e["spec"].is_null() {
                    return Err(conflict(
                        "Runtime custody incomplete; retry retained operation",
                    ));
                }
                quiesce.push(g);
            }
        }
        runtime_generation = runtime_generation
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or_else(error)?;
        let generation = p["generation"]
            .as_i64()
            .ok_or_else(error)?
            .checked_add(1)
            .ok_or_else(error)?;
        // Only the retained masters cross generations. TLS, spec and material are new.
        let mut private = serde_json::Map::new();
        for name in ["seed.storagePassword", "seed.sealingKey"] {
            let value = open_component(
                &config.cipher,
                plugin,
                source_gen,
                name,
                &source["private_components"][name],
            )?;
            private.insert(
                name.into(),
                seal_component(
                    &config.cipher,
                    plugin,
                    runtime_generation as u64,
                    name,
                    &value,
                )?,
            );
        }
        for (name, value) in [
            ("catalog.target", target.policy.to_string()),
            ("catalog.recipe", entry.recipe.to_string()),
        ] {
            private.insert(
                name.into(),
                seal_component(
                    &config.cipher,
                    plugin,
                    runtime_generation as u64,
                    name,
                    &value,
                )?,
            );
        }
        self.retain_release(&tx, plugin, &version, entry).await?;
        let id = self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string();
        for row in busy {
            tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET status='failed',claim_token=NULL,lease_until=NULL WHERE id=$1",[row.try_get::<String>("","id").map_err(db_error)?.into()])).await.map_err(db_error)?;
        }
        payload["_authority"] =
            json!({"subject":actor.user_id,"sessionID":actor.session_id,"proof":authority});
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_operation(id,plugin_id,actor_id,idempotency_key,payload,generation) VALUES($1,$2,$3,$4,$5,$6)",[id.clone().into(),plugin.into(),actor.user_id.clone().into(),key.into(),payload.into(),generation.into()])).await.map_err(db_error)?;
        let image = entry.manifest.as_json()["image"]
            .as_str()
            .ok_or_else(error)?;
        let revision = c["revision"].as_i64().ok_or_else(error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,cluster_id,operation_id) VALUES($1,$2,$3,$4,'prepared',$5,$6,$7)",[plugin.into(),runtime_generation.into(),version.into(),image.into(),revision.into(),target.cluster_id.clone().into(),id.clone().into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_install_execution(plugin_id,runtime_generation,configuration_revision,operation_id,origin_operation_id,source_runtime_generation,platform_id,target_id,cluster_id,review_sha256,supported_features,private_components,configuration_snapshot,quiesce_generations) VALUES($1,$2,$3,$4,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",[plugin.into(),runtime_generation.into(),revision.into(),id.clone().into(),(source_gen as i64).into(),config.catalog.platform_id.clone().into(),target_id.into(),target.cluster_id.clone().into(),review.into(),json!(entry.supported_features).into(),Value::Object(private).into(),json!({"configuration":c["configuration"],"approvedCapabilities":c["approved_capabilities"]}).into(),json!(quiesce).into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_installation SET candidate_generation=$2,generation=$3,schema_high_water=$4,desired_state='enabled',revision=revision+1,updated_at=now() WHERE id=$1",[plugin.into(),runtime_generation.into(),generation.into(),high.into()])).await.map_err(db_error)?;
        self.audit_admin(&tx, actor, "plugin.release.requested", plugin)
            .await?;
        let result = self.install_snapshot_in(&tx, &id).await?;
        self.admin_commit(tx, actor, Some(authority), auth, true)
            .await?;
        Ok(result)
    }
    async fn retain_release(
        &self,
        tx: &DatabaseTransaction,
        plugin: &str,
        version: &str,
        entry: &crate::catalog::CatalogEntry,
    ) -> Result<()> {
        let raw = std::str::from_utf8(entry.manifest.as_bytes()).map_err(|_| error())?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING",[plugin.into(),version.into(),raw.into(),entry.manifest_sha256.clone().into(),entry.manifest.as_json()["image"].as_str().ok_or_else(error)?.into(),entry.registry_url.clone().into(),entry.kid.clone().into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_release_verification(plugin_id,version,jws,signer_sha256,format_version) VALUES($1,$2,$3,$4,1) ON CONFLICT DO NOTHING",[plugin.into(),version.into(),entry.jws.clone().into(),entry.signer_sha256.clone().into()])).await.map_err(db_error)?;
        self.verified_release(tx, plugin, version, entry).await
    }
    pub(super) async fn verified_release(
        &self,
        tx: &DatabaseTransaction,
        plugin: &str,
        version: &str,
        entry: &crate::catalog::CatalogEntry,
    ) -> Result<()> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT l.manifest_bytes,l.manifest_sha256,l.image,l.registry_url,l.signer_key_id,v.jws,v.signer_sha256 FROM webdock_auth.plugin_release l JOIN webdock_auth.plugin_release_verification v USING(plugin_id,version) WHERE l.plugin_id=$1 AND l.version=$2 FOR SHARE OF l",[plugin.into(),version.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        for (key, want) in [
            (
                "manifest_bytes",
                std::str::from_utf8(entry.manifest.as_bytes()).map_err(|_| error())?,
            ),
            ("manifest_sha256", &entry.manifest_sha256),
            (
                "image",
                entry.manifest.as_json()["image"]
                    .as_str()
                    .ok_or_else(error)?,
            ),
            ("registry_url", &entry.registry_url),
            ("signer_key_id", &entry.kid),
            ("jws", &entry.jws),
            ("signer_sha256", &entry.signer_sha256),
        ] {
            if row.try_get::<String>("", key).map_err(db_error)? != want {
                return Err(conflict("Retained release provenance changed"));
            }
        }
        Ok(())
    }
    pub(super) async fn initial_schema_baseline(
        &self,
        tx: &DatabaseTransaction,
        plugin: &str,
    ) -> Result<i64> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT r.version FROM webdock_auth.plugin_install_execution e JOIN webdock_auth.plugin_runtime r ON r.plugin_id=e.plugin_id AND r.generation=e.runtime_generation JOIN webdock_auth.plugin_operation o ON o.id=e.origin_operation_id WHERE e.plugin_id=$1 AND e.runtime_generation=1 AND e.source_runtime_generation IS NULL AND o.payload->>'kind' IN ('install','installRetry')",[plugin.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let version: String = row.try_get("", "version").map_err(db_error)?;
        let entry = self
            .installer()?
            .catalog
            .entries
            .get(&(plugin.into(), version.clone()))
            .ok_or_else(PluginError::denied)?;
        self.verified_release(tx, plugin, &version, entry).await?;
        entry.manifest.as_json()["schemaVersion"]
            .as_i64()
            .ok_or_else(error)
    }
}

impl HostService {
    pub(crate) async fn require_current_reader(
        &self,
        tx: &DatabaseTransaction,
        plugin: &str,
    ) -> Result<()> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.schema_high_water,l.manifest_bytes FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation JOIN webdock_auth.plugin_release l ON l.plugin_id=r.plugin_id AND l.version=r.version WHERE p.id=$1 AND EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution e WHERE e.plugin_id=p.id)",[plugin.into()])).await.map_err(db_error)?;
        let Some(row) = row else {
            return Ok(());
        };
        let high = match row
            .try_get::<Option<i64>>("", "schema_high_water")
            .map_err(db_error)?
        {
            Some(h) => h,
            None => self.initial_schema_baseline(tx, plugin).await?,
        };
        let manifest = webdock_plugin_protocol::Manifest::parse(
            row.try_get::<String>("", "manifest_bytes")
                .map_err(db_error)?
                .as_bytes(),
        )?;
        if !readable(manifest.as_json(), high) {
            return Err(conflict("Current release cannot read retained schema"));
        }
        Ok(())
    }
    pub async fn release_history_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        plugin: &str,
    ) -> Result<Value> {
        if !crate::slug(plugin) {
            return Err(error());
        }
        let auth = self.admin_guard(actor, Some(authority), false).await?;
        let tx = self.operator_tx(actor).await?;
        let p=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(p) AS value FROM webdock_auth.plugin_installation p WHERE id=$1 AND EXISTS(SELECT 1 FROM webdock_auth.plugin_install_execution e WHERE e.plugin_id=p.id)",[plugin.into()])).await.map_err(db_error)?.ok_or_else(PluginError::denied)?;
        let p: Value = p.try_get("", "value").map_err(db_error)?;
        let high = p["schema_high_water"]
            .as_i64()
            .or(self.initial_schema_baseline(&tx, plugin).await.ok());
        let rows=tx.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT r.generation,r.version,r.image,r.status,e.operation_id,e.target_id,e.promoted_at IS NOT NULL AS promoted,l.manifest_bytes,l.manifest_sha256 FROM webdock_auth.plugin_runtime r JOIN webdock_auth.plugin_install_execution e ON e.plugin_id=r.plugin_id AND e.runtime_generation=r.generation JOIN webdock_auth.plugin_release l ON l.plugin_id=r.plugin_id AND l.version=r.version WHERE r.plugin_id=$1 ORDER BY r.generation DESC",[plugin.into()])).await.map_err(db_error)?;
        let mut releases = Vec::new();
        let mut target = Value::Null;
        for row in rows {
            let version: String = row.try_get("", "version").map_err(db_error)?;
            let manifest = webdock_plugin_protocol::Manifest::parse(
                row.try_get::<String>("", "manifest_bytes")
                    .map_err(db_error)?
                    .as_bytes(),
            )?;
            let target_id: String = row.try_get("", "target_id").map_err(db_error)?;
            if target.is_null() {
                target = json!(target_id);
            }
            let entry = self
                .installer()?
                .catalog
                .entries
                .get(&(plugin.into(), version.clone()));
            let blocker = if !row.try_get::<bool>("", "promoted").map_err(db_error)? {
                Some("release_not_promoted")
            } else if entry.is_none() {
                Some("catalog_release_unavailable")
            } else if !high.is_some_and(|h| readable(manifest.as_json(), h)) {
                Some("schema_incompatible")
            } else if self
                .verified_release(&tx, plugin, &version, entry.ok_or_else(error)?)
                .await
                .is_err()
            {
                Some("release_provenance_changed")
            } else {
                None
            };
            releases.push(json!({"generation":row.try_get::<i64>("","generation").map_err(db_error)?.to_string(),"version":version,"imageDigest":row.try_get::<String>("","image").map_err(db_error)?,"manifestSha256":row.try_get::<String>("","manifest_sha256").map_err(db_error)?,"status":row.try_get::<String>("","status").map_err(db_error)?,"operationID":row.try_get::<String>("","operation_id").map_err(db_error)?,"schemaVersion":manifest.as_json()["schemaVersion"],"readableSchema":manifest.as_json()["readableSchema"],"rollbackEligible":blocker.is_none(),"rollbackBlocker":blocker}));
        }
        let pointer = |name: &str| p[name].as_i64().map(|v| v.to_string());
        let result = json!({"pluginID":plugin,"currentGeneration":pointer("current_generation"),"candidateGeneration":pointer("candidate_generation"),"previousGeneration":pointer("previous_generation"),"schemaHighWater":high,"targetID":target,"releases":releases});
        self.admin_commit(tx, actor, Some(authority), auth, false)
            .await?;
        Ok(result)
    }
}
