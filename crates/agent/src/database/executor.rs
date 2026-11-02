use super::*;
use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, GroupVersionKind, Patch, PatchParams, PostParams},
};
use serde_json::{Value, json};
use webdock_agent_protocol::database::{
    DatabaseCheckpoint, DatabaseCheckpointPhase, DatabaseClaim, DatabaseCompletionProof,
    DatabaseExecutionPhase, DatabaseJobSpec,
};
#[async_trait::async_trait]
pub trait DatabaseAuthority: Send + Sync {
    async fn guard(&self, _claim: &DatabaseClaim) -> Result<()> {
        Err(ExecutionError::Denied)
    }
    async fn checkpoint(
        &self,
        _claim: &DatabaseClaim,
        _checkpoint: DatabaseCheckpoint,
    ) -> Result<()> {
        Err(ExecutionError::Denied)
    }
}
pub struct DenyDatabaseAuthority;
#[async_trait::async_trait]
impl DatabaseAuthority for DenyDatabaseAuthority {}
#[async_trait::async_trait]
pub trait DatabaseRuntime: Send + Sync {
    async fn capabilities(&self, _nodes: &[String]) -> Result<Option<DatabaseCapabilities>> {
        Ok(None)
    }
    async fn execute(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep>;
}
#[async_trait::async_trait]
impl DatabaseRuntime for NativeDatabaseExecutor {
    async fn capabilities(&self, nodes: &[String]) -> Result<Option<DatabaseCapabilities>> {
        NativeDatabaseExecutor::capabilities(self, nodes).await
    }
    async fn execute(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        NativeDatabaseExecutor::execute(self, claim, authority).await
    }
}
/// Uses only explicitly supplied operator credentials and private configuration.
pub struct NativeDatabaseExecutor {
    pub(super) client: Client,
    config_path: PathBuf,
    pub(super) config: DatabaseRuntimeConfig,
    pub(super) ca: Vec<u8>,
}
#[derive(Debug)]
pub enum DatabaseStep {
    Pending,
    Complete(DatabaseCompletionProof),
}
impl NativeDatabaseExecutor {
    pub fn open(client: Client, config_path: PathBuf) -> Result<Self> {
        let config = DatabaseRuntimeConfig::load(&config_path)?;
        let ca = config.ca_certificate()?;
        Ok(Self {
            client,
            config_path,
            config,
            ca,
        })
    }
    pub(super) async fn guard(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        authority.guard(claim).await?;
        self.verify_configuration()?;
        self.config.admit(&claim.spec)?;
        if claim.spec.backup.is_some() || claim.spec.recovery.is_some() {
            self.verify_backup_runtime().await?;
        }
        Ok(())
    }
    pub(super) fn verify_configuration(&self) -> Result<()> {
        let config = DatabaseRuntimeConfig::load(&self.config_path)?;
        if serde_json::to_vec(&config).map_err(|_| ExecutionError::Runtime)?
            != serde_json::to_vec(&self.config).map_err(|_| ExecutionError::Runtime)?
            || config.ca_certificate()? != self.ca
        {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    pub(super) async fn guarded<T>(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        self.guard(claim, authority).await?;
        tokio::pin!(work);
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        interval.tick().await;
        loop {
            tokio::select! {result=&mut work=>{let value=result?;self.guard(claim,authority).await?;return Ok(value);},_=interval.tick()=>self.guard(claim,authority).await?,}
        }
    }
    pub(super) fn api(&self, kind: &str, namespace: Option<&str>) -> Result<Api<DynamicObject>> {
        let (group, version) = match kind {
            "Namespace"
            | "PersistentVolume"
            | "PersistentVolumeClaim"
            | "Secret"
            | "Service"
            | "Pod" => ("", "v1"),
            "ResourceQuota" => ("", "v1"),
            "NetworkPolicy" => ("networking.k8s.io", "v1"),
            "Certificate" => ("cert-manager.io", "v1"),
            "ObjectStore" => ("barmancloud.cnpg.io", "v1"),
            "Cluster" | "Pooler" | "Backup" | "ScheduledBackup" => ("postgresql.cnpg.io", "v1"),
            "Deployment" => ("apps", "v1"),
            _ => return Err(ExecutionError::Invalid("database resource kind")),
        };
        let resource = ApiResource::from_gvk(&GroupVersionKind::gvk(group, version, kind));
        Ok(if let Some(namespace) = namespace {
            Api::namespaced_with(self.client.clone(), namespace, &resource)
        } else {
            Api::all_with(self.client.clone(), &resource)
        })
    }
    pub(super) async fn get(
        &self,
        kind: &str,
        name: &str,
        namespace: Option<&str>,
    ) -> Result<Option<DynamicObject>> {
        tokio::time::timeout(
            std::time::Duration::from_secs(25),
            self.api(kind, namespace)?.get_opt(name),
        )
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(|_| ExecutionError::Runtime)
    }
    pub(super) fn owned(&self, object: &DynamicObject, job: &DatabaseJobSpec) -> Result<()> {
        if object
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get("webdock.dev/database-installation"))
            != Some(&job.backend.id)
        {
            return Err(ExecutionError::Ownership);
        }
        if object.types.as_ref().is_some_and(|t| t.kind == "Cluster")
            && job
                .backend
                .cnpg_uid
                .as_ref()
                .is_some_and(|uid| object.metadata.uid.as_ref() != Some(uid))
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    pub(super) async fn apply(
        &self,
        mut value: Value,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DynamicObject> {
        let kind = value["kind"]
            .as_str()
            .ok_or(ExecutionError::Invalid("database object"))?
            .to_owned();
        let name = value["metadata"]["name"]
            .as_str()
            .ok_or(ExecutionError::Invalid("database object"))?
            .to_owned();
        let namespace = value["metadata"]["namespace"].as_str().map(str::to_owned);
        let api = self.api(&kind, namespace.as_deref())?;
        value["metadata"]["annotations"]["webdock.dev/database-operation"] =
            json!(claim.spec.operation_id);
        value["metadata"]["annotations"]["webdock.dev/database-operation-generation"] =
            json!(claim.spec.operation_generation.to_string());
        if self
            .get(&kind, &name, namespace.as_deref())
            .await?
            .is_some()
        {
            return self
                .patch_owned(&kind, &name, namespace.as_deref(), &value, claim, authority)
                .await;
        }
        let value: DynamicObject = serde_json::from_value(value)
            .map_err(|_| ExecutionError::Invalid("database object"))?;
        let object = self
            .guarded(claim, authority, async {
                api.create(
                    &PostParams {
                        field_manager: Some("webdock-postgres".into()),
                        ..PostParams::default()
                    },
                    &value,
                )
                .await
                .map_err(|_| ExecutionError::Runtime)
            })
            .await?;
        self.owned(&object, &claim.spec)?;
        Ok(object)
    }
    pub(super) async fn patch_owned(
        &self,
        kind: &str,
        name: &str,
        namespace: Option<&str>,
        value: &Value,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DynamicObject> {
        let api = self.api(kind, namespace)?;
        // Retry only a server-confirmed rejected write, refreshing ownership and live authority.
        for _ in 0..3 {
            let current = self
                .get(kind, name, namespace)
                .await?
                .ok_or(ExecutionError::Ownership)?;
            self.owned(&current, &claim.spec)?;
            if kind == "Secret" && current.data["data"] != value["data"] {
                return Err(ExecutionError::Ownership);
            }
            let mut patch = value.clone();
            patch["metadata"]["uid"] =
                json!(current.metadata.uid.ok_or(ExecutionError::Ownership)?);
            patch["metadata"]["resourceVersion"] = json!(
                current
                    .metadata
                    .resource_version
                    .ok_or(ExecutionError::Ownership)?
            );
            let updated = self
                .guarded(claim, authority, async {
                    match api
                        .patch(
                            name,
                            &PatchParams {
                                field_manager: Some("webdock-postgres".into()),
                                ..PatchParams::default()
                            },
                            &Patch::Merge(patch),
                        )
                        .await
                    {
                        Ok(object) => Ok(Some(object)),
                        Err(kube::Error::Api(error)) if error.code == 409 => Ok(None),
                        Err(_) => Err(ExecutionError::Runtime),
                    }
                })
                .await?;
            if let Some(object) = updated {
                self.owned(&object, &claim.spec)?;
                return Ok(object);
            }
        }
        Err(ExecutionError::Conflict)
    }
    pub async fn execute(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        self.guard(claim, authority).await?;
        if claim.spec.phase == DatabaseExecutionPhase::Uncertain {
            return Err(ExecutionError::Conflict);
        }
        if matches!(
            claim.spec.action,
            DatabaseAction::ConfigureBackup | DatabaseAction::Backup
        ) {
            return self.backup_stage(claim, authority).await;
        }
        if claim.spec.action == DatabaseAction::Purge && claim.spec.backend.kind == "dedicated" {
            return self.purge_stage(claim, authority).await;
        }
        if claim.spec.action == DatabaseAction::Migrate {
            return Err(ExecutionError::Unsupported("database migration executor"));
        }
        match claim.spec.phase {
            DatabaseExecutionPhase::Apply => self.apply_stage(claim, authority).await,
            DatabaseExecutionPhase::Configure => self.configure_stage(claim, authority).await,
            DatabaseExecutionPhase::Observe => self.observe_stage(claim, authority).await,
            DatabaseExecutionPhase::Uncertain => Err(ExecutionError::Conflict),
        }
    }
    pub(super) fn storage_packet(
        &self,
        claim: &DatabaseClaim,
    ) -> Result<crate::storage::database::DatabaseStoragePacket> {
        let b = &claim.spec.backend;
        Ok(crate::storage::database::DatabaseStoragePacket {
            version: 1,
            installation_id: webdock_core::id::RecordId::parse(&b.id)
                .map_err(|_| ExecutionError::Ownership)?,
            cluster_id: webdock_core::id::RecordId::parse(&claim.spec.cluster_id)
                .map_err(|_| ExecutionError::Ownership)?,
            revision: b.revision as u64,
            node: b.node.clone(),
            pool_id: b.pool_id.clone(),
            volume_bytes: b.resources.volume_bytes,
            lease_until: claim.lease_until,
        })
    }
    pub(super) async fn verify_storage(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<()> {
        let b = &claim.spec.backend;
        let packet = self.storage_packet(claim)?;
        let helper = crate::storage::database::DatabaseStorageClient::new(
            self.config.storage_route(&b.node, &b.pool_id)?.into(),
        );
        let state = self
            .guarded(claim, authority, helper.state(&packet))
            .await?;
        if state.state != crate::storage::VolumeState::Ready
            || state.volume_bytes != b.resources.volume_bytes
        {
            return Err(ExecutionError::Ownership);
        }
        let expected = self
            .guarded(claim, authority, helper.ensure(&packet))
            .await?;
        let claim_object = self
            .get("PersistentVolumeClaim", "postgres-1", Some(&b.namespace))
            .await?;
        let volume = self
            .get("PersistentVolume", &format!("wd-db-{}", b.id), None)
            .await?
            .ok_or(ExecutionError::Ownership)?;
        self.owned(&volume, &claim.spec)?;
        let v = &volume.data["spec"];
        match &claim_object {
            Some(c) => {
                if c.data["spec"]["volumeName"] != format!("wd-db-{}", b.id)
                    || (v["claimRef"]["uid"].as_str() != c.metadata.uid.as_deref()
                        && !(b.cnpg_uid.is_none() && v["claimRef"]["uid"].is_null()))
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            None => {
                if b.cnpg_uid.is_some() || !v["claimRef"]["uid"].is_null() {
                    return Err(ExecutionError::Ownership);
                }
            }
        }
        if v["claimRef"]["namespace"] != b.namespace
            || v["claimRef"]["name"] != "postgres-1"
            || v["local"] != expected.persistent_volume["spec"]["local"]
            || v["nodeAffinity"] != expected.persistent_volume["spec"]["nodeAffinity"]
            || v["persistentVolumeReclaimPolicy"] != "Retain"
            || !v["storageClassName"].as_str().unwrap_or("").is_empty()
            || crate::kubernetes::parse_quantity(
                v["capacity"]["storage"]
                    .as_str()
                    .ok_or(ExecutionError::Ownership)?,
                false,
            )? != b.resources.volume_bytes
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    async fn apply_stage(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> Result<DatabaseStep> {
        let job = &claim.spec;
        let backend = &job.backend;
        let mut cluster = self
            .get("Cluster", "postgres", Some(&backend.namespace))
            .await?;
        if let Some(current) = &cluster {
            self.owned(current, job)?;
            if backend.cnpg_uid.is_none()
                && (current
                    .metadata
                    .annotations
                    .as_ref()
                    .and_then(|a| a.get("webdock.dev/database-operation"))
                    != Some(&job.operation_id)
                    || current
                        .metadata
                        .annotations
                        .as_ref()
                        .and_then(|a| a.get("webdock.dev/database-operation-generation"))
                        != Some(&job.operation_generation.to_string()))
            {
                return Err(ExecutionError::Ownership);
            }
            self.verify_storage(claim, authority).await?;
        } else {
            if backend.cnpg_uid.is_some()
                || backend.revision != 1
                || !matches!(
                    job.action,
                    DatabaseAction::Create
                        | DatabaseAction::PrepareMigration
                        | DatabaseAction::Restore
                )
            {
                return Err(ExecutionError::Ownership);
            }
            let packet = crate::storage::database::DatabaseStoragePacket {
                version: 1,
                installation_id: webdock_core::id::RecordId::parse(&backend.id)
                    .map_err(|_| ExecutionError::Ownership)?,
                cluster_id: webdock_core::id::RecordId::parse(&job.cluster_id)
                    .map_err(|_| ExecutionError::Ownership)?,
                revision: backend.revision as u64,
                node: backend.node.clone(),
                pool_id: backend.pool_id.clone(),
                volume_bytes: backend.resources.volume_bytes,
                lease_until: claim.lease_until,
            };
            self.guard(claim, authority).await?;
            let helper = crate::storage::database::DatabaseStorageClient::new(
                self.config
                    .storage_route(&backend.node, &backend.pool_id)?
                    .into(),
            );
            let storage = self
                .guarded(claim, authority, helper.ensure(&packet))
                .await?;
            self.guard(claim, authority).await?;
            let objects = backend_manifests(job, &self.config)?;
            self.apply(objects[0].clone(), claim, authority).await?;
            self.apply(storage.persistent_volume, claim, authority)
                .await?;
            use base64::Engine;
            self.apply(json!({"apiVersion":"v1","kind":"Secret","metadata":{"name":"server-ca","namespace":backend.namespace,"labels":{"app.kubernetes.io/managed-by":"webdock","webdock.dev/database-installation":backend.id}},"type":"Opaque","data":{"ca.crt":base64::engine::general_purpose::STANDARD.encode(&self.ca)}}),claim,authority).await?;
            for object in objects.into_iter().skip(1) {
                if object["kind"] == "Cluster" && job.recovery.is_some() {
                    for backup in backup_manifests(
                        job,
                        &self.config,
                        claim
                            .material
                            .object_store
                            .as_ref()
                            .ok_or(ExecutionError::Denied)?,
                    )? {
                        self.apply(backup, claim, authority).await?;
                    }
                }
                self.apply(object, claim, authority).await?;
            }
            cluster = self
                .get("Cluster", "postgres", Some(&backend.namespace))
                .await?;
        }
        let cluster = cluster.ok_or(ExecutionError::Runtime)?;
        self.owned(&cluster, job)?;
        if job.action == DatabaseAction::Resume && backend.kind == "dedicated" {
            self.patch_cluster(
                claim,
                authority,
                json!({"metadata":{"annotations":{"cnpg.io/hibernation":"off"}}}),
            )
            .await?;
        }
        let previous_pod_uid = if job.action == DatabaseAction::Restart {
            let pod = self
                .get("Pod", "postgres-1", Some(&backend.namespace))
                .await?
                .ok_or(ExecutionError::Runtime)?;
            if !pod.metadata.owner_references.as_ref().is_some_and(|refs| {
                refs.iter()
                    .any(|r| Some(&r.uid) == cluster.metadata.uid.as_ref())
            }) {
                return Err(ExecutionError::Ownership);
            }
            pod.metadata.uid
        } else {
            None
        };
        self.guard(claim, authority).await?;
        authority
            .checkpoint(
                claim,
                DatabaseCheckpoint {
                    lease: claim.lease.clone(),
                    phase: DatabaseCheckpointPhase::Applied,
                    cnpg_uid: cluster.metadata.uid.ok_or(ExecutionError::Ownership)?,
                    ca_certificate: Some(
                        String::from_utf8(self.ca.clone())
                            .map_err(|_| ExecutionError::Invalid("database CA"))?,
                    ),
                    previous_pod_uid,
                    migration_job_uid: None,
                    copy_sha256: None,
                },
            )
            .await?;
        Ok(DatabaseStep::Pending)
    }
}
