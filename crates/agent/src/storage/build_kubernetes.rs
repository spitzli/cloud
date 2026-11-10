//! Node-owned fixed-volume lifecycle. No customer-selected path or Pod template.
use super::build::{BuildFilesystem, BuildStoragePhase, BuildStorageTrust};
use super::*;
use kube::{
    Api, Client,
    api::{ApiResource, DeleteParams, DynamicObject, ListParams, PostParams, Preconditions},
};
use serde_json::{Value, json};
use spitzli_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildVolumeHandle, SignedBuildStorageReservation,
};

fn invalid() -> ExecutionError {
    ExecutionError::Invalid("build volume binding")
}
fn api_error(error: kube::Error) -> ExecutionError {
    tracing::debug!(%error,"Private build-volume API diagnostic");
    match error {
        kube::Error::Api(status) => tracing::warn!(
            code = status.code,
            "Build volume Kubernetes operation refused"
        ),
        _ => tracing::warn!("Build volume Kubernetes transport failed"),
    }
    ExecutionError::Runtime
}
fn labels(request: &BuildStorageReservation) -> Result<Value> {
    Ok(
        json!({"app.kubernetes.io/managed-by":"webdock-build-storage", "webdock.dev/volume-id":request.volume_id,
        "webdock.dev/customer-id":request.customer_id,"webdock.dev/project-id":request.project_id,
        "webdock.dev/build-id":request.build_id,"webdock.dev/generation":request.generation.to_string()}),
    )
}
fn validate_namespace(namespace: &str) -> Result<()> {
    if !namespace.starts_with("webdock-build-")
        || namespace.len() > 63
        || namespace.ends_with('-')
        || !namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid());
    }
    Ok(())
}
pub fn volume_manifests(
    request: &BuildStorageReservation,
    filesystem: &BuildFilesystem,
    namespace: &str,
    pvc_uid: &str,
) -> Result<(Value, Value)> {
    request.validate_shape().map_err(|_| invalid())?;
    validate_namespace(namespace)?;
    if filesystem.allocation_fingerprint
        != request.allocation_fingerprint().map_err(|_| invalid())?
        || filesystem.volume_bytes != request.volume_bytes
        || !filesystem.data_path.is_absolute()
        || filesystem.data_path.components().any(|p| {
            matches!(
                p,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(ExecutionError::Ownership);
    }
    let name = BuildVolumeHandle::resource_name(request).map_err(|_| invalid())?;
    let labels = labels(request)?;
    let annotations = json!({"webdock.dev/allocation-sha256":request.allocation_fingerprint().map_err(|_|invalid())?});
    let claim = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{"name":name,"namespace":namespace,"labels":labels,"annotations":annotations},
        "spec":{"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","volumeName":name,"resources":{"requests":{"storage":request.volume_bytes.to_string()}}}});
    let volume = json!({"apiVersion":"v1","kind":"PersistentVolume","metadata":{"name":name,"labels":labels,"annotations":annotations},"spec":{
        "accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","capacity":{"storage":request.volume_bytes.to_string()},
        "persistentVolumeReclaimPolicy":"Retain","claimRef":{"apiVersion":"v1","kind":"PersistentVolumeClaim","namespace":namespace,"name":name,"uid":pvc_uid},
        "local":{"path":filesystem.data_path},"nodeAffinity":{"required":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"kubernetes.io/hostname","operator":"In","values":[request.target.node]}]}]}}}});
    Ok((volume, claim))
}
fn quantity(value: &Value, bytes: u64) -> Result<()> {
    if crate::kubernetes::parse_quantity(value.as_str().ok_or_else(invalid)?, false)? != bytes {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
fn verify_metadata(
    actual: &Value,
    expected: &Value,
    uid: Option<&str>,
    retiring: bool,
) -> Result<()> {
    let metadata = &actual["metadata"];
    if metadata["name"] != expected["metadata"]["name"]
        || metadata["annotations"]["webdock.dev/allocation-sha256"]
            != expected["metadata"]["annotations"]["webdock.dev/allocation-sha256"]
        || metadata["namespace"] != expected["metadata"]["namespace"]
        || metadata["labels"] != expected["metadata"]["labels"]
        || (!retiring && !metadata["deletionTimestamp"].is_null())
        || metadata["uid"].as_str().is_none_or(str::is_empty)
        || metadata["resourceVersion"]
            .as_str()
            .is_none_or(str::is_empty)
        || uid.is_some_and(|u| metadata["uid"] != u)
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
fn verify_spec(actual: &Value, expected: &Value, bytes: u64, volume: bool) -> Result<()> {
    let mut spec = actual["spec"].clone();
    let pointer = if volume {
        "/capacity/storage"
    } else {
        "/resources/requests/storage"
    };
    quantity(spec.pointer(pointer).ok_or_else(invalid)?, bytes)?;
    *spec.pointer_mut(pointer).ok_or_else(invalid)? = json!(bytes.to_string());
    if volume {
        if spec.get("storageClassName").is_none() {
            spec["storageClassName"] = json!("");
        }
        if let Some(reference) = spec["claimRef"].as_object_mut() {
            reference.remove("resourceVersion");
        }
    }
    if spec != expected["spec"] {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
pub fn verify_volume_objects(
    handle: &BuildVolumeHandle,
    path: &Path,
    volume: &Value,
    claim: &Value,
) -> Result<()> {
    handle
        .validate_for(&handle.reservation)
        .map_err(|_| invalid())?;
    let filesystem = BuildFilesystem {
        allocation_fingerprint: handle
            .reservation
            .allocation_fingerprint()
            .map_err(|_| invalid())?,
        volume_bytes: handle.reservation.volume_bytes,
        filesystem_uuid: handle.filesystem_uuid.clone(),
        data_path: path.into(),
    };
    let (expected_volume, expected_claim) = volume_manifests(
        &handle.reservation,
        &filesystem,
        &handle.namespace,
        &handle.pvc_uid,
    )?;
    verify_metadata(volume, &expected_volume, Some(&handle.pv_uid), false)?;
    verify_metadata(claim, &expected_claim, Some(&handle.pvc_uid), false)?;
    verify_spec(
        volume,
        &expected_volume,
        handle.reservation.volume_bytes,
        true,
    )?;
    verify_spec(
        claim,
        &expected_claim,
        handle.reservation.volume_bytes,
        false,
    )
}
/// All Pod references matter, including foreign labels, Pending and terminating Pods.
pub fn consumers_absent(
    handle: &BuildVolumeHandle,
    job: Option<&Value>,
    pods: &Value,
) -> Result<bool> {
    handle
        .validate_for(&handle.reservation)
        .map_err(|_| invalid())?;
    consumers_absent_for(&handle.reservation, &handle.namespace, job, pods)
}
fn consumers_absent_for(
    request: &BuildStorageReservation,
    namespace: &str,
    job: Option<&Value>,
    pods: &Value,
) -> Result<bool> {
    validate_namespace(namespace)?;
    let job_name = request.job_name().map_err(|_| invalid())?;
    let claim = BuildVolumeHandle::resource_name(request).map_err(|_| invalid())?;
    let generation = request.generation.to_string();
    if pods["metadata"]["continue"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
    {
        return Err(ExecutionError::Conflict);
    }
    let pods = pods["items"].as_array().ok_or_else(invalid)?;
    if let Some(job) = job {
        if job["metadata"]["name"] != job_name || job["metadata"]["namespace"] != namespace {
            return Err(ExecutionError::Ownership);
        }
        return Ok(false);
    }
    for pod in pods {
        if pod["metadata"]["namespace"] != namespace {
            return Err(ExecutionError::Ownership);
        }
        if pod["metadata"]["labels"]["webdock.dev/build-id"] == request.build_id
            && pod["metadata"]["labels"]["webdock.dev/generation"].as_str()
                == Some(generation.as_str())
            || pod["metadata"]["ownerReferences"]
                .as_array()
                .is_some_and(|owners| {
                    owners
                        .iter()
                        .any(|o| o["kind"] == "Job" && o["name"] == job_name)
                })
            || pod["spec"]["volumes"].as_array().is_none_or(|volumes| {
                volumes
                    .iter()
                    .any(|v| v["persistentVolumeClaim"]["claimName"] == claim)
            })
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindingRecord {
    version: u8,
    reservation: BuildStorageReservation,
    namespace: String,
    filesystem_uuid: String,
    pvc_uid: Option<String>,
    pv_uid: Option<String>,
}
impl BindingRecord {
    fn handle(&self) -> Result<Option<BuildVolumeHandle>> {
        if let (Some(pv), Some(pvc)) = (&self.pv_uid, &self.pvc_uid) {
            Ok(Some(
                BuildVolumeHandle::new(
                    self.reservation.clone(),
                    &self.namespace,
                    pv,
                    pvc,
                    &self.filesystem_uuid,
                )
                .map_err(|_| invalid())?,
            ))
        } else {
            Ok(None)
        }
    }
}
pub use spitzli_agent_protocol::build_storage::{
    BuildVolumePhase as NodeVolumePhase, BuildVolumeResult as NodeVolumeResult,
};

/// Explicit token-only node identity. No ambient context, exec helper, file auth,
/// client-admin certificate, proxy, insecure TLS or endpoint substitution.
pub async fn client_config(
    bytes: &[u8],
    context: &str,
    endpoint: &str,
    ca_sha256: &str,
    namespace: &str,
) -> Result<kube::Config> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    validate_namespace(namespace)?;
    let expected = url::Url::parse(endpoint).map_err(|_| invalid())?;
    if expected.scheme() != "https"
        || expected.host_str().is_none()
        || !expected.username().is_empty()
        || expected.password().is_some()
        || expected.query().is_some()
        || expected.fragment().is_some()
        || expected.path() != "/"
        || context.is_empty()
        || context.len() > 128
        || bytes.len() > 131072
    {
        return Err(invalid());
    }
    let document =
        kube::config::Kubeconfig::from_yaml(std::str::from_utf8(bytes).map_err(|_| invalid())?)
            .map_err(|_| invalid())?;
    if document.contexts.len() != 1
        || document.clusters.len() != 1
        || document.auth_infos.len() != 1
        || !document.other.is_empty()
        || document.extensions.is_some()
        || document.kind.as_deref() != Some("Config")
        || document.api_version.as_deref() != Some("v1")
    {
        return Err(invalid());
    }
    let named = &document.contexts[0];
    let selected = named.context.as_ref().ok_or_else(invalid)?;
    let cluster = document.clusters[0].cluster.as_ref().ok_or_else(invalid)?;
    let auth = document.auth_infos[0]
        .auth_info
        .as_ref()
        .ok_or_else(invalid)?;
    if named.name != context
        || !named.other.is_empty()
        || !selected.other.is_empty()
        || selected.extensions.is_some()
        || selected
            .namespace
            .as_deref()
            .is_some_and(|n| n != namespace)
        || selected.cluster != document.clusters[0].name
        || selected.user.as_deref() != Some(document.auth_infos[0].name.as_str())
        || !document.clusters[0].other.is_empty()
        || !document.auth_infos[0].other.is_empty()
    {
        return Err(invalid());
    }
    let c = serde_json::to_value(cluster).map_err(|_| invalid())?;
    let a = serde_json::to_value(auth).map_err(|_| invalid())?;
    if c.as_object().is_none_or(|v| {
        v.keys().any(|k| {
            ![
                "server",
                "certificate-authority-data",
                "insecure-skip-tls-verify",
                "disable-compression",
            ]
            .contains(&k.as_str())
        })
    }) || cluster.insecure_skip_tls_verify == Some(true)
        || url::Url::parse(cluster.server.as_deref().ok_or_else(invalid)?).map_err(|_| invalid())?
            != expected
        || a.as_object()
            .is_none_or(|v| v.len() != 1 || !v.contains_key("token"))
        || a["token"]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 16384 || s.chars().any(char::is_control))
    {
        return Err(ExecutionError::Denied);
    }
    let ca = base64::engine::general_purpose::STANDARD
        .decode(
            cluster
                .certificate_authority_data
                .as_deref()
                .ok_or_else(invalid)?,
        )
        .map_err(|_| invalid())?;
    if ca.len() > 65536 || format!("{:x}", Sha256::digest(&ca)) != ca_sha256 {
        return Err(ExecutionError::Ownership);
    }
    let mut config = kube::Config::from_custom_kubeconfig(
        document,
        &kube::config::KubeConfigOptions {
            context: Some(context.into()),
            ..Default::default()
        },
    )
    .await
    .map_err(|_| invalid())?;
    config.default_namespace = namespace.into();
    config.proxy_url = None;
    config.default_retry = false;
    config.connect_timeout = Some(Duration::from_secs(5));
    config.read_timeout = Some(Duration::from_secs(10));
    config.write_timeout = Some(Duration::from_secs(10));
    Ok(config)
}

/// One locked pool and an explicitly injected build-cluster identity. This client
/// has no source, registry, Core publication or tenant execution authority.
pub struct BuildVolumeReconciler {
    pool: StoragePool,
    client: Client,
    namespace: String,
}
impl BuildVolumeReconciler {
    pub fn new(pool: StoragePool, client: Client, namespace: String) -> Result<Self> {
        validate_namespace(&namespace)?;
        if pool.kind != StorageKind::Builds {
            return Err(ExecutionError::Denied);
        }
        Ok(Self {
            pool,
            client,
            namespace,
        })
    }
    fn api(&self, kind: &str, plural: &str, scoped: bool) -> Api<DynamicObject> {
        let mut resource = ApiResource::from_gvk(&kube::api::GroupVersionKind::gvk("", "v1", kind));
        if kind == "Job" {
            resource =
                ApiResource::from_gvk(&kube::api::GroupVersionKind::gvk("batch", "v1", kind));
        }
        resource.plural = plural.into();
        if scoped {
            Api::namespaced_with(self.client.clone(), &self.namespace, &resource)
        } else {
            Api::all_with(self.client.clone(), &resource)
        }
    }
    fn binding_path(&self, request: &BuildStorageReservation) -> Result<PathBuf> {
        request.validate_shape().map_err(|_| invalid())?;
        Ok(self
            .pool
            .config
            .root
            .join("_pool")
            .join(format!("build-binding-{}.json", request.volume_id)))
    }
    fn binding(&self, request: &BuildStorageReservation) -> Result<Option<BindingRecord>> {
        self.pool.state()?;
        let path = self.binding_path(request)?;
        if !path.try_exists().map_err(|_| ExecutionError::Runtime)? {
            return Ok(None);
        }
        let record: BindingRecord = read_json(&path, 16384)?;
        if record.version != 1
            || record.namespace != self.namespace
            || record
                .reservation
                .allocation_fingerprint()
                .map_err(|_| invalid())?
                != request.allocation_fingerprint().map_err(|_| invalid())?
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(Some(record))
    }
    fn save(&mut self, record: &BindingRecord) -> Result<()> {
        let parent = self.pool.config.root.join("_pool");
        let mut file = io(tempfile::NamedTempFile::new_in(&parent))?;
        let bytes = serde_json::to_vec(record).map_err(|_| ExecutionError::Journal)?;
        if bytes.len() > 16384 {
            return Err(ExecutionError::Journal);
        }
        io(file.write_all(&bytes))?;
        io(file.as_file().sync_all())?;
        if file
            .persist(self.binding_path(&record.reservation)?)
            .is_err()
            || sync_directory(&parent).is_err()
        {
            self.pool.poisoned = true;
            return Err(ExecutionError::Journal);
        }
        Ok(())
    }
    async fn get(
        &self,
        api: &Api<DynamicObject>,
        name: &str,
        guard: &StorageGuard,
    ) -> Result<Option<Value>> {
        guard.check()?;
        let object = tokio::time::timeout(Duration::from_secs(10), api.get_opt(name))
            .await
            .map_err(|_| ExecutionError::Expired)?
            .map_err(api_error)?;
        guard.check()?;
        object
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| ExecutionError::Runtime)
    }
    async fn ensure_object(
        &self,
        api: &Api<DynamicObject>,
        expected: &Value,
        uid: Option<&str>,
        volume: bool,
        bytes: u64,
        guard: &StorageGuard,
    ) -> Result<Value> {
        let name = expected["metadata"]["name"].as_str().ok_or_else(invalid)?;
        let object = match self.get(api, name, guard).await? {
            Some(object) => object,
            None => {
                if uid.is_some() {
                    return Err(ExecutionError::Ownership);
                }
                guard.check()?;
                let object: DynamicObject =
                    serde_json::from_value(expected.clone()).map_err(|_| invalid())?;
                match tokio::time::timeout(
                    Duration::from_secs(10),
                    api.create(&PostParams::default(), &object),
                )
                .await
                .map_err(|_| ExecutionError::Expired)?
                {
                    Ok(value) => serde_json::to_value(value).map_err(|_| invalid())?,
                    Err(kube::Error::Api(error)) if error.code == 409 => self
                        .get(api, name, guard)
                        .await?
                        .ok_or(ExecutionError::Conflict)?,
                    Err(error) => return Err(api_error(error)),
                }
            }
        };
        guard.check()?;
        verify_metadata(&object, expected, uid, false)?;
        verify_spec(&object, expected, bytes, volume)?;
        Ok(object)
    }
    async fn remove(
        &self,
        api: &Api<DynamicObject>,
        object: &Value,
        guard: &StorageGuard,
    ) -> Result<()> {
        let meta = &object["metadata"];
        let params = DeleteParams {
            preconditions: Some(Preconditions {
                uid: Some(meta["uid"].as_str().ok_or_else(invalid)?.into()),
                resource_version: Some(
                    meta["resourceVersion"].as_str().ok_or_else(invalid)?.into(),
                ),
            }),
            ..DeleteParams::default()
        };
        guard.check()?;
        match tokio::time::timeout(
            Duration::from_secs(10),
            api.delete(meta["name"].as_str().ok_or_else(invalid)?, &params),
        )
        .await
        .map_err(|_| ExecutionError::Expired)?
        {
            Ok(_) => {}
            Err(kube::Error::Api(e)) if e.code == 404 => {}
            Err(error) => return Err(api_error(error)),
        }
        guard.check()
    }
    async fn no_consumers(
        &self,
        request: &BuildStorageReservation,
        guard: &StorageGuard,
    ) -> Result<bool> {
        let jobs = self.api("Job", "jobs", true);
        let job = self
            .get(&jobs, &request.job_name().map_err(|_| invalid())?, guard)
            .await?;
        guard.check()?;
        let pods = self.api("Pod", "pods", true);
        let pods = tokio::time::timeout(
            Duration::from_secs(10),
            pods.list(&ListParams::default().limit(1000)),
        )
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(api_error)?;
        guard.check()?;
        consumers_absent_for(
            request,
            &self.namespace,
            job.as_ref(),
            &serde_json::to_value(pods).map_err(|_| invalid())?,
        )
    }
    pub async fn reconcile(
        &mut self,
        signed: &SignedBuildStorageReservation,
        trust: &BuildStorageTrust,
        guard: &StorageGuard,
    ) -> Result<NodeVolumeResult> {
        let request = &signed.reservation;
        // Verify and durably fence reserve/retire before any Kubernetes I/O.
        let scoped = StorageGuard::new(
            guard.deadline.min(request.expires_at),
            guard.cancelled.clone(),
        );
        let guard = &scoped;
        let result = self.pool.apply_build_storage(signed, trust, guard)?;
        let mut record = self.binding(request)?;
        if result.phase == BuildStoragePhase::Retired {
            if request.action != BuildStorageAction::Retire {
                return Ok(NodeVolumeResult {
                    phase: NodeVolumePhase::Retiring,
                    handle: None,
                });
            }
            return self.retire(request, record, guard).await;
        }
        let filesystem = result.filesystem.ok_or(ExecutionError::Journal)?;
        if record.is_none() {
            if request.action != BuildStorageAction::Reserve {
                return Err(ExecutionError::Conflict);
            }
            record = Some(BindingRecord {
                version: 1,
                reservation: request.clone(),
                namespace: self.namespace.clone(),
                filesystem_uuid: filesystem.filesystem_uuid.clone(),
                pvc_uid: None,
                pv_uid: None,
            });
            self.save(record.as_ref().ok_or(ExecutionError::Journal)?)?;
        }
        let mut record = record.ok_or(ExecutionError::Journal)?;
        if record.filesystem_uuid != filesystem.filesystem_uuid {
            return Err(ExecutionError::Ownership);
        }
        // Kubelet subPath mounts expose only these siblings. No unmounted fallback
        // directory is ever created outside the verified fixed filesystem.
        for name in ["workspace", "cache"] {
            let path = filesystem.data_path.join(name);
            if !path.try_exists().map_err(|_| ExecutionError::Runtime)? {
                if request.action != BuildStorageAction::Reserve {
                    return Err(ExecutionError::Conflict);
                }
                mkdir(&path)?;
                io(std::os::unix::fs::chown(&path, Some(1000), Some(1000)))?;
                io(std::fs::set_permissions(
                    &path,
                    std::fs::Permissions::from_mode(0o770),
                ))?;
            }
            let meta = io(std::fs::symlink_metadata(&path))?;
            if !meta.is_dir()
                || meta.uid() != 1000
                || meta.gid() != 1000
                || io(std::fs::canonicalize(&path))? != path
            {
                return Err(ExecutionError::Ownership);
            }
        }
        let claims = self.api("PersistentVolumeClaim", "persistentvolumeclaims", true);
        let volumes = self.api("PersistentVolume", "persistentvolumes", false);
        let (_, expected_claim) = volume_manifests(request, &filesystem, &self.namespace, "")?;
        let claim = if request.action == BuildStorageAction::Reserve {
            self.ensure_object(
                &claims,
                &expected_claim,
                record.pvc_uid.as_deref(),
                false,
                request.volume_bytes,
                guard,
            )
            .await?
        } else {
            self.get(
                &claims,
                expected_claim["metadata"]["name"]
                    .as_str()
                    .ok_or_else(invalid)?,
                guard,
            )
            .await?
            .ok_or(ExecutionError::Ownership)?
        };
        let claim_uid = claim["metadata"]["uid"]
            .as_str()
            .ok_or_else(invalid)?
            .to_owned();
        if record.pvc_uid.is_none() {
            record.pvc_uid = Some(claim_uid.clone());
            self.save(&record)?;
        }
        let (expected_volume, _) =
            volume_manifests(request, &filesystem, &self.namespace, &claim_uid)?;
        let volume = if request.action == BuildStorageAction::Reserve {
            self.ensure_object(
                &volumes,
                &expected_volume,
                record.pv_uid.as_deref(),
                true,
                request.volume_bytes,
                guard,
            )
            .await?
        } else {
            self.get(
                &volumes,
                expected_volume["metadata"]["name"]
                    .as_str()
                    .ok_or_else(invalid)?,
                guard,
            )
            .await?
            .ok_or(ExecutionError::Ownership)?
        };
        if record.pv_uid.is_none() {
            record.pv_uid = Some(
                volume["metadata"]["uid"]
                    .as_str()
                    .ok_or_else(invalid)?
                    .into(),
            );
            self.save(&record)?;
        }
        let handle = record.handle()?.ok_or(ExecutionError::Journal)?;
        verify_volume_objects(&handle, &filesystem.data_path, &volume, &claim)?;
        // Re-check the actual mount after API waits, before publishing readiness.
        self.pool.apply_build_storage(signed, trust, guard)?;
        Ok(NodeVolumeResult {
            phase: if claim["status"]["phase"] == "Bound" {
                NodeVolumePhase::Ready
            } else {
                NodeVolumePhase::Pending
            },
            handle: Some(handle),
        })
    }
    async fn retire(
        &mut self,
        request: &BuildStorageReservation,
        mut record: Option<BindingRecord>,
        guard: &StorageGuard,
    ) -> Result<NodeVolumeResult> {
        let pending = || NodeVolumeResult {
            phase: NodeVolumePhase::Retiring,
            handle: None,
        };
        if !self.no_consumers(request, guard).await? {
            return Ok(pending());
        }
        let name = BuildVolumeHandle::resource_name(request).map_err(|_| invalid())?;
        let claims = self.api("PersistentVolumeClaim", "persistentvolumeclaims", true);
        let volumes = self.api("PersistentVolume", "persistentvolumes", false);
        let claim = self.get(&claims, &name, guard).await?;
        let volume = self.get(&volumes, &name, guard).await?;
        if claim.is_some() || volume.is_some() {
            let state = record.as_mut().ok_or(ExecutionError::Ownership)?;
            let filesystem = BuildFilesystem {
                allocation_fingerprint: request.allocation_fingerprint().map_err(|_| invalid())?,
                volume_bytes: request.volume_bytes,
                filesystem_uuid: state.filesystem_uuid.clone(),
                data_path: self
                    .pool
                    .config
                    .root
                    .join(&request.volume_id)
                    .join("mount/data"),
            };
            if let Some(claim) = &claim {
                let (_, expected) = volume_manifests(request, &filesystem, &self.namespace, "")?;
                verify_metadata(claim, &expected, state.pvc_uid.as_deref(), true)?;
                verify_spec(claim, &expected, request.volume_bytes, false)?;
                if state.pvc_uid.is_none() {
                    state.pvc_uid = Some(
                        claim["metadata"]["uid"]
                            .as_str()
                            .ok_or_else(invalid)?
                            .into(),
                    );
                    self.save(state)?;
                }
            }
            if let Some(volume) = &volume {
                let (expected, _) = volume_manifests(
                    request,
                    &filesystem,
                    &self.namespace,
                    state.pvc_uid.as_deref().ok_or(ExecutionError::Ownership)?,
                )?;
                verify_metadata(volume, &expected, state.pv_uid.as_deref(), true)?;
                verify_spec(volume, &expected, request.volume_bytes, true)?;
                if state.pv_uid.is_none() {
                    state.pv_uid = Some(
                        volume["metadata"]["uid"]
                            .as_str()
                            .ok_or_else(invalid)?
                            .into(),
                    );
                    self.save(state)?;
                }
            }
        }
        // Removing the exact claim first fences late Job creation: an orchestrator
        // has no PVC/PV creation authority and cannot restore a retired source.
        if let Some(claim) = &claim {
            self.remove(&claims, claim, guard).await?;
        }
        if self.get(&claims, &name, guard).await?.is_some() {
            return Ok(pending());
        }
        if let Some(volume) = &volume {
            self.remove(&volumes, volume, guard).await?;
        }
        if self.get(&volumes, &name, guard).await?.is_some()
            || !self.no_consumers(request, guard).await?
        {
            return Ok(pending());
        }
        guard.check()?;
        let packet = StoragePacket::new(
            &request.volume_id,
            &request.project_id,
            &request.target.cluster_id,
            1,
            request.volume_bytes,
            request
                .expires_at
                .timestamp()
                .try_into()
                .map_err(|_| invalid())?,
        )?;
        // Normal umount plus loop association checks retain uncertain in-flight
        // kubelet bind mounts; never force/lazy detach or unlink a mounted image.
        self.pool.purge_record(&packet, guard)?;
        Ok(NodeVolumeResult {
            phase: NodeVolumePhase::Retired,
            handle: record.map(|r| r.handle()).transpose()?.flatten(),
        })
    }
}
