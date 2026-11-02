use super::executor::*;
use super::*;
use kube::api::{DeleteParams, Preconditions};
use serde_json::json;
use webdock_agent_protocol::database::{
    DatabaseCheckpoint, DatabaseCheckpointPhase, DatabaseClaim, DatabaseCompletionProof,
    DatabaseProofState,
};
impl NativeDatabaseExecutor {
    pub(super) async fn checkpoint(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
        phase: DatabaseCheckpointPhase,
        uid: &str,
    ) -> Result<()> {
        self.guard(claim, authority).await?;
        authority
            .checkpoint(
                claim,
                DatabaseCheckpoint {
                    lease: claim.lease.clone(),
                    phase,
                    cnpg_uid: uid.into(),
                    ca_certificate: None,
                    previous_pod_uid: None,
                    migration_job_uid: None,
                    copy_sha256: None,
                },
            )
            .await
    }
    pub(super) async fn patch_cluster(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
        patch: serde_json::Value,
    ) -> Result<()> {
        self.patch_owned(
            "Cluster",
            "postgres",
            Some(&claim.spec.backend.namespace),
            &patch,
            claim,
            authority,
        )
        .await
        .map(|_| ())
    }
    async fn endpoints(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<bool> {
        let b = &claim.spec.backend;
        for (suffix, target, port) in [
            ("direct", "postgres-rw", b.direct_port),
            ("pooled", "pooler", b.pooled_port),
        ] {
            let Some(service) = self.get("Service", target, Some(&b.namespace)).await? else {
                return Ok(false);
            };
            let controller = self
                .get(
                    if target == "pooler" {
                        "Pooler"
                    } else {
                        "Cluster"
                    },
                    if target == "pooler" {
                        "pooler"
                    } else {
                        "postgres"
                    },
                    Some(&b.namespace),
                )
                .await?
                .ok_or(ExecutionError::Ownership)?;
            self.owned(&controller, &claim.spec)?;
            if !service.metadata.owner_references.as_ref().is_some_and(|r| {
                r.iter()
                    .any(|r| Some(&r.uid) == controller.metadata.uid.as_ref())
            }) || service.data["spec"]["selector"]
                .as_object()
                .is_none_or(|s| s.is_empty())
            {
                return Err(ExecutionError::Ownership);
            }
            self.apply(json!({"apiVersion":"v1","kind":"Service","metadata":{"name":format!("public-{suffix}"),"namespace":b.namespace,"labels":{"webdock.dev/database-installation":b.id}},"spec":{"type":"NodePort","selector":service.data["spec"]["selector"],"ports":[{"name":"postgres","port":5432,"targetPort":5432,"nodePort":port,"protocol":"TCP"}]}}),claim,authority).await?;
        }
        Ok(true)
    }
    pub(super) async fn configure_stage(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        let job = &claim.spec;
        let b = &job.backend;
        let cluster = self
            .get("Cluster", "postgres", Some(&b.namespace))
            .await?
            .ok_or(ExecutionError::Ownership)?;
        self.owned(&cluster, job)?;
        let uid = cluster
            .metadata
            .uid
            .clone()
            .ok_or(ExecutionError::Ownership)?;
        if cluster.data["status"]["readyInstances"] != 1 && job.action != DatabaseAction::Restart {
            return Ok(DatabaseStep::Pending);
        }
        self.verify_storage(claim, authority).await?;
        self.checkpoint(claim, authority, DatabaseCheckpointPhase::Configuring, &uid)
            .await?;
        if job.action == DatabaseAction::Restore {
            self.configure_recovery(claim, authority).await?;
            self.configure_export(claim, authority).await?;
            self.recovered_export_password(claim, authority).await?;
            self.enable_export(claim, authority).await?;
            self.patch_cluster(
                claim,
                authority,
                json!({"spec":{"postgresql":{"pg_hba":self.config.hba(job)?}}}),
            )
            .await?;
        }
        match job.action {
            DatabaseAction::Create | DatabaseAction::RotateCredentials | DatabaseAction::Resume
                if job.database.is_some() =>
            {
                let statements = super::sql::statements(job)?;
                let database = job.database.as_ref().unwrap();
                self.sql(claim, authority, "postgres", &statements.roles)
                    .await?;
                if !self.database_exists(claim, authority).await? {
                    if job.action != DatabaseAction::Create {
                        return Err(ExecutionError::Ownership);
                    }
                    self.sql(claim, authority, "postgres", &statements.create)
                        .await?;
                    self.sql(claim, authority, "postgres", &statements.mark)
                        .await?;
                }
                self.sql(claim, authority, "postgres", &statements.database)
                    .await?;
                self.sql(claim, authority, &database.name, &statements.schema)
                    .await?;
                if job.action == DatabaseAction::RotateCredentials
                    && let Some(role) =
                        super::exports::ExportRole::new(&database.id, &database.name, &b.kind)
                            .map_err(|_| ExecutionError::Ownership)?
                {
                    self.sql(claim, authority, "postgres", &role.disable)
                        .await?;
                }
                self.configure_export(claim, authority).await?;
                self.patch_cluster(
                    claim,
                    authority,
                    json!({"spec":{"postgresql":{"pg_hba":self.config.hba(job)?}}}),
                )
                .await?;
                self.passwords(claim, authority).await?;
                self.sql(claim, authority, "postgres", &statements.enable)
                    .await?;
                self.enable_export(claim, authority).await?;
            }
            DatabaseAction::Pause | DatabaseAction::Delete if job.database.is_some() => {
                if !self.database_exists(claim, authority).await? {
                    return Err(ExecutionError::Ownership);
                }
                self.sql(
                    claim,
                    authority,
                    "postgres",
                    &super::sql::statements(job)?.disable,
                )
                .await?;
                if b.kind == "dedicated" {
                    self.patch_cluster(
                        claim,
                        authority,
                        json!({"metadata":{"annotations":{"cnpg.io/hibernation":"on"}}}),
                    )
                    .await?;
                }
            }
            DatabaseAction::Restart if b.kind == "dedicated" => {
                let pod = self
                    .get("Pod", "postgres-1", Some(&b.namespace))
                    .await?
                    .ok_or(ExecutionError::Ownership)?;
                if job.previous_pod_uid.is_none()
                    || pod.metadata.uid != job.previous_pod_uid
                    || !pod
                        .metadata
                        .owner_references
                        .as_ref()
                        .is_some_and(|refs| refs.iter().any(|r| r.uid == uid))
                {
                    return Err(ExecutionError::Ownership);
                }
                self.guarded(claim, authority, async {
                    self.api("Pod", Some(&b.namespace))?
                        .delete(
                            "postgres-1",
                            &DeleteParams {
                                preconditions: Some(Preconditions {
                                    uid: pod.metadata.uid,
                                    resource_version: pod.metadata.resource_version,
                                }),
                                ..Default::default()
                            },
                        )
                        .await
                        .map(|_| ())
                        .map_err(|_| ExecutionError::Runtime)
                })
                .await?;
            }
            DatabaseAction::Resize if job.database.is_some() => {
                let d = job.database.as_ref().unwrap();
                let existing_quota = self
                    .get("ResourceQuota", "database-budget", Some(&b.namespace))
                    .await?
                    .ok_or(ExecutionError::Ownership)?;
                self.owned(&existing_quota, job)?;
                if existing_quota
                    .metadata
                    .labels
                    .as_ref()
                    .is_some_and(|labels| labels.contains_key("webdock.dev/backup-policy"))
                {
                    self.verify_backup_runtime().await?;
                }
                // Resource changes can immediately replace the primary Pod; finish SQL first.
                self.sql(claim,authority,"postgres",&format!("ALTER ROLE \"wd_migrate_{}\" CONNECTION LIMIT {}; ALTER ROLE \"wd_app_{}\" CONNECTION LIMIT {};",d.id,d.connection_limit,d.id,d.connection_limit)).await?;
                if b.kind == "dedicated" {
                    let objects = backend_manifests(job, &self.config)?;
                    let quota = self
                        .get("ResourceQuota", "database-budget", Some(&b.namespace))
                        .await?
                        .ok_or(ExecutionError::Ownership)?;
                    self.owned(&quota, job)?;
                    let current =
                        serde_json::to_value(&cluster).map_err(|_| ExecutionError::Runtime)?;
                    let quota = resize_quota(
                        job,
                        &self.config,
                        &current,
                        &serde_json::to_value(&quota).map_err(|_| ExecutionError::Runtime)?,
                    )?;
                    self.apply(quota, claim, authority).await?;
                    let resources = objects
                        .iter()
                        .find(|o| o["kind"] == "Cluster")
                        .ok_or(ExecutionError::Runtime)?["spec"]["resources"]
                        .clone();
                    self.patch_cluster(claim, authority, json!({"spec":{"resources":resources}}))
                        .await?;
                }
            }
            DatabaseAction::Purge if job.database.is_some() && b.kind == "shared-pool" => {
                let d = job.database.as_ref().unwrap();
                if self.database_exists(claim, authority).await? {
                    self.sql(
                        claim,
                        authority,
                        "postgres",
                        &super::sql::statements(job)?.disable,
                    )
                    .await?;
                    self.sql(
                        claim,
                        authority,
                        "postgres",
                        &format!("DROP DATABASE \"{}\";", d.name),
                    )
                    .await?;
                }
                for prefix in ["wd_owner_", "wd_migrate_", "wd_app_"] {
                    let role = format!("{prefix}{}", d.id);
                    let owner=self.sql(claim,authority,"postgres",&format!("SELECT COALESCE(pg_catalog.shobj_description(oid,'pg_authid'),'unowned') FROM pg_catalog.pg_roles WHERE rolname='{role}';")).await?;
                    if !owner.is_empty() && owner != format!("webdock:database:{}", d.id) {
                        return Err(ExecutionError::Ownership);
                    }
                    let references=self.sql(claim,authority,"postgres",&format!("SELECT count(*) FROM pg_catalog.pg_shdepend WHERE refclassid='pg_authid'::regclass AND refobjid=(SELECT oid FROM pg_catalog.pg_roles WHERE rolname='{role}');")).await?;
                    if references == "0" {
                        self.sql(
                            claim,
                            authority,
                            "postgres",
                            &format!("DROP ROLE IF EXISTS \"{role}\";"),
                        )
                        .await?;
                    }
                }
            }
            DatabaseAction::Create | DatabaseAction::PrepareMigration | DatabaseAction::Restore => {
            }
            _ => return Err(ExecutionError::Unsupported("database maintenance action")),
        }
        if !matches!(
            job.action,
            DatabaseAction::Pause | DatabaseAction::Delete | DatabaseAction::Purge
        ) && !self.endpoints(claim, authority).await?
        {
            return Err(ExecutionError::Runtime);
        }
        self.checkpoint(claim, authority, DatabaseCheckpointPhase::Observing, &uid)
            .await?;
        Ok(DatabaseStep::Pending)
    }
    pub(super) async fn observe_stage(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        let job = &claim.spec;
        let b = &job.backend;
        let cluster = self
            .get("Cluster", "postgres", Some(&b.namespace))
            .await?
            .ok_or(ExecutionError::Ownership)?;
        self.owned(&cluster, job)?;
        let uid = cluster
            .metadata
            .uid
            .clone()
            .ok_or(ExecutionError::Ownership)?;
        self.checkpoint(claim, authority, DatabaseCheckpointPhase::Observing, &uid)
            .await?;
        if matches!(job.action, DatabaseAction::Pause | DatabaseAction::Delete) {
            if b.kind == "dedicated" {
                let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
                    kube::Api::namespaced(self.client.clone(), &b.namespace);
                let pods = pods
                    .list(&kube::api::ListParams::default().limit(201))
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
                if pods.metadata.continue_.is_some_and(|s| !s.is_empty()) || pods.items.len() > 200
                {
                    return Err(ExecutionError::Runtime);
                }
                if pods
                    .items
                    .iter()
                    .filter(|p| {
                        !p.status
                            .as_ref()
                            .and_then(|s| s.phase.as_deref())
                            .is_some_and(|phase| matches!(phase, "Succeeded" | "Failed"))
                    })
                    .filter_map(|p| p.spec.as_ref()?.volumes.as_ref())
                    .flatten()
                    .filter_map(|v| v.persistent_volume_claim.as_ref())
                    .any(|claim| claim.claim_name == "postgres-1")
                {
                    return Ok(DatabaseStep::Pending);
                }
                let helper = crate::storage::database::DatabaseStorageClient::new(
                    self.config.storage_route(&b.node, &b.pool_id)?.into(),
                );
                self.guarded(
                    claim,
                    authority,
                    helper.retained(&self.storage_packet(claim)?),
                )
                .await?;
            } else if let Some(d) = &job.database
                && self.sql(claim,authority,"postgres",&format!("SELECT count(*) FROM pg_catalog.pg_roles WHERE rolname IN ('wd_migrate_{}','wd_app_{}') AND rolcanlogin;",d.id,d.id)).await?!="0"{return Ok(DatabaseStep::Pending);}
            return Ok(DatabaseStep::Complete(DatabaseCompletionProof {
                installation_id: b.id.clone(),
                database_id: job.database.as_ref().map(|d| d.id.clone()),
                cnpg_uid: uid,
                revision: job.revision,
                state: if job.action == DatabaseAction::Pause {
                    DatabaseProofState::Paused
                } else {
                    DatabaseProofState::Retained
                },
                endpoint_ready: b.kind == "shared-pool",
                copy_sha256: None,
                backup: None,
            }));
        }
        if job.action == DatabaseAction::Purge {
            if self.database_exists(claim, authority).await? {
                return Ok(DatabaseStep::Pending);
            }
            return Ok(DatabaseStep::Complete(DatabaseCompletionProof {
                installation_id: b.id.clone(),
                database_id: job.database.as_ref().map(|d| d.id.clone()),
                cnpg_uid: uid,
                revision: job.revision,
                state: DatabaseProofState::Purged,
                endpoint_ready: true,
                copy_sha256: None,
                backup: None,
            }));
        }
        let Some(pod) = self.get("Pod", "postgres-1", Some(&b.namespace)).await? else {
            return Ok(DatabaseStep::Pending);
        };
        if cluster.data["status"]["readyInstances"] != 1
            || !pod.data["status"]["conditions"]
                .as_array()
                .is_some_and(|conditions| {
                    conditions
                        .iter()
                        .any(|c| c["type"] == "Ready" && c["status"] == "True")
                })
            || !pod
                .metadata
                .owner_references
                .as_ref()
                .is_some_and(|refs| refs.iter().any(|r| r.uid == uid))
        {
            return Ok(DatabaseStep::Pending);
        }
        let container = pod.data["spec"]["containers"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["name"] == "postgres"))
            .ok_or(ExecutionError::Ownership)?;
        if container["image"] != self.config.image {
            return Err(ExecutionError::Ownership);
        }
        for kind in ["requests", "limits"] {
            if crate::kubernetes::parse_quantity(
                container["resources"][kind]["cpu"]
                    .as_str()
                    .ok_or(ExecutionError::Ownership)?,
                true,
            )? != b.resources.cpu_millicores
                || crate::kubernetes::parse_quantity(
                    container["resources"][kind]["memory"]
                        .as_str()
                        .ok_or(ExecutionError::Ownership)?,
                    false,
                )? != b.resources.memory_bytes
            {
                return Ok(DatabaseStep::Pending);
            }
        }
        if job.action == DatabaseAction::Restart
            && (job.previous_pod_uid.is_none() || pod.metadata.uid == job.previous_pod_uid)
        {
            return Ok(DatabaseStep::Pending);
        }
        if job.database.is_some() && !self.database_exists(claim, authority).await? {
            return Err(ExecutionError::Ownership);
        }
        let Some(pooler) = self.get("Deployment", "pooler", Some(&b.namespace)).await? else {
            return Ok(DatabaseStep::Pending);
        };
        let Some(certificate) = self
            .get("Certificate", "server-tls", Some(&b.namespace))
            .await?
        else {
            return Ok(DatabaseStep::Pending);
        };
        self.owned(&certificate, job)?;
        if pooler.data["status"]["readyReplicas"] != 1
            || !certificate.data["status"]["conditions"]
                .as_array()
                .is_some_and(|cs| {
                    cs.iter()
                        .any(|c| c["type"] == "Ready" && c["status"] == "True")
                })
        {
            return Ok(DatabaseStep::Pending);
        }
        for (name, port) in [
            ("public-direct", b.direct_port),
            ("public-pooled", b.pooled_port),
        ] {
            let Some(service) = self.get("Service", name, Some(&b.namespace)).await? else {
                return Ok(DatabaseStep::Pending);
            };
            self.owned(&service, job)?;
            if service.data["spec"]["type"] != "NodePort"
                || !service.data["spec"]["ports"]
                    .as_array()
                    .is_some_and(|ports| ports.iter().any(|p| p["nodePort"] == port))
            {
                return Err(ExecutionError::Ownership);
            }
        }
        if !self.verify_logins(claim, authority).await? {
            return Ok(DatabaseStep::Pending);
        }
        self.guard(claim, authority).await?;
        Ok(DatabaseStep::Complete(DatabaseCompletionProof {
            installation_id: b.id.clone(),
            database_id: job.database.as_ref().map(|d| d.id.clone()),
            cnpg_uid: uid,
            revision: job.revision,
            state: DatabaseProofState::Ready,
            endpoint_ready: true,
            copy_sha256: if job.recovery.is_some() {
                Some(self.recovery_digest(claim, authority).await?)
            } else {
                None
            },
            backup: None,
        }))
    }
    pub(super) async fn verify_logins(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<bool> {
        let Some(d) = &claim.spec.database else {
            return Err(ExecutionError::Unsupported("physical-only TLS observation"));
        };
        let credentials = claim
            .material
            .credentials
            .as_ref()
            .ok_or(ExecutionError::Denied)?;
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut self.ca.as_slice()) {
            roots
                .add(cert.map_err(|_| ExecutionError::Invalid("database trust"))?)
                .map_err(|_| ExecutionError::Invalid("database trust"))?;
        }
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| ExecutionError::Runtime)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let mut logins = vec![
            (
                format!("wd_migrate_{}", d.id),
                credentials.migration_password.clone(),
                claim.spec.backend.direct_port,
            ),
            (
                format!("wd_app_{}", d.id),
                credentials.application_password.clone(),
                claim.spec.backend.pooled_port,
            ),
        ];
        if let Some(export) =
            super::exports::ExportRole::new(&d.id, &d.name, &claim.spec.backend.kind)
                .map_err(|_| ExecutionError::Ownership)?
        {
            logins.push((
                export.name,
                super::exports::ExportRole::password(&d.id, &credentials.application_password)
                    .map_err(|_| ExecutionError::Denied)?,
                claim.spec.backend.direct_port,
            ));
        }
        for (role, password, port) in logins {
            let mut config = tokio_postgres::Config::new();
            config
                .host(&claim.spec.backend.host)
                .port(port)
                .dbname(&d.name)
                .user(&role)
                .password(&password)
                .ssl_mode(tokio_postgres::config::SslMode::Require)
                .connect_timeout(std::time::Duration::from_secs(3));
            let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls.clone());
            let work = async {
                let (client, connection) = config
                    .connect(tls)
                    .await
                    .map_err(|_| ExecutionError::Runtime)?;
                tokio::pin!(connection);
                let validation = async {
                    let row=client.query_one("SELECT current_user::text,pg_catalog.current_database()::text,has_schema_privilege(current_user,'app','USAGE')",&[]).await.map_err(|_|ExecutionError::Runtime)?;
                    if row.get::<_, String>(0) != role
                        || row.get::<_, String>(1) != d.name
                        || !row.get::<_, bool>(2)
                    {
                        return Err(ExecutionError::Ownership);
                    }
                    if role.starts_with("wd_export_") {
                        let row=client.query_one("SELECT NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=current_user AND (rolsuper OR rolcreaterole OR rolcreatedb OR rolreplication OR rolbypassrls OR rolinherit)) AND NOT EXISTS(SELECT 1 FROM pg_auth_members WHERE member=current_user::regrole) AND NOT has_database_privilege(current_user,current_database(),'CREATE,TEMP') AND NOT has_schema_privilege(current_user,'app','CREATE') AND NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='app' AND c.relkind IN('r','p','v','m','f') AND (has_table_privilege(current_user,c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER') OR has_any_column_privilege(current_user,c.oid,'INSERT,UPDATE,REFERENCES'))) AND NOT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='app' AND c.relkind='S' AND has_sequence_privilege(current_user,c.oid,'USAGE,UPDATE'))",&[]).await.map_err(|_|ExecutionError::Runtime)?;
                        row.get::<_, bool>(0)
                            .then_some(())
                            .ok_or(ExecutionError::Ownership)?;
                    }
                    if claim.spec.recovery.is_some() && role.starts_with("wd_migrate_") {
                        client.batch_execute(&format!("BEGIN; CREATE TABLE app.wd_restore_probe_{}(value integer); INSERT INTO app.wd_restore_probe_{} VALUES(1); ROLLBACK;",claim.spec.operation_id,claim.spec.operation_id)).await.map_err(|_|ExecutionError::Runtime)?;
                    }
                    Ok(())
                };
                tokio::pin!(validation);
                tokio::select! {result=&mut validation=>result?,_=&mut connection=>return Err(ExecutionError::Runtime)};
                Ok(())
            };
            match self
                .guarded(claim, authority, async {
                    tokio::time::timeout(std::time::Duration::from_secs(8), work)
                        .await
                        .map_err(|_| ExecutionError::Runtime)?
                })
                .await
            {
                Ok(()) => {}
                Err(ExecutionError::Runtime) => return Ok(false),
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}

impl NativeDatabaseExecutor {
    async fn volume_users(&self, namespace: &str) -> Result<bool> {
        let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
            kube::Api::namespaced(self.client.clone(), namespace);
        let pods = match pods
            .list(&kube::api::ListParams::default().limit(201))
            .await
        {
            Ok(p) => p,
            Err(kube::Error::Api(e)) if e.code == 404 => return Ok(false),
            Err(_) => return Err(ExecutionError::Runtime),
        };
        if pods.items.len() > 200 || pods.metadata.continue_.is_some_and(|v| !v.is_empty()) {
            return Err(ExecutionError::Runtime);
        }
        Ok(pods
            .items
            .iter()
            .filter(|p| {
                !p.status
                    .as_ref()
                    .and_then(|s| s.phase.as_deref())
                    .is_some_and(|phase| matches!(phase, "Succeeded" | "Failed"))
            })
            .any(|p| {
                p.spec
                    .as_ref()
                    .and_then(|s| s.volumes.as_ref())
                    .is_some_and(|v| {
                        v.iter().any(|v| {
                            v.persistent_volume_claim
                                .as_ref()
                                .is_some_and(|c| c.claim_name == "postgres-1")
                        })
                    })
            }))
    }
    pub(super) async fn delete_owned(
        &self,
        kind: &str,
        name: &str,
        namespace: Option<&str>,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
        cnpg_owner: bool,
    ) -> Result<bool> {
        let Some(object) = self.get(kind, name, namespace).await? else {
            return Ok(false);
        };
        if cnpg_owner {
            if object.metadata.namespace.as_deref() != Some(claim.spec.backend.namespace.as_str())
                || !object
                    .metadata
                    .owner_references
                    .as_ref()
                    .is_some_and(|refs| {
                        refs.iter()
                            .any(|r| Some(&r.uid) == claim.spec.backend.cnpg_uid.as_ref())
                    })
            {
                return Err(ExecutionError::Ownership);
            }
        } else {
            self.owned(&object, &claim.spec)?;
        }
        let uid = object.metadata.uid.ok_or(ExecutionError::Ownership)?;
        let version = object
            .metadata
            .resource_version
            .ok_or(ExecutionError::Ownership)?;
        self.guarded(claim, authority, async {
            self.api(kind, namespace)?
                .delete(
                    name,
                    &DeleteParams {
                        propagation_policy: Some(kube::api::PropagationPolicy::Foreground),
                        preconditions: Some(Preconditions {
                            uid: Some(uid),
                            resource_version: Some(version),
                        }),
                        ..Default::default()
                    },
                )
                .await
                .map(|_| ())
                .map_err(|_| ExecutionError::Runtime)
        })
        .await?;
        Ok(true)
    }
    pub(super) async fn purge_stage(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        use webdock_agent_protocol::database::DatabaseExecutionPhase;
        let b = &claim.spec.backend;
        let uid = b.cnpg_uid.as_deref().ok_or(ExecutionError::Ownership)?;
        let namespace = self.get("Namespace", &b.namespace, None).await?;
        if let Some(namespace) = &namespace {
            self.owned(namespace, &claim.spec)?;
        }
        let helper = crate::storage::database::DatabaseStorageClient::new(
            self.config.storage_route(&b.node, &b.pool_id)?.into(),
        );
        let packet = self.storage_packet(claim)?;
        match claim.spec.phase {
            DatabaseExecutionPhase::Apply => {
                if let Some(cluster) = self.get("Cluster", "postgres", Some(&b.namespace)).await? {
                    self.owned(&cluster, &claim.spec)?;
                    if cluster
                        .metadata
                        .annotations
                        .as_ref()
                        .and_then(|a| a.get("cnpg.io/hibernation"))
                        .map(String::as_str)
                        != Some("on")
                        || self.volume_users(&b.namespace).await?
                    {
                        return Err(ExecutionError::Ownership);
                    }
                    self.verify_storage(claim, authority).await?;
                }
                for (kind, name) in [
                    ("Service", "public-direct"),
                    ("Service", "public-pooled"),
                    ("Pooler", "pooler"),
                    ("Cluster", "postgres"),
                ] {
                    self.delete_owned(kind, name, Some(&b.namespace), claim, authority, false)
                        .await?;
                }
                self.checkpoint(claim, authority, DatabaseCheckpointPhase::Applied, uid)
                    .await?;
                Ok(DatabaseStep::Pending)
            }
            DatabaseExecutionPhase::Configure => {
                if self
                    .get("Cluster", "postgres", Some(&b.namespace))
                    .await?
                    .is_some()
                    || self
                        .get("Pooler", "pooler", Some(&b.namespace))
                        .await?
                        .is_some()
                    || self.volume_users(&b.namespace).await?
                {
                    return Ok(DatabaseStep::Pending);
                }
                let pv = self
                    .get("PersistentVolume", &format!("wd-db-{}", b.id), None)
                    .await?;
                let pvc = self
                    .get("PersistentVolumeClaim", "postgres-1", Some(&b.namespace))
                    .await?;
                if let Some(pv) = &pv {
                    self.owned(pv, &claim.spec)?;
                    if pv.data["spec"]["claimRef"]["namespace"] != b.namespace
                        || pv.data["spec"]["claimRef"]["name"] != "postgres-1"
                    {
                        return Err(ExecutionError::Ownership);
                    }
                    if let Some(pvc) = &pvc
                        && pv.data["spec"]["claimRef"]["uid"].as_str()
                            != pvc.metadata.uid.as_deref()
                    {
                        return Err(ExecutionError::Ownership);
                    }
                }
                if let Some(pvc) = &pvc {
                    if pvc.data["spec"]["volumeName"] != format!("wd-db-{}", b.id) {
                        return Err(ExecutionError::Ownership);
                    }
                    self.delete_owned(
                        "PersistentVolumeClaim",
                        "postgres-1",
                        Some(&b.namespace),
                        claim,
                        authority,
                        true,
                    )
                    .await?;
                    return Ok(DatabaseStep::Pending);
                }
                if pv.is_some() {
                    self.delete_owned(
                        "PersistentVolume",
                        &format!("wd-db-{}", b.id),
                        None,
                        claim,
                        authority,
                        false,
                    )
                    .await?;
                    return Ok(DatabaseStep::Pending);
                }
                self.checkpoint(claim, authority, DatabaseCheckpointPhase::Configuring, uid)
                    .await?;
                self.guarded(claim, authority, helper.purge(&packet))
                    .await?;
                if namespace.is_some() {
                    self.delete_owned("Namespace", &b.namespace, None, claim, authority, false)
                        .await?;
                }
                self.checkpoint(claim, authority, DatabaseCheckpointPhase::Observing, uid)
                    .await?;
                Ok(DatabaseStep::Pending)
            }
            DatabaseExecutionPhase::Observe => {
                self.checkpoint(claim, authority, DatabaseCheckpointPhase::Observing, uid)
                    .await?;
                if namespace.is_some()
                    || self
                        .get("PersistentVolume", &format!("wd-db-{}", b.id), None)
                        .await?
                        .is_some()
                {
                    return Ok(DatabaseStep::Pending);
                }
                if self
                    .guarded(claim, authority, helper.state(&packet))
                    .await?
                    .state
                    != crate::storage::VolumeState::Purged
                {
                    return Ok(DatabaseStep::Pending);
                }
                Ok(DatabaseStep::Complete(DatabaseCompletionProof {
                    installation_id: b.id.clone(),
                    database_id: claim.spec.database.as_ref().map(|d| d.id.clone()),
                    cnpg_uid: uid.into(),
                    revision: claim.spec.revision,
                    state: DatabaseProofState::Purged,
                    endpoint_ready: false,
                    copy_sha256: None,
                    backup: None,
                }))
            }
            DatabaseExecutionPhase::Uncertain => Err(ExecutionError::Conflict),
        }
    }
}

fn resize_quota(
    job: &DatabaseJobSpec,
    config: &DatabaseRuntimeConfig,
    current: &serde_json::Value,
    quota: &serde_json::Value,
) -> Result<serde_json::Value> {
    let mut result = backend_manifests(job, config)?
        .into_iter()
        .find(|object| object["kind"] == "ResourceQuota")
        .ok_or(ExecutionError::Runtime)?;
    let main = &current["spec"]["resources"]["requests"];
    let hard = quota["spec"]["hard"]
        .as_object()
        .ok_or(ExecutionError::Ownership)?;
    result["spec"]["hard"] = serde_json::Value::Object(hard.clone());
    for (dimension, cpu, new, marker) in [
        (
            "cpu",
            true,
            job.backend.resources.cpu_millicores,
            "webdock.dev/quota-main-cpu-millicores",
        ),
        (
            "memory",
            false,
            job.backend.resources.memory_bytes,
            "webdock.dev/quota-main-memory-bytes",
        ),
    ] {
        let live = crate::kubernetes::parse_quantity(
            main[dimension].as_str().ok_or(ExecutionError::Ownership)?,
            cpu,
        )?;
        let old = match quota["metadata"]["annotations"].get(marker) {
            Some(value) => value
                .as_str()
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or(ExecutionError::Ownership)?,
            None => live,
        };
        if old != live
            && (old != new
                || quota["metadata"]["annotations"]["webdock.dev/database-operation"]
                    != job.operation_id)
        {
            return Err(ExecutionError::Ownership);
        }
        let delta = new.checked_sub(old).ok_or(ExecutionError::Ownership)?;
        for kind in ["requests", "limits"] {
            let name = format!("{kind}.{dimension}");
            let before = crate::kubernetes::parse_quantity(
                hard.get(&name)
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ExecutionError::Ownership)?,
                cpu,
            )?;
            let after = before.checked_add(delta).ok_or(ExecutionError::Runtime)?;
            result["spec"]["hard"][name] = if cpu {
                json!(format!("{after}m"))
            } else {
                json!(after.to_string())
            };
        }
        result["metadata"]["annotations"][marker] = json!(new.to_string());
    }
    result["metadata"]["annotations"]["webdock.dev/database-operation"] = json!(job.operation_id);
    Ok(result)
}
#[cfg(test)]
mod resize_tests {
    use super::*;
    #[test]
    fn resize_keeps_backup_and_restore_headroom_instead_of_replacing_it_with_ordinary_quota() {
        let job:DatabaseJobSpec=serde_json::from_value(json!({"version":1,"operationID":"101","agentGeneration":1,"operationGeneration":1,"clusterID":"789","action":"resize","phase":"configure","revision":2,"backend":{"id":"456","kind":"dedicated","namespace":"wd-db-456","node":"fixture-node","poolID":"123e4567-e89b-42d3-a456-426614174000","revision":2,"cnpgUID":"owned-uid","resources":{"cpuMillicores":1000,"memoryBytes":1073741824,"volumeBytes":1610612736},"host":"db-456.example.test","directPort":32000,"pooledPort":32001},"database":{"id":"123","kind":"dedicated","name":"db_123","revision":2,"credentialRevision":1,"connectionLimit":10},"members":[{"id":"123","projectID":"321","databaseName":"db_123","enabled":true}]})).unwrap();
        let config:DatabaseRuntimeConfig=serde_json::from_value(json!({"image":format!("ghcr.io/cloudnative-pg/postgresql:18.6@sha256:{}","a".repeat(64)),"poolerImage":format!("ghcr.io/cloudnative-pg/pgbouncer:1.26@sha256:{}","b".repeat(64)),"issuer":"test-ca","apiCIDRs":["10.43.0.1/32"],"podCIDRs":["10.42.0.0/16"],"storageSocket":"/run/webdock-postgres-storage/test.sock","caFile":"/private/ca.crt"})).unwrap();
        let current = json!({"spec":{"resources":{"requests":{"cpu":"500m","memory":"1Gi"},"limits":{"cpu":"500m","memory":"1Gi"}}}});
        let existing = json!({"metadata":{"labels":{"webdock.dev/database-installation":"456","webdock.dev/backup-policy":"777"}},"spec":{"hard":{"requests.cpu":"1300m","limits.cpu":"1300m","requests.memory":"2415919104","limits.memory":"2415919104","requests.ephemeral-storage":"1342177280","limits.ephemeral-storage":"1342177280","requests.storage":"1610612736","pods":"4","persistentvolumeclaims":"1"}}});
        let quota = resize_quota(&job, &config, &current, &existing).unwrap();
        assert_eq!(quota["spec"]["hard"]["requests.cpu"], "1800m");
        assert_eq!(quota["spec"]["hard"]["requests.memory"], "2415919104");
        assert_eq!(
            quota["spec"]["hard"]["requests.ephemeral-storage"],
            "1342177280"
        );
        let retry = resize_quota(&job, &config, &current, &quota).unwrap();
        assert_eq!(
            retry["spec"]["hard"], quota["spec"]["hard"],
            "retry after quota commit but before Cluster patch must not add the resize delta twice"
        );
    }
}
