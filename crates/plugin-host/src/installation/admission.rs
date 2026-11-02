use super::*;

impl HostService {
    pub(super) async fn installation_target_enrolled(
        tx: &DatabaseTransaction,
        auth: &DatabaseTransaction,
        target: &crate::catalog::Target,
        platform: &str,
    ) -> Result<bool> {
        // Same lock order as offline enrollment and dispatch. Authentication is already held.
        let cluster=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.hosting_cluster WHERE id=$1 AND verified AND lower(region)='eu' AND ownership='platform' AND runtime_installation=$2 FOR SHARE",[target.cluster_id.clone().into(),platform.into()])).await.map_err(db_error)?;
        if cluster.is_none() {
            return Ok(false);
        }
        let agent=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT generation,credential_hash FROM webdock_auth.hosting_agent WHERE cluster_id=$1 AND NOT revoked AND generation>0 FOR SHARE",[target.cluster_id.clone().into()])).await.map_err(db_error)?;
        let Some(agent) = agent else {
            return Ok(false);
        };
        let enrollments=auth.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT agent_id FROM webdock_auth.agent_transport_enrollment WHERE cluster_id=$1 AND installation=$2 AND generation=$3 AND credential_hash=$4 AND NOT revoked ORDER BY agent_id FOR SHARE",[target.cluster_id.clone().into(),platform.into(),agent.try_get::<i32>("","generation").map_err(db_error)?.into(),agent.try_get::<String>("","credential_hash").map_err(db_error)?.into()])).await.map_err(db_error)?;
        Ok(!enrollments.is_empty())
    }
    pub async fn prepare_installation(
        &self,
        claim: &InstallClaim,
        prepared: PreparedInstallation,
    ) -> Result<()> {
        let spec: PluginRuntimeSpec = parsed(prepared.spec.clone())?;
        spec.validate().map_err(|_| error())?;
        let material = PluginRuntimeMaterial {
            files: prepared.files,
        };
        material.validate().map_err(|_| error())?;
        if spec.material_sha256 != material.fingerprint().map_err(|_| error())?
            || spec.manifest_sha256
                != digest(material.files.get("manifest.json").ok_or_else(error)?)
        {
            return Err(error());
        }
        let auth = self.install_authorization(&claim.id).await?;
        let tx = self.controller().await?;
        let v = self.worker_install_guard(&tx, claim, &auth).await?;
        let e = &v["execution"];
        let binding = &prepared.binding;
        if (lifecycle(&v) && spec.version != 2)
            || v["operation"]["phase"] == "verifying_runtime"
            || spec.plugin_id != claim.plugin
            || spec.platform_id != e["platform_id"]
            || spec.cluster_id != e["cluster_id"]
            || Some(spec.runtime_generation) != e["runtime_generation"].as_u64()
            || Some(spec.configuration_revision) != e["configuration_revision"].as_u64()
            || spec.image != v["release"]["image"]
            || spec.manifest_sha256 != v["release"]["manifest_sha256"]
            || material.files["manifest.json"].as_bytes()
                != v["release"]["manifest_bytes"]
                    .as_str()
                    .ok_or_else(error)?
                    .as_bytes()
            || binding.generation != spec.runtime_generation
            || binding.namespace != spec.namespace
            || binding.service != spec.service
            || binding.workload_cluster.as_deref() != Some(&spec.cluster_id)
            || binding.credential_sha256.len() != 64
            || !binding
                .credential_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || binding.peers.is_empty()
            || binding.peers.len() > 8
            || !binding.projects.is_empty()
            || !binding.customers.is_empty()
            || !binding.public_configuration.is_empty()
            || !binding.public_customer_configuration.is_empty()
        {
            return Err(error());
        }
        let spec_hash = spec.fingerprint().map_err(|_| error())?;
        let binding_json = json(binding)?;
        if !e["spec"].is_null() && (e["spec"] != prepared.spec || e["binding"] != binding_json) {
            return Err(conflict("Prepared runtime identity is immutable"));
        }
        let mut private = e["private_components"]
            .as_object()
            .ok_or_else(error)?
            .clone();
        for (file, text) in &material.files {
            if file == "manifest.json" {
                continue;
            }
            let key = format!("material.{file}");
            if let Some(old) = private.get(&key) {
                if open_component(
                    &self.installer()?.cipher,
                    &claim.plugin,
                    spec.runtime_generation,
                    &key,
                    old,
                )? != *text
                {
                    return Err(conflict("Prepared material is immutable"));
                }
            } else {
                private.insert(
                    key.clone(),
                    seal_component(
                        &self.installer()?.cipher,
                        &claim.plugin,
                        spec.runtime_generation,
                        &key,
                        text,
                    )?,
                );
            }
        }
        let endpoint = serde_json::to_string(&prepared.endpoint).map_err(|_| error())?;
        if !prepared.endpoint.is_object() || endpoint.len() > 65536 {
            return Err(error());
        }
        if let Some(old) = private.get("prepared.endpoint") {
            if open_component(
                &self.installer()?.cipher,
                &claim.plugin,
                spec.runtime_generation,
                "prepared.endpoint",
                old,
            )? != endpoint
            {
                return Err(conflict("Prepared endpoint is immutable"));
            }
        } else {
            private.insert(
                "prepared.endpoint".into(),
                seal_component(
                    &self.installer()?.cipher,
                    &claim.plugin,
                    spec.runtime_generation,
                    "prepared.endpoint",
                    &endpoint,
                )?,
            );
        }
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_install_execution SET spec=$2,spec_sha256=$3,binding=$4,private_components=$5,updated_at=now() WHERE plugin_id=$1 AND runtime_generation=$6",[claim.plugin.clone().into(),prepared.spec.into(),spec_hash.into(),binding_json.into(),Value::Object(private).into(),(spec.runtime_generation as i64).into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_runtime SET status='starting',credential_hash=$3,namespace=$4,service_name=$5 WHERE plugin_id=$1 AND generation=$2",[claim.plugin.clone().into(),(spec.runtime_generation as i64).into(),binding.credential_sha256.clone().into(),spec.namespace.into(),spec.service.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET status='pending',phase=$2,claim_token=NULL,lease_until=NULL,checkpoint=jsonb_build_object('sequence',coalesce((checkpoint->>'sequence')::bigint,0)+1),updated_at=now() WHERE id=$1",[claim.id.clone().into(),if e["quiesce_generations"].as_array().is_some_and(|q|!q.is_empty()){"suspending_runtime"}else{"deploying"}.into()])).await.map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        auth.commit().await.map_err(db_error)
    }
    pub async fn retry_install_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        operation_id: &str,
        idempotency_key: &str,
    ) -> Result<InstallSnapshot> {
        if !crate::data::revision(operation_id) || !idempotency(idempotency_key) {
            return Err(error());
        }
        let auth = self.admin_guard(actor, Some(authority), true).await?;
        let tx = self.operator_tx(actor).await?;
        let prior = tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT payload->>'kind' AS kind FROM webdock_auth.plugin_operation WHERE id=$1",
                [operation_id.into()],
            ))
            .await
            .map_err(db_error)?
            .ok_or_else(PluginError::denied)?;
        let kind = operation_kind(
            prior
                .try_get::<Option<String>>("", "kind")
                .map_err(db_error)?
                .as_deref(),
        );
        let payload = json!({"kind":format!("{kind}Retry"),"request":{"operationID":operation_id}});
        if let Some(row)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,plugin_id,status,phase,generation,checkpoint,payload,payload->>'kind' AS kind FROM webdock_auth.plugin_operation WHERE actor_id=$1 AND idempotency_key=$2",[actor.user_id.clone().into(),idempotency_key.into()])).await.map_err(db_error)?{
            let mut previous:Value=row.try_get("","payload").map_err(db_error)?;previous.as_object_mut().ok_or_else(error)?.remove("_authority");if previous!=payload{return Err(conflict("Key already used with a different command"));}let result=snapshot(&row)?;self.admin_commit(tx,actor,Some(authority),auth,true).await?;return Ok(result);
        }
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.id,p.generation,e.runtime_generation,e.quiesce_generations,e.quiesce_index,e.spec IS NOT NULL AS prepared FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_install_execution e ON e.plugin_id=p.id JOIN webdock_auth.plugin_operation o ON o.id=e.operation_id WHERE o.id=$1 AND o.status IN ('failed','blocked') AND p.state='disabled' AND p.candidate_generation=e.runtime_generation FOR UPDATE OF p,o,e",[operation_id.into()])).await.map_err(db_error)?.ok_or_else(||conflict("Installation cannot be retried"))?;
        let plugin: String = row.try_get("", "id").map_err(db_error)?;
        let generation: i64 = row.try_get::<i64>("", "generation").map_err(db_error)? + 1;
        let current = self
            .locked_install(&tx, operation_id, generation - 1)
            .await?;
        let catalog = &self.installer()?.catalog;
        let entry = catalog
            .entries
            .get(&(
                plugin.clone(),
                current["runtime"]["version"]
                    .as_str()
                    .ok_or_else(error)?
                    .into(),
            ))
            .ok_or_else(PluginError::denied)?;
        let (target, _) = entry
            .targets
            .get(
                current["execution"]["target_id"]
                    .as_str()
                    .ok_or_else(error)?,
            )
            .ok_or_else(PluginError::denied)?;
        if !Self::installation_target_enrolled(
            &tx,
            auth.as_ref().ok_or_else(PluginError::denied)?,
            target,
            &catalog.platform_id,
        )
        .await?
        {
            return Err(invalid(
                "Target requires a current matching agent enrollment",
            ));
        }
        let id = self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string();
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_operation SET status='failed',claim_token=NULL,lease_until=NULL WHERE id=$1",[operation_id.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_installation SET generation=$2,revision=revision+1,desired_state='enabled',runtime=CASE WHEN current_generation IS NULL THEN 'starting' ELSE runtime END,updated_at=now() WHERE id=$1",[plugin.clone().into(),generation.into()])).await.map_err(db_error)?;
        let phase = if row.try_get::<bool>("", "prepared").map_err(db_error)? {
            if row
                .try_get::<Value>("", "quiesce_generations")
                .map_err(db_error)?
                .as_array()
                .ok_or_else(error)?
                .len()
                > row.try_get::<i32>("", "quiesce_index").map_err(db_error)? as usize
            {
                "suspending_runtime"
            } else {
                "deploying"
            }
        } else {
            "requested"
        };
        let mut payload = payload;
        payload["_authority"] =
            json!({"subject":actor.user_id,"sessionID":actor.session_id,"proof":authority});
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_operation(id,plugin_id,actor_id,idempotency_key,payload,generation,phase) VALUES($1,$2,$3,$4,$5,$6,$7)",[id.clone().into(),plugin.clone().into(),actor.user_id.clone().into(),idempotency_key.into(),payload.into(),generation.into(),phase.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_install_execution SET operation_id=$2,agent_generation=NULL,agent_token_sha256=NULL,updated_at=now() WHERE plugin_id=$1 AND runtime_generation=$3",[plugin.clone().into(),id.clone().into(),row.try_get::<i64>("","runtime_generation").map_err(db_error)?.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.plugin_runtime SET operation_id=$2,status=CASE WHEN credential_hash IS NULL THEN 'prepared' ELSE 'starting' END WHERE plugin_id=$1 AND generation=$3",[plugin.clone().into(),id.clone().into(),row.try_get::<i64>("","runtime_generation").map_err(db_error)?.into()])).await.map_err(db_error)?;
        self.audit_admin(&tx, actor, "plugin.install.retry", &plugin)
            .await?;
        let result = self.install_snapshot_in(&tx, &id).await?;
        self.admin_commit(tx, actor, Some(authority), auth, true)
            .await?;
        Ok(result)
    }
}

impl HostService {
    pub fn with_installation(
        mut self,
        catalog: Arc<Catalog>,
        encryption_secret: &str,
    ) -> Result<Self> {
        self.installation = Some(InstallationConfig {
            catalog,
            cipher: SecretCipher::new(encryption_secret)
                .map_err(|_| invalid("Installation encryption required"))?,
        });
        Ok(self)
    }
    pub(super) fn installer(&self) -> Result<&InstallationConfig> {
        self.installation
            .as_ref()
            .ok_or_else(|| unsupported("Native installation is not configured"))
    }
    pub async fn catalog_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
    ) -> Result<Vec<CatalogDescriptor>> {
        let auth = self.admin_guard(actor, Some(authority), false).await?;
        let tx = self.operator_tx(actor).await?;
        let mut result = self
            .installation
            .as_ref()
            .map_or_else(Vec::new, |config| config.catalog.descriptors());
        if let Some(config) = &self.installation {
            for descriptor in &mut result {
                let entry = config
                    .catalog
                    .entries
                    .get(&(descriptor.catalog_id.clone(), descriptor.version.clone()))
                    .ok_or_else(error)?;
                let mut eligible = Vec::new();
                for view in std::mem::take(&mut descriptor.targets) {
                    let (target, _) = entry.targets.get(&view.id).ok_or_else(error)?;
                    if Self::installation_target_enrolled(
                        &tx,
                        auth.as_ref().ok_or_else(PluginError::denied)?,
                        target,
                        &config.catalog.platform_id,
                    )
                    .await?
                    {
                        eligible.push(view);
                    }
                }
                descriptor.targets = eligible;
            }
        }
        self.admin_commit(tx, actor, Some(authority), auth, false)
            .await?;
        Ok(result)
    }
    pub async fn request_install_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        input: InstallRequest,
    ) -> Result<InstallSnapshot> {
        if !idempotency(&input.idempotency_key)
            || !crate::slug(&input.catalog_id)
            || !crate::slug(&input.target_id)
            || input.approved_capabilities.len() > 32
        {
            return Err(error());
        }
        let payload = json!({"kind":"install","request":input});
        let auth = self.admin_guard(actor, Some(authority), true).await?;
        let tx = self.operator_tx(actor).await?;
        if let Some(row)=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,plugin_id,status,phase,generation,checkpoint,payload,payload->>'kind' AS kind FROM webdock_auth.plugin_operation WHERE actor_id=$1 AND idempotency_key=$2",[actor.user_id.clone().into(),input.idempotency_key.clone().into()])).await.map_err(db_error)?{
            let mut previous:Value=row.try_get("","payload").map_err(db_error)?;previous.as_object_mut().ok_or_else(error)?.remove("_authority");
            if previous!=payload{return Err(conflict("Key already used with a different command"));}
            let result=snapshot(&row)?;self.admin_commit(tx,actor,Some(authority),auth,true).await?;return Ok(result);
        }
        let config = self.installer()?;
        let entry = config
            .catalog
            .entries
            .get(&(input.catalog_id.clone(), input.version.clone()))
            .ok_or_else(|| invalid("Catalog release unavailable"))?;
        let (target, review) = entry
            .targets
            .get(&input.target_id)
            .ok_or_else(|| invalid("Catalog target unavailable"))?;
        if input.review_sha256 != *review {
            return Err(conflict("Catalog review changed"));
        }
        let approved: std::collections::BTreeSet<_> = input.approved_capabilities.iter().collect();
        if approved.len() != input.approved_capabilities.len()
            || approved.len()
                != entry.manifest.as_json()["capabilities"]
                    .as_array()
                    .unwrap()
                    .len()
            || !entry.manifest.as_json()["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| approved.contains(&c.as_str().unwrap().to_owned()))
        {
            return Err(invalid(
                "Approved capabilities must match the reviewed release",
            ));
        }
        webdock_plugin_protocol::validate_value(
            Some(&entry.manifest.as_json()["configuration"]),
            &input.configuration,
        )?;
        if self.bindings.runtime(&input.catalog_id).await?.is_some() {
            return Err(conflict("A file-enrolled runtime already owns this plugin"));
        }
        if tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT singleton FROM webdock_auth.plugin_bootstrap",
            ))
            .await
            .map_err(db_error)?
            .is_none()
        {
            return Err(unsupported("Initialize plugin state offline"));
        }
        if tx
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM webdock_auth.plugin_installation WHERE id=$1",
                [input.catalog_id.clone().into()],
            ))
            .await
            .map_err(db_error)?
            .is_some()
        {
            return Err(conflict("Plugin already has an installation"));
        }
        if !Self::installation_target_enrolled(
            &tx,
            auth.as_ref().ok_or_else(PluginError::denied)?,
            target,
            &config.catalog.platform_id,
        )
        .await?
        {
            return Err(invalid(
                "Target requires a current matching agent enrollment",
            ));
        }
        let id = self
            .ids
            .allocate()
            .await
            .map_err(|_| unavailable())?
            .to_string();
        let image = entry.manifest.as_json()["image"].as_str().unwrap();
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,image_digest,runtime,generation,candidate_generation,schema_high_water) VALUES($1,'external','disabled','enabled',$2,$3,'starting',1,1,$4)",[input.catalog_id.clone().into(),input.version.clone().into(),image.into(),entry.manifest.as_json()["schemaVersion"].as_i64().ok_or_else(error)?.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,$2,$3,$4,$5,$6,$7)",[input.catalog_id.clone().into(),input.version.clone().into(),std::str::from_utf8(entry.manifest.as_bytes()).map_err(|_|error())?.into(),entry.manifest_sha256.clone().into(),image.into(),entry.registry_url.clone().into(),entry.kid.clone().into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_release_verification(plugin_id,version,jws,signer_sha256,format_version) VALUES($1,$2,$3,$4,1)",[input.catalog_id.clone().into(),input.version.clone().into(),entry.jws.clone().into(),entry.signer_sha256.clone().into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,revision,approved_capabilities,configuration) VALUES($1,1,$2,$3)",[input.catalog_id.clone().into(),json!(input.approved_capabilities).into(),input.configuration.clone().into()])).await.map_err(db_error)?;
        let mut payload = payload;
        payload["_authority"] =
            json!({"subject":actor.user_id,"sessionID":actor.session_id,"proof":authority});
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_operation(id,plugin_id,actor_id,idempotency_key,payload,generation) VALUES($1,$2,$3,$4,$5,1)",[id.clone().into(),input.catalog_id.clone().into(),actor.user_id.clone().into(),input.idempotency_key.into(),payload.into()])).await.map_err(db_error)?;
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,cluster_id,operation_id) VALUES($1,1,$2,$3,'prepared',1,$4,$5)",[input.catalog_id.clone().into(),input.version.into(),image.into(),target.cluster_id.clone().into(),id.clone().into()])).await.map_err(db_error)?;
        let private = json!({"catalog.recipe":seal_component(&config.cipher,&input.catalog_id,1,"catalog.recipe",&entry.recipe.to_string())?,"catalog.target":seal_component(&config.cipher,&input.catalog_id,1,"catalog.target",&target.policy.to_string())?});
        tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_install_execution(plugin_id,runtime_generation,configuration_revision,operation_id,platform_id,target_id,cluster_id,review_sha256,supported_features,private_components,origin_operation_id,configuration_snapshot) VALUES($1,1,1,$2,$3,$4,$5,$6,$7,$8,$2,$9)",[input.catalog_id.clone().into(),id.clone().into(),config.catalog.platform_id.clone().into(),input.target_id.into(),target.cluster_id.clone().into(),review.clone().into(),json!(entry.supported_features).into(),private.into(),json!({"configuration":input.configuration,"approvedCapabilities":input.approved_capabilities}).into()])).await.map_err(db_error)?;
        self.audit_admin(&tx, actor, "plugin.install.requested", &input.catalog_id)
            .await?;
        let result = self.install_snapshot_in(&tx, &id).await?;
        self.admin_commit(tx, actor, Some(authority), auth, true)
            .await?;
        Ok(result)
    }
    pub(super) async fn install_snapshot_in(
        &self,
        tx: &DatabaseTransaction,
        id: &str,
    ) -> Result<InstallSnapshot> {
        let row=tx.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,plugin_id,status,phase,generation,checkpoint,payload->>'kind' AS kind FROM webdock_auth.plugin_operation WHERE id=$1 AND payload->>'kind' IN ('install','installRetry','update','updateRetry','rollback','rollbackRetry')",[id.into()])).await.map_err(db_error)?.ok_or_else(||PluginError::new(webdock_plugin_protocol::ErrorCode::NotFound,"Installation operation not found"))?;
        snapshot(&row)
    }
    pub async fn install_operation_with_authority(
        &self,
        actor: &Principal,
        authority: &StoredAuthority,
        id: &str,
    ) -> Result<InstallSnapshot> {
        if !crate::data::revision(id) {
            return Err(error());
        }
        let auth = self.admin_guard(actor, Some(authority), false).await?;
        let tx = self.operator_tx(actor).await?;
        let result = self.install_snapshot_in(&tx, id).await?;
        self.admin_commit(tx, actor, Some(authority), auth, false)
            .await?;
        Ok(result)
    }
}
