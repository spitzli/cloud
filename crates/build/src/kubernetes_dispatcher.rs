//! Private dispatcher. The concrete source/result channel and operator enrollment
//! are injected; this module does not enable public Kubernetes build admission.
use crate::{
    Result,
    artifact::{Archive, ArtifactStore, MAX_SOURCE, validate_oci, validate_retained_image_size},
    error,
    execution::GuestRequest,
    io,
    kubernetes::{
        KubernetesBuildLimits, KubernetesBuildRequest, KubernetesJobBuildExecutor,
        KubernetesJobHandle, KubernetesJobPhase, KubernetesLogBounds,
    },
    worker::{ControlClient, Outbox},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Implementations own a reviewed, bounded source/result transport. They must not
/// use Pod exec, host mounts, runtime sockets, or platform/Git/registry credentials.
#[async_trait::async_trait]
pub trait KubernetesArtifactTransfer: Send + Sync {
    /// Bind result authority to an observed exact Job UID before releasing execution.
    async fn bind(&self, _handle: &KubernetesJobHandle) -> Result<()> {
        Ok(())
    }
    /// Revocation precedes every cancellation/cleanup, including an in-flight body.
    async fn revoke(&self, _request: &KubernetesBuildRequest) -> Result<()> {
        Ok(())
    }
    /// Stage exact source and file-only runner input, without starting execution.
    /// A failed/uncertain stage is never automatically retried for this attempt.
    async fn stage(
        &self,
        request: &KubernetesBuildRequest,
        guest: &GuestRequest,
        settings: &crate::contracts::ContainerSettings,
        source: &[u8],
        archive_sha256: &str,
    ) -> Result<()>;
    /// Download a bounded result for this exact Job UID; do not trust its digests.
    async fn collect(&self, handle: &KubernetesJobHandle) -> Result<KubernetesTransferredOutput>;
    /// Delete only this attempt's staged Secret/capabilities after its Job and
    /// Pods are absent. Implementations retain enough identity to reconcile an
    /// uncertain stage without creating or replacing a handoff.
    async fn cleanup(&self, request: &KubernetesBuildRequest) -> Result<()>;
}

pub struct KubernetesTransferredOutput {
    pub build_id: String,
    pub generation: u64,
    pub job_uid: String,
    pub source_sha: String,
    pub source_archive_sha256: String,
    pub image_tar: Vec<u8>,
    pub logs: String,
}

#[derive(Clone)]
struct DispatchAttempt {
    request: KubernetesBuildRequest,
    guest: GuestRequest,
    container_build: crate::contracts::ContainerSettings,
    cluster_id: String,
    namespace: String,
    job_name: String,
    runtime_fingerprint: String,
    spec_fingerprint: Option<String>,
    uid: Option<String>,
    lease_until: i64,
    cancel_requested: bool,
    storage_request: Option<webdock_agent_protocol::build_storage::BuildStorageReservation>,
}

fn text<'a>(value: &'a Value, camel: &str, snake: &str) -> Result<&'a str> {
    value[camel]
        .as_str()
        .or_else(|| value[snake].as_str())
        .ok_or_else(|| error("invalid_kubernetes_attempt"))
}

impl DispatchAttempt {
    fn claim(value: &Value) -> Result<Self> {
        let generation = value["generation"]
            .as_u64()
            .filter(|g| *g > 0 && *g < i32::MAX as u64)
            .ok_or_else(|| error("invalid_generation"))?;
        let mut guest_value = value["request"].clone();
        if guest_value.get("buildEnvironment").is_none() {
            guest_value["buildEnvironment"] = json!({});
        }
        let guest: GuestRequest =
            serde_json::from_value(guest_value).map_err(|_| error("invalid_kubernetes_attempt"))?;
        guest.validate()?;
        let container_build = if value.get("containerBuild").is_none_or(Value::is_null) {
            crate::contracts::ContainerSettings::default()
        } else {
            serde_json::from_value(value["containerBuild"].clone())
                .map_err(|_| error("invalid_container_settings"))?
        };
        container_build.validate()?;
        let legacy_limits = KubernetesBuildLimits {
            cpu_millis: guest
                .limits
                .cpu
                .checked_mul(1000)
                .ok_or_else(|| error("invalid_limits"))?,
            memory_mib: guest.limits.memory_mi_b,
            workspace_mib: u32::try_from(guest.limits.disk_bytes.div_ceil(1024 * 1024))
                .map_err(|_| error("invalid_limits"))?,
            // Native snapshotter copies exceeded both 1 and 4 GiB in qualification.
            // One 10 GiB workspace + 16 GiB cache fits the 40 GiB namespace cap.
            cache_mib: 16384,
            deadline_seconds: u32::try_from(guest.limits.duration_seconds)
                .map_err(|_| error("invalid_limits"))?,
        };
        let budget = value
            .get("kubernetesLimits")
            .or_else(|| value.get("kubernetes_limits"));
        let limits = if let Some(budget) = budget.filter(|v| !v.is_null()) {
            let limits: KubernetesBuildLimits = serde_json::from_value(budget.clone())
                .map_err(|_| error("invalid_kubernetes_budget"))?;
            if limits.cpu_millis != legacy_limits.cpu_millis
                || limits.memory_mib != legacy_limits.memory_mib
                || limits.workspace_mib != legacy_limits.workspace_mib
                || limits.deadline_seconds != legacy_limits.deadline_seconds
                || !(64..=16384).contains(&limits.cache_mib)
            {
                return Err(error("kubernetes_budget_conflict"));
            }
            limits
        } else {
            // Compatibility only for the original closed trusted pilot protocol.
            // Fixed-volume attempts never infer a budget from worker defaults.
            if value
                .get("storageRequest")
                .or_else(|| value.get("storage_request"))
                .is_some_and(|v| !v.is_null())
            {
                return Err(error("kubernetes_budget_missing"));
            }
            legacy_limits
        };
        let volume = value
            .get("volumeHandle")
            .or_else(|| value.get("volume_handle"))
            .filter(|v| !v.is_null())
            .map(|v| {
                serde_json::from_value(v.clone()).map_err(|_| error("invalid_build_storage_handle"))
            })
            .transpose()?;
        let request = KubernetesBuildRequest {
            build_id: text(value, "buildID", "build_id")?.into(),
            customer_id: text(value, "customerID", "customer_id")?.into(),
            project_id: text(value, "projectID", "project_id")?.into(),
            generation,
            attempt: u32::try_from(generation).map_err(|_| error("invalid_generation"))?,
            commit_sha: text(value, "sourceSHA", "source_sha")?.into(),
            limits,
            volume,
        };
        for id in [&request.build_id, &request.customer_id, &request.project_id] {
            crate::contracts::id(id)?;
        }
        let storage_request: Option<
            webdock_agent_protocol::build_storage::BuildStorageReservation,
        > = value
            .get("storageRequest")
            .or_else(|| value.get("storage_request"))
            .filter(|v| !v.is_null())
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|_| error("invalid_build_storage_authority"))
            })
            .transpose()?;
        if let Some(storage) = &storage_request {
            storage
                .validate_shape()
                .map_err(|_| error("invalid_build_storage_authority"))?;
            if storage.build_id != request.build_id
                || storage.customer_id != request.customer_id
                || storage.project_id != request.project_id
                || storage.generation != generation
                || storage.target.cluster_id != text(value, "clusterID", "cluster_id")?
            {
                return Err(error("build_storage_scope_conflict"));
            }
            if let Some(handle) = &request.volume {
                handle
                    .validate_for(storage)
                    .map_err(|_| error("build_storage_scope_conflict"))?;
            }
        } else if request.volume.is_some() {
            return Err(error("build_storage_scope_conflict"));
        }
        let cancel_requested = value["cancelRequested"] == true
            || value["buildStatus"]
                .as_str()
                .is_some_and(|s| s != "running");
        let lease_until = if cancel_requested
            && value["leaseUntil"].is_null()
            && value["lease_until"].is_null()
        {
            // Core deliberately clears terminal execution leases. The exact
            // attempt remains cleanup authority; it cannot authorize new work.
            0
        } else {
            chrono::DateTime::parse_from_rfc3339(text(value, "leaseUntil", "lease_until")?)
                .map_err(|_| error("invalid_lease"))?
                .timestamp()
        };
        Ok(Self {
            request,
            guest,
            container_build,
            cluster_id: text(value, "clusterID", "cluster_id")?.into(),
            namespace: text(value, "namespace", "namespace")?.into(),
            job_name: value["jobName"]
                .as_str()
                .or_else(|| value["job_name"].as_str())
                .unwrap_or_default()
                .into(),
            runtime_fingerprint: value["runtimeFingerprint"]
                .as_str()
                .or_else(|| value["runtime_fingerprint"].as_str())
                .unwrap_or_default()
                .into(),
            spec_fingerprint: value["spec_fingerprint"].as_str().map(str::to_owned),
            uid: value["job_uid"].as_str().map(str::to_owned),
            lease_until,
            cancel_requested,
            storage_request,
        })
    }
    fn generation(&self) -> i32 {
        self.request.generation as i32
    }
    fn expired(&self) -> bool {
        self.lease_until <= chrono::Utc::now().timestamp()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchReceipt {
    version: u8,
    cluster_id: String,
    runtime_fingerprint: String,
    request: KubernetesBuildRequest,
    #[serde(default)]
    container_build: crate::contracts::ContainerSettings,
    spec_fingerprint: String,
    source_archive_sha256: Option<String>,
    uid: Option<String>,
    logs: String,
    outcome: Option<String>,
    completion: Option<Value>,
    acknowledged: bool,
    #[serde(default)]
    abandoned: bool,
    #[serde(default)]
    sent_log_keys: BTreeSet<String>,
    #[serde(default)]
    pending_logs: Vec<LogDelivery>,
    retired: bool,
    #[serde(default)]
    storage_operation: Option<crate::build_storage::VolumeOperation>,
    #[serde(default)]
    volume_checked: bool,
    #[serde(default)]
    volume_retired: bool,
    #[serde(default)]
    infrastructure_failure: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogDelivery {
    entry_key: String,
    phase: String,
    text: String,
}

fn compatible_requests(
    local: &KubernetesBuildRequest,
    remote: &KubernetesBuildRequest,
    unbound: bool,
) -> Result<bool> {
    let mut left = serde_json::to_value(local).map_err(|_| error("invalid_dispatch_receipt"))?;
    let mut right =
        serde_json::to_value(remote).map_err(|_| error("invalid_kubernetes_attempt"))?;
    left.as_object_mut()
        .ok_or_else(|| error("invalid_dispatch_receipt"))?
        .remove("volume");
    right
        .as_object_mut()
        .ok_or_else(|| error("invalid_kubernetes_attempt"))?
        .remove("volume");
    if left != right {
        return Ok(false);
    }
    match (&local.volume, &remote.volume) {
        (None, None) => Ok(true),
        (Some(a), Some(b)) => Ok(a
            .fingerprint()
            .map_err(|_| error("invalid_build_storage_handle"))?
            == b.fingerprint()
                .map_err(|_| error("invalid_build_storage_handle"))?),
        _ => Ok(unbound),
    }
}

/// One process owns one private state directory. Calls are serialized with `&mut
/// self`; a filesystem lock also excludes a second process using the same outbox.
pub struct KubernetesBuildDispatcher {
    _lock: std::fs::File,
    control: ControlClient,
    executor: KubernetesJobBuildExecutor,
    transfer: Arc<dyn KubernetesArtifactTransfer>,
    store: Arc<ArtifactStore>,
    outbox: Outbox,
    root: PathBuf,
    cluster_id: String,
    storage: Option<Arc<crate::build_storage::KubernetesBuildStorage>>,
}

impl KubernetesBuildDispatcher {
    pub fn new(
        control: ControlClient,
        executor: KubernetesJobBuildExecutor,
        transfer: Arc<dyn KubernetesArtifactTransfer>,
        store: impl Into<Arc<ArtifactStore>>,
        state_root: PathBuf,
        cluster_id: String,
    ) -> Result<Self> {
        crate::contracts::id(&cluster_id)?;
        let metadata = state_root.symlink_metadata().map_err(io)?;
        // SAFETY: geteuid has no preconditions or pointer arguments.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.mode() & 0o077 != 0 || metadata.uid() != owner {
            return Err(error("private_dispatch_state_required"));
        }
        let root = state_root.canonicalize().map_err(io)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join("dispatcher.lock"))
            .map_err(io)?;
        // SAFETY: the live File owns the descriptor and flock retains no pointer.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error("dispatcher_already_running"));
        }
        let outbox_root = root.join("completion-outbox");
        match std::fs::create_dir(&outbox_root) {
            Ok(()) => {
                std::fs::set_permissions(&outbox_root, std::fs::Permissions::from_mode(0o700))
                    .map_err(io)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e)),
        }
        let outbox = Outbox::open(outbox_root)?;
        Ok(Self {
            _lock: lock,
            control,
            executor,
            transfer,
            store: store.into(),
            outbox,
            root,
            cluster_id,
            storage: None,
        })
    }
    pub fn with_storage(mut self, storage: crate::build_storage::KubernetesBuildStorage) -> Self {
        self.storage = Some(Arc::new(storage));
        self
    }

    fn path(&self, request: &KubernetesBuildRequest) -> Result<PathBuf> {
        crate::contracts::id(&request.build_id)?;
        if request.generation == 0 || request.generation >= i32::MAX as u64 {
            return Err(error("invalid_generation"));
        }
        Ok(self.root.join(format!(
            "attempt-{}-{}.json",
            request.build_id, request.generation
        )))
    }

    fn save(&self, receipt: &DispatchReceipt) -> Result<()> {
        let bytes = serde_json::to_vec(receipt).map_err(|_| error("invalid_dispatch_receipt"))?;
        if bytes.len() > 400000 {
            return Err(error("dispatch_receipt_limit"));
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(io)?;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.as_file().sync_all().map_err(io)?;
        file.persist(self.path(&receipt.request)?)
            .map_err(|_| error("dispatch_receipt_unavailable"))?;
        std::fs::File::open(&self.root)
            .map_err(io)?
            .sync_all()
            .map_err(io)
    }

    fn read_receipt(path: &Path) -> Result<Option<DispatchReceipt>> {
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let metadata = file.metadata().map_err(io)?;
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.len() > 400000 {
            return Err(error("invalid_dispatch_receipt"));
        }
        let mut bytes = vec![];
        file.take(400001).read_to_end(&mut bytes).map_err(io)?;
        let receipt: DispatchReceipt =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid_dispatch_receipt"))?;
        if receipt.version != 1 {
            return Err(error("invalid_dispatch_receipt"));
        }
        Ok(Some(receipt))
    }

    fn new_receipt(&self, attempt: &DispatchAttempt, fingerprint: String) -> DispatchReceipt {
        DispatchReceipt {
            version: 1,
            cluster_id: self.cluster_id.clone(),
            runtime_fingerprint: self.executor.runtime_fingerprint(),
            request: attempt.request.clone(),
            container_build: attempt.container_build.clone(),
            spec_fingerprint: fingerprint,
            source_archive_sha256: None,
            uid: None,
            logs: String::new(),
            outcome: None,
            completion: None,
            acknowledged: false,
            abandoned: false,
            sent_log_keys: BTreeSet::new(),
            pending_logs: vec![],
            retired: false,
            storage_operation: None,
            volume_checked: false,
            volume_retired: false,
            infrastructure_failure: None,
        }
    }

    fn check_attempt(&self, attempt: &DispatchAttempt) -> Result<()> {
        let name = self
            .executor
            .job_name(&attempt.request)
            .map_err(kube_error)?;
        if self.storage.is_some() != attempt.storage_request.is_some()
            || attempt.cluster_id != self.cluster_id
            || attempt.namespace != self.executor.namespace()
            || attempt.job_name != name
            || attempt.runtime_fingerprint != self.executor.runtime_fingerprint()
        {
            return Err(error("kubernetes_attempt_conflict"));
        }
        Ok(())
    }

    async fn guarded<T>(
        &self,
        attempt: &DispatchAttempt,
        operation: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        self.control
            .guard_build(&attempt.request.build_id, attempt.generation())
            .await?;
        tokio::pin!(operation);
        loop {
            tokio::select! {
                result=&mut operation=>{self.control.guard_build(&attempt.request.build_id,attempt.generation()).await?;return result;},
                _=tokio::time::sleep(Duration::from_secs(2))=>{
                    if attempt.expired(){return Err(error("lease_changed"));}
                    self.control.guard_build(&attempt.request.build_id,attempt.generation()).await?;
                }
            }
        }
    }

    /// Reconcile durable attempts before admitting another build. The caller may
    /// poll this finite turn; no public worker command or enrollment is enabled.
    pub async fn turn(&mut self) -> Result<bool> {
        if self.storage.is_some() != self.executor.requires_storage() {
            return Err(error("build_storage_configuration_conflict"));
        }
        let pending_error = self.outbox.flush(&self.control).await.err();
        let response = match self
            .control
            .post("kubernetes-attempts", &json!({"clusterID":self.cluster_id}))
            .await
        {
            Ok(response) => response,
            Err(error) => {
                self.cancel_local().await?;
                return Err(error);
            }
        };
        let attempts = response["attempts"]
            .as_array()
            .ok_or_else(|| error("invalid_kubernetes_attempts"))?;
        if !attempts.is_empty() {
            let mut failure = None;
            for row in attempts {
                let result = async {
                    let attempt = DispatchAttempt::claim(row)?;
                    self.reconcile(&attempt).await
                }
                .await;
                if let Err(error) = result {
                    failure.get_or_insert(error);
                }
            }
            if let Some(error) = failure.or(pending_error.filter(|_| self.outbox.has_pending())) {
                return Err(error);
            }
            return Ok(true);
        }
        if let Some(error) = pending_error {
            return Err(error);
        }
        // Leave unclaimed work queued while the pinned build API is unavailable.
        // Active attempts above still reconcile and retain their uncertain capacity.
        self.executor.available().await.map_err(kube_error)?;
        let mut input = json!({"clusterID":self.cluster_id,"runtimeFingerprint":self.executor.runtime_fingerprint()});
        if self.storage.is_some() {
            input["storageProfile"] = json!("fixed-ext4-v1");
        }
        let claim = self.control.post("kubernetes-claim", &input).await?;
        if claim.is_null() {
            return Ok(false);
        }
        let attempt = DispatchAttempt::claim(&claim)?;
        self.check_attempt(&attempt)?;
        self.start_claim(&attempt).await?;
        Ok(true)
    }

    async fn start_claim(&self, attempt: &DispatchAttempt) -> Result<()> {
        if self.storage.is_some() {
            if Self::read_receipt(&self.path(&attempt.request)?)?.is_some() {
                return Err(error("kubernetes_attempt_reused"));
            }
            let mut receipt = self.new_receipt(attempt, String::new());
            self.save(&receipt)?;
            self.prepare_storage(attempt, &mut receipt).await?;
            let mut bound = attempt.clone();
            bound.request = receipt.request.clone();
            return self.launch_claim(&bound, &mut receipt).await;
        }
        let manifest = self
            .executor
            .manifest(&attempt.request)
            .map_err(kube_error)?;
        if manifest.metadata.namespace.as_deref() != Some(attempt.namespace.as_str()) {
            return Err(error("kubernetes_attempt_conflict"));
        }
        let fingerprint = self
            .executor
            .fingerprint(&attempt.request)
            .map_err(kube_error)?;
        if Self::read_receipt(&self.path(&attempt.request)?)?.is_some() {
            return Err(error("kubernetes_attempt_reused"));
        }
        let mut receipt = self.new_receipt(attempt, fingerprint);
        // This marker makes interrupted transfers non-retriable after restart.
        self.save(&receipt)?;
        self.launch_claim(attempt, &mut receipt).await
    }

    async fn launch_claim(
        &self,
        attempt: &DispatchAttempt,
        receipt: &mut DispatchReceipt,
    ) -> Result<()> {
        let fingerprint = self
            .executor
            .fingerprint(&attempt.request)
            .map_err(kube_error)?;
        if !receipt.spec_fingerprint.is_empty() && receipt.spec_fingerprint != fingerprint {
            return Err(error("kubernetes_attempt_conflict"));
        }
        receipt.spec_fingerprint = fingerprint;
        self.save(receipt)?;
        let prepared=async {
            let source=self.guarded(attempt,self.control.archive(&attempt.request.build_id,attempt.generation())).await?;
            Archive::read(&source,MAX_SOURCE)?;
            let hash=hex::encode(Sha256::digest(&source));
            receipt.source_archive_sha256=Some(hash.clone());self.save(receipt)?;
            let guest=self.execution_guest(attempt).await?;
            self.guarded(attempt,self.transfer.stage(&attempt.request,&guest,&attempt.container_build,&source,&hash)).await?;
            if attempt.lease_until-chrono::Utc::now().timestamp()<i64::from(attempt.request.limits.deadline_seconds)+60{return Err(error("lease_too_short"));}
            self.control.post("kubernetes-intent",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"specFingerprint":receipt.spec_fingerprint})).await?;
            self.control.guard_build(&attempt.request.build_id,attempt.generation()).await?;
            self.executor.start(&attempt.request).await.map_err(kube_error)
        }.await;
        let handle = match prepared {
            Ok(handle) => handle,
            Err(error) => {
                if error.code == "kubernetes_api_unavailable" {
                    receipt.infrastructure_failure = Some("KUBERNETES_API_UNAVAILABLE".into());
                }
                receipt.logs = format!(
                    "Preparation/create outcome requires reconciliation ({}).",
                    error.code
                );
                receipt.pending_logs.push(LogDelivery {
                    entry_key: "provisioning-error".into(),
                    phase: "provisioning".into(),
                    text: receipt.logs.clone(),
                });
                self.save(receipt)?;
                return Err(error); // persisted intent/marker is reconciled; never blindly rebuild
            }
        };
        receipt.uid = Some(handle.uid.clone());
        self.save(receipt)?;
        if let Err(error)=self.control.post("kubernetes-job",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"uid":handle.uid})).await {
            self.cancel_receipt(receipt,Some(&handle),error.code).await?;
            self.executor.cancel(&handle).await.map_err(kube_error)?;
            return Err(error);
        }
        self.bind_execution(attempt, receipt, &handle).await?;
        Ok(())
    }

    async fn execution_guest(&self, attempt: &DispatchAttempt) -> Result<GuestRequest> {
        let environment = self
            .guarded(
                attempt,
                self.control.post(
                    "kubernetes-environment",
                    &json!({"buildID":attempt.request.build_id,"generation":attempt.generation()}),
                ),
            )
            .await?;
        let mut guest = attempt.guest.clone();
        guest.build_environment =
            serde_json::from_value(environment).map_err(|_| error("invalid_build_environment"))?;
        guest.validate()?;
        Ok(guest)
    }

    async fn prepare_storage(
        &self,
        attempt: &DispatchAttempt,
        receipt: &mut DispatchReceipt,
    ) -> Result<()> {
        use webdock_agent_protocol::build_storage::{
            BuildStorageAction, BuildStorageReservation, BuildVolumePhase,
        };
        let storage = self
            .storage
            .as_ref()
            .ok_or_else(|| error("build_storage_configuration_conflict"))?;
        let expected = attempt
            .storage_request
            .as_ref()
            .ok_or_else(|| error("build_storage_scope_conflict"))?;
        loop {
            if receipt.request.volume.is_some()
                && receipt.volume_checked
                && receipt.storage_operation.is_none()
            {
                return Ok(());
            }
            if receipt.storage_operation.is_none() {
                let action = if receipt.request.volume.is_none() {
                    BuildStorageAction::Reserve
                } else {
                    BuildStorageAction::Inspect
                };
                let value=self.control.post("kubernetes-storage",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"action":action})).await?;
                let request: BuildStorageReservation = serde_json::from_value(value)
                    .map_err(|_| error("invalid_build_storage_authority"))?;
                if request
                    .allocation_fingerprint()
                    .map_err(|_| error("invalid_build_storage_authority"))?
                    != expected
                        .allocation_fingerprint()
                        .map_err(|_| error("invalid_build_storage_authority"))?
                {
                    return Err(error("build_storage_scope_conflict"));
                }
                receipt.storage_operation = Some(storage.prepare(request)?);
                self.save(receipt)?;
            }
            let mut operation = receipt
                .storage_operation
                .clone()
                .ok_or_else(|| error("build_storage_request_missing"))?;
            let reserve = operation.signed.reservation.action == BuildStorageAction::Reserve;
            let processed = if reserve {
                receipt.request.volume.is_some()
            } else {
                receipt.volume_checked
            };
            if !processed {
                self.guarded(attempt, storage.send(&mut operation)).await?;
                receipt.storage_operation = Some(operation.clone());
                self.save(receipt)?;
                let Some(reply) = self.guarded(attempt, storage.reply(&operation)).await? else {
                    return Err(error("build_storage_pending"));
                };
                if reply.failure_code.is_some() {
                    receipt.outcome = Some("storage_provisioning_failed".into());
                    self.save(receipt)?;
                    return Err(error("build_storage_failed"));
                }
                let result = reply
                    .result
                    .ok_or_else(|| error("invalid_build_storage_reply"))?;
                if result.phase != BuildVolumePhase::Ready {
                    return Err(error("build_storage_pending"));
                }
                let handle = result
                    .handle
                    .ok_or_else(|| error("invalid_build_storage_reply"))?;
                handle
                    .validate_for(expected)
                    .map_err(|_| error("build_storage_scope_conflict"))?;
                if let Some(existing) = &receipt.request.volume
                    && existing
                        .fingerprint()
                        .map_err(|_| error("invalid_build_storage_handle"))?
                        != handle
                            .fingerprint()
                            .map_err(|_| error("invalid_build_storage_handle"))?
                {
                    return Err(error("build_storage_scope_conflict"));
                }
                receipt.request.volume = Some(handle);
                if !reserve {
                    receipt.volume_checked = true;
                }
                self.save(receipt)?;
            }
            if reserve {
                self.control.post("kubernetes-volume",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"handle":receipt.request.volume})).await?;
            }
            storage.cleanup(&operation).await?;
            receipt.storage_operation = None;
            self.save(receipt)?;
        }
    }

    async fn bind_execution(
        &self,
        attempt: &DispatchAttempt,
        receipt: &mut DispatchReceipt,
        handle: &KubernetesJobHandle,
    ) -> Result<()> {
        if let Err(error) = self.transfer.bind(handle).await {
            self.cancel_receipt(receipt, Some(handle), "upload_authority_unavailable")
                .await?;
            self.executor.cancel(handle).await.map_err(kube_error)?;
            return Err(error);
        }
        if let Err(error) = self
            .control
            .guard_build(&attempt.request.build_id, attempt.generation())
            .await
        {
            self.cancel_receipt(receipt, Some(handle), error.code)
                .await?;
            self.executor.cancel(handle).await.map_err(kube_error)?;
            return Err(error);
        }
        self.executor.release(handle).await.map_err(kube_error)
    }

    async fn retire_storage(
        &self,
        attempt: &DispatchAttempt,
        receipt: &mut DispatchReceipt,
    ) -> Result<()> {
        use webdock_agent_protocol::build_storage::{
            BuildStorageAction, BuildStorageReservation, BuildVolumePhase,
        };
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        if receipt.volume_retired && receipt.storage_operation.is_none() {
            return Ok(());
        }
        if let Some(operation) = &receipt.storage_operation
            && (operation.signed.reservation.action != BuildStorageAction::Retire
                || (!receipt.volume_retired
                    && operation.signed.reservation.expires_at <= chrono::Utc::now()))
        {
            storage.cleanup(operation).await?;
            receipt.storage_operation = None;
            self.save(receipt)?;
        }
        if receipt.storage_operation.is_none() {
            let value=self.control.post("kubernetes-storage",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"action":"retire"})).await?;
            let request: BuildStorageReservation = serde_json::from_value(value)
                .map_err(|_| error("invalid_build_storage_authority"))?;
            let expected = attempt
                .storage_request
                .as_ref()
                .ok_or_else(|| error("build_storage_scope_conflict"))?;
            if request
                .allocation_fingerprint()
                .map_err(|_| error("invalid_build_storage_authority"))?
                != expected
                    .allocation_fingerprint()
                    .map_err(|_| error("invalid_build_storage_authority"))?
            {
                return Err(error("build_storage_scope_conflict"));
            }
            receipt.storage_operation = Some(storage.prepare(request)?);
            self.save(receipt)?;
        }
        let mut operation = receipt
            .storage_operation
            .clone()
            .ok_or_else(|| error("build_storage_request_missing"))?;
        if !receipt.volume_retired {
            storage.send(&mut operation).await?;
            receipt.storage_operation = Some(operation.clone());
            self.save(receipt)?;
            let Some(reply) = storage.reply(&operation).await? else {
                return Err(error("build_storage_pending"));
            };
            if reply.failure_code
                == Some(webdock_agent_protocol::build_storage::BuildVolumeFailure::Expired)
            {
                storage.cleanup(&operation).await?;
                receipt.storage_operation = None;
                self.save(receipt)?;
                return Err(error("build_storage_pending"));
            }
            if reply.failure_code.is_some() {
                return Err(error("build_storage_retirement_failed"));
            }
            if reply
                .result
                .as_ref()
                .is_none_or(|r| r.phase != BuildVolumePhase::Retired)
            {
                return Err(error("build_storage_pending"));
            }
            receipt.volume_retired = true;
            self.save(receipt)?;
        }
        // Physical proof precedes Core acknowledgment; response loss is replayed.
        self.control.post("kubernetes-volume-retired",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"epoch":operation.signed.reservation.epoch})).await?;
        storage.cleanup(&operation).await?;
        receipt.storage_operation = None;
        self.save(receipt)
    }

    async fn reconcile(&self, attempt: &DispatchAttempt) -> Result<()> {
        self.check_attempt(attempt)?;
        let mut receipt = Self::read_receipt(&self.path(&attempt.request)?)?;
        if let Some(value) = &receipt
            && (value.cluster_id != self.cluster_id
                || value.runtime_fingerprint != self.executor.runtime_fingerprint()
                || !compatible_requests(
                    &value.request,
                    &attempt.request,
                    attempt.spec_fingerprint.is_none() && value.spec_fingerprint.is_empty(),
                )?
                || serde_json::to_value(&value.container_build)
                    .map_err(|_| error("invalid_dispatch_receipt"))?
                    != serde_json::to_value(&attempt.container_build)
                        .map_err(|_| error("invalid_container_settings"))?
                || attempt
                    .spec_fingerprint
                    .as_ref()
                    .is_some_and(|fp| fp != &value.spec_fingerprint))
        {
            return Err(error("kubernetes_attempt_conflict"));
        }
        let fingerprint = attempt.spec_fingerprint.clone();
        let handle = if let Some(fp) = &fingerprint {
            self.executor
                .observe(&attempt.request, fp)
                .await
                .map_err(kube_error)?
        } else {
            if !self
                .executor
                .absent(&attempt.request, attempt.uid.as_deref())
                .await
                .map_err(kube_error)?
            {
                return Err(error("kubernetes_intent_missing"));
            }
            None
        };
        let mut receipt = receipt
            .take()
            .unwrap_or_else(|| self.new_receipt(attempt, fingerprint.unwrap_or_default()));
        if let Some(handle) = &handle {
            if receipt.outcome.is_none() {
                receipt.infrastructure_failure = None;
            }
            if handle.namespace != attempt.namespace
                || attempt.uid.as_ref().is_some_and(|uid| uid != &handle.uid)
                || receipt.uid.as_ref().is_some_and(|uid| uid != &handle.uid)
            {
                return Err(error("kubernetes_attempt_conflict"));
            }
            receipt.uid = Some(handle.uid.clone());
            self.save(&receipt)?;
        }
        if receipt.outcome.is_some() {
            return self.finish(attempt, &mut receipt, handle.as_ref()).await;
        }
        if attempt.cancel_requested || attempt.expired() {
            self.cancel_receipt(&mut receipt, handle.as_ref(), "attempt_cancelled")
                .await?;
            return self.finish(attempt, &mut receipt, handle.as_ref()).await;
        }
        if let Err(authority) = self
            .control
            .guard_build(&attempt.request.build_id, attempt.generation())
            .await
        {
            self.cancel_receipt(&mut receipt, handle.as_ref(), authority.code)
                .await?;
            self.finish(attempt, &mut receipt, handle.as_ref()).await?;
            return Ok(());
        }
        if self.storage.is_some() && receipt.spec_fingerprint.is_empty() {
            self.prepare_storage(attempt, &mut receipt).await?;
            let mut bound = attempt.clone();
            bound.request = receipt.request.clone();
            return self.launch_claim(&bound, &mut receipt).await;
        }
        // Preparation can fail before a Job UID exists. Preserve its safe error
        // event durably before retiring that otherwise absent execution.
        self.flush_logs(attempt, &mut receipt).await?;
        let Some(handle) = handle else {
            receipt.outcome = Some("execution_missing".into());
            append_summary(
                &mut receipt.logs,
                "Execution was absent during recovery; this attempt was not restarted.",
            );
            if attempt.uid.is_some() {
                receipt.completion = Some(failed(attempt, &receipt.logs));
            }
            self.save(&receipt)?;
            return self.finish(attempt, &mut receipt, None).await;
        };
        if attempt.uid.is_none() {
            if let Err(error)=self.control.post("kubernetes-job",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"uid":handle.uid})).await {
                self.cancel_receipt(&mut receipt,Some(&handle),error.code).await?;
                self.executor.cancel(&handle).await.map_err(kube_error)?;
                return Err(error);
            }
            // Wait for a fresh durable row before accepting source/artifact callbacks.
            return Ok(());
        }
        self.bind_execution(attempt, &mut receipt, &handle).await?;
        let mut execution = attempt.clone();
        execution.guest = match self.execution_guest(attempt).await {
            Ok(guest) => guest,
            Err(failure) if authority_lost(failure.code) || completion_fenced(failure.code) => {
                self.cancel_receipt(&mut receipt, Some(&handle), failure.code)
                    .await?;
                return self.finish(attempt, &mut receipt, Some(&handle)).await;
            }
            Err(failure) => return Err(failure),
        };
        let attempt = &execution;
        if let Err(error) = self.capture_logs(attempt, &handle, &mut receipt).await {
            if completion_fenced(error.code)
                || matches!(
                    error.code,
                    "build_log_limit" | "build_log_event_limit" | "invalid_build_log"
                )
            {
                self.cancel_receipt(&mut receipt, Some(&handle), error.code)
                    .await?;
                return self.finish(attempt, &mut receipt, Some(&handle)).await;
            }
            return Err(error);
        }
        match self.executor.inspect(&handle).await.map_err(kube_error)? {
            Some(KubernetesJobPhase::Pending | KubernetesJobPhase::Running) => Ok(()),
            Some(KubernetesJobPhase::SucceededAwaitingRegistryVerification) => {
                let result = self.guarded(attempt, self.transfer.collect(&handle)).await;
                if let Err(error) = &result
                    && authority_lost(error.code)
                {
                    self.cancel_receipt(&mut receipt, Some(&handle), error.code)
                        .await?;
                    return self.finish(attempt, &mut receipt, Some(&handle)).await;
                }
                match result
                    .and_then(|output| self.retain_artifact(attempt, &handle, &receipt, output))
                {
                    Ok(body) => {
                        receipt.logs = body["logs"].as_str().unwrap_or_default().into();
                        receipt.completion = Some(body);
                        receipt.outcome = Some("artifact_retained".into());
                    }
                    Err(error) => {
                        receipt.logs = attempt
                            .guest
                            .redact(&format!("{}\n{}", error.code, receipt.logs));
                        receipt.completion = Some(failed(attempt, &receipt.logs));
                        receipt.outcome = Some("result_rejected".into());
                    }
                }
                self.save(&receipt)?;
                self.finish(attempt, &mut receipt, Some(&handle)).await
            }
            Some(KubernetesJobPhase::Failed) => {
                let code = self
                    .executor
                    .failure_code(&handle)
                    .await
                    .map_err(kube_error)?;
                if code == "KUBERNETES_NODE_UNAVAILABLE" {
                    receipt.infrastructure_failure = Some(code.into());
                    receipt.completion = None;
                } else {
                    let mut completion = failed(attempt, &receipt.logs);
                    completion["failureCode"] = json!(code);
                    receipt.completion = Some(completion);
                }
                receipt.outcome = Some("execution_failed".into());
                self.save(&receipt)?;
                self.finish(attempt, &mut receipt, Some(&handle)).await
            }
            Some(KubernetesJobPhase::Deleting) | None => {
                self.cancel_receipt(&mut receipt, Some(&handle), "execution_deleted")
                    .await?;
                self.finish(attempt, &mut receipt, Some(&handle)).await
            }
        }
    }

    fn retain_artifact(
        &self,
        attempt: &DispatchAttempt,
        handle: &KubernetesJobHandle,
        receipt: &DispatchReceipt,
        output: KubernetesTransferredOutput,
    ) -> Result<Value> {
        if output.build_id != attempt.request.build_id
            || output.generation != attempt.request.generation
            || output.job_uid != handle.uid
            || output.source_sha != attempt.request.commit_sha
            || receipt.source_archive_sha256.as_deref()
                != Some(output.source_archive_sha256.as_str())
        {
            return Err(error("kubernetes_result_conflict"));
        }
        validate_retained_image_size(output.image_tar.len() as u64)?;
        validate_oci(&output.image_tar)?;
        let envelope =
            Archive::from_files(BTreeMap::from([("image.tar".into(), output.image_tar)]))?
                .encode()?;
        let artifact = self.store.put(
            &attempt.request.customer_id,
            &attempt.request.build_id,
            attempt.generation(),
            &envelope,
        )?;
        Ok(
            json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"status":"succeeded","logs":attempt.guest.redact(&output.logs),
            "artifact":{"kind":"oci","digest":artifact.digest(),"storageKey":artifact.storage_key(),"sizeBytes":artifact.size(),"imageDigest":artifact.image_digest(),"remoteStorage":artifact.remote_storage()}}),
        )
    }

    async fn capture_logs(
        &self,
        attempt: &DispatchAttempt,
        handle: &KubernetesJobHandle,
        receipt: &mut DispatchReceipt,
    ) -> Result<()> {
        self.flush_logs(attempt, receipt).await?;
        if let Some(logs) = self
            .executor
            .logs(
                handle,
                KubernetesLogBounds {
                    max_bytes: 262144,
                    tail_lines: 10000,
                },
            )
            .await
            .map_err(kube_error)?
        {
            if logs.truncated {
                return Err(error("build_log_limit"));
            }
            for entry in log_deliveries(&handle.uid, &logs.bytes, Some(&attempt.guest))? {
                if receipt.sent_log_keys.contains(&entry.entry_key) {
                    continue;
                }
                if receipt.sent_log_keys.len() >= 4096 {
                    return Err(error("build_log_limit"));
                }
                append_summary(&mut receipt.logs, &entry.text);
                receipt.pending_logs.push(entry);
                self.save(receipt)?;
                self.flush_logs(attempt, receipt).await?;
            }
        }
        Ok(())
    }

    async fn flush_logs(
        &self,
        attempt: &DispatchAttempt,
        receipt: &mut DispatchReceipt,
    ) -> Result<()> {
        while let Some(entry) = receipt.pending_logs.first().cloned() {
            self.control.post("kubernetes-log",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"entryKey":entry.entry_key,"phase":entry.phase,"text":entry.text})).await?;
            receipt.sent_log_keys.insert(entry.entry_key);
            receipt.pending_logs.remove(0);
            self.save(receipt)?;
        }
        Ok(())
    }

    async fn cancel_receipt(
        &self,
        receipt: &mut DispatchReceipt,
        _handle: Option<&KubernetesJobHandle>,
        reason: &str,
    ) -> Result<()> {
        self.transfer.revoke(&receipt.request).await?;
        // Revoked authority cannot hydrate the old secret set safely. Keep the already
        // redacted durable tail instead of capturing new untrusted output after closure.
        receipt.outcome = Some(reason.into());
        self.save(receipt)
    }

    async fn finish(
        &self,
        attempt: &DispatchAttempt,
        receipt: &mut DispatchReceipt,
        handle: Option<&KubernetesJobHandle>,
    ) -> Result<()> {
        if receipt.outcome.is_none() {
            return Err(error("kubernetes_outcome_missing"));
        }
        self.transfer.revoke(&receipt.request).await?;
        if let Some(body) = &receipt.completion
            && !receipt.acknowledged
            && !receipt.abandoned
        {
            if attempt.cancel_requested || attempt.expired() {
                receipt.abandoned = true;
                self.save(receipt)?;
                self.outbox.discard_exact("complete", body)?;
            } else {
                let sent = async {
                    if self.outbox.has_pending() {
                        return Err(error("pending_completion"));
                    }
                    self.outbox.put("complete", body.clone())?;
                    self.outbox.flush(&self.control).await?;
                    Ok(())
                }
                .await;
                if let Err(error) = sent {
                    // The full outcome is already fsynced. Cleanup must still work
                    // when completion is fenced or its response was lost.
                    if let Some(handle) = handle {
                        self.executor.cancel(handle).await.map_err(kube_error)?;
                    }
                    if completion_fenced(error.code) {
                        receipt.abandoned = true;
                        self.save(receipt)?;
                        self.outbox.discard_exact("complete", body)?;
                    } else {
                        return Err(error);
                    }
                } else {
                    receipt.acknowledged = true;
                    self.save(receipt)?;
                }
            }
        }
        if let Some(handle) = handle {
            self.executor.cancel(handle).await.map_err(kube_error)?;
        }
        if !self
            .executor
            .absent(
                &receipt.request,
                receipt.uid.as_deref().or(attempt.uid.as_deref()),
            )
            .await
            .map_err(kube_error)?
        {
            return Ok(());
        }
        self.retire_storage(attempt, receipt).await?;
        if self.storage.is_none() || !receipt.spec_fingerprint.is_empty() {
            self.transfer.cleanup(&receipt.request).await?;
        }
        self.control.post("kubernetes-retire",&json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"uid":attempt.uid,"infrastructureFailure":receipt.infrastructure_failure})).await?;
        receipt.retired = true;
        self.save(receipt)
    }

    /// Stop exact locally journaled attempts on shutdown, revocation or a fatal companion task.
    /// Revokes transfer authority before cancellation; retains volumes and receipts for reconciliation.
    pub async fn cancel_local_on_shutdown(&self) -> Result<()> {
        self.cancel_local().await
    }
    async fn cancel_local(&self) -> Result<()> {
        for entry in std::fs::read_dir(&self.root).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("attempt-") || !name.ends_with(".json") {
                continue;
            }
            let Some(mut receipt) = Self::read_receipt(&entry.path())? else {
                continue;
            };
            if receipt.retired {
                continue;
            }
            self.transfer.revoke(&receipt.request).await?;
            if receipt.cluster_id != self.cluster_id
                || receipt.runtime_fingerprint != self.executor.runtime_fingerprint()
            {
                return Err(error("kubernetes_attempt_conflict"));
            }
            if receipt.spec_fingerprint.is_empty() {
                continue;
            }
            let handle = self
                .executor
                .observe(&receipt.request, &receipt.spec_fingerprint)
                .await
                .map_err(kube_error)?;
            if let Some(handle) = handle {
                if receipt.uid.as_ref().is_some_and(|uid| uid != &handle.uid) {
                    return Err(error("kubernetes_attempt_conflict"));
                }
                receipt.uid = Some(handle.uid.clone());
                self.cancel_receipt(&mut receipt, Some(&handle), "control_unavailable")
                    .await?;
                self.executor.cancel(&handle).await.map_err(kube_error)?;
            }
        }
        Ok(())
    }
}

fn kube_error(failure: crate::kubernetes::KubernetesBuildError) -> crate::Error {
    use crate::kubernetes::KubernetesBuildError;
    error(match failure {
        KubernetesBuildError::IdentityConflict => "kubernetes_identity_conflict",
        KubernetesBuildError::InvalidConfig => "invalid_kubernetes_configuration",
        KubernetesBuildError::InvalidRequest => "invalid_kubernetes_request",
        KubernetesBuildError::ProjectNotTrusted => "kubernetes_project_not_trusted",
        KubernetesBuildError::Timeout => "kubernetes_request_uncertain",
        KubernetesBuildError::Api(kube::Error::Api(response))
            if (500..=599).contains(&response.code) =>
        {
            "kubernetes_api_unavailable"
        }
        KubernetesBuildError::Api(_)
        | KubernetesBuildError::Manifest(_)
        | KubernetesBuildError::Logs(_) => "kubernetes_unavailable",
    })
}

fn failed(attempt: &DispatchAttempt, logs: &str) -> Value {
    json!({"buildID":attempt.request.build_id,"generation":attempt.generation(),"status":"failed","failureCode":"BUILD_FAILED","logs":attempt.guest.redact(logs)})
}

fn authority_lost(code: &str) -> bool {
    matches!(
        code,
        "worker_revoked"
            | "lease_changed"
            | "build_lease_lost"
            | "kubernetes_attempt_lost"
            | "control_request_failed"
            | "control_unavailable"
    )
}

fn completion_fenced(code: &str) -> bool {
    matches!(
        code,
        "worker_revoked" | "lease_changed" | "build_lease_lost" | "kubernetes_attempt_lost"
    )
}

fn append_summary(summary: &mut String, text: &str) {
    summary.push_str(text);
    let mut end = summary.len().min(crate::execution::MAX_LOG);
    while !summary.is_char_boundary(end) {
        end -= 1;
    }
    summary.truncate(end);
}

fn log_deliveries(
    uid: &str,
    bytes: &[u8],
    guest: Option<&GuestRequest>,
) -> Result<Vec<LogDelivery>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Event {
        phase: String,
        logs: String,
    }
    let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
        return Ok(vec![]);
    };
    let mut result = vec![];
    for (index, line) in bytes[..=end].split(|b| *b == b'\n').enumerate() {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let (identity, payload) = if line.starts_with('{') {
            (format!("line-{index}"), line)
        } else if let Some((stamp, payload)) = line.split_once(' ') {
            if chrono::DateTime::parse_from_rfc3339(stamp).is_err() {
                continue;
            }
            (stamp.into(), payload)
        } else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Event>(payload) else {
            continue;
        };
        let phase = match event.phase.as_str() {
            "source" | "checkout" => "cloning",
            "strategy" | "prepare" => "preparing",
            "build" => "building",
            "export" | "complete" => "pushing",
            _ => continue,
        };
        if event.logs.len() > 24000 {
            return Err(error("build_log_event_limit"));
        }
        let text = guest.map_or_else(|| event.logs.clone(), |g| g.redact(&event.logs));
        let mut tail = text.as_str();
        let mut chunk = 0;
        while !tail.is_empty() {
            let mut end = tail.len().min(8192);
            while !tail.is_char_boundary(end) {
                end -= 1;
            }
            let text = tail[..end].to_owned();
            let bytes = serde_json::to_vec(&(uid, &identity, phase, chunk, &text))
                .map_err(|_| error("invalid_build_log"))?;
            result.push(LogDelivery {
                entry_key: format!("log-{}", hex::encode(Sha256::digest(bytes))),
                phase: phase.into(),
                text,
            });
            tail = &tail[end..];
            chunk += 1;
        }
    }
    Ok(result)
}

#[cfg(all(test, feature = "local-registry-test"))]
mod tests {
    #[test]
    fn durable_cache_budget_is_restored_and_conflicting_source_resources_are_rejected() {
        let mut value = serde_json::json!({"generation":1,"buildID":"500","customerID":"100","projectID":"300","clusterID":"400","namespace":"webdock-builds",
            "sourceSHA":"a".repeat(40),"leaseUntil":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339(),
            "request":{"recipe":"dockerfile","rootDirectory":".","limits":{"cpu":2,"memoryMiB":4096,"diskBytes":10737418240_u64,"durationSeconds":1500}},
            "kubernetesLimits":{"cpu_millis":2000,"memory_mib":4096,"workspace_mib":10240,"cache_mib":512,"deadline_seconds":1500}});
        assert_eq!(
            super::DispatchAttempt::claim(&value)
                .unwrap()
                .request
                .limits
                .cache_mib,
            512,
            "restart must use the database budget rather than a worker default"
        );
        value["kubernetesLimits"]["memory_mib"] = serde_json::json!(8192);
        assert!(
            super::DispatchAttempt::claim(&value).is_err(),
            "resource snapshot must agree with original source execution limits"
        );
    }
    use super::*;
    use crate::artifact::MAX_OUTPUT;
    use crate::kubernetes::{KubernetesBuildConfig, KubernetesBuildProfile, TrustedProject};
    use axum::{
        Json, Router,
        body::Body as AxumBody,
        extract::{Request, State},
        http::StatusCode,
        response::IntoResponse,
        routing::any,
    };
    use http_body_util::{BodyExt, Full};
    use kube::{Client, client::Body};
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };
    use tower::service_fn;

    struct Remote {
        storage: Option<StorageRemote>,
        claim: Value,
        attempts: Vec<Value>,
        events: Vec<String>,
        job: Option<Value>,
        creates: usize,
        uncertain_create: bool,
        api_rejected_create: bool,
        api_offline: bool,
        uncertain_stage: bool,
        lost_completion_response: bool,
        wrong_source_digest: bool,
        revoke_on_bind: bool,
        upload_bind_fails: bool,
        revoke_on_collect: bool,
        staged_strategy: Option<String>,
        log_tail: Option<Vec<u8>>,
        delivered_logs: Vec<Value>,
        lost_log_response: bool,
        pods_remain: bool,
        completed: Vec<Value>,
        active: bool,
    }
    struct StorageRemote {
        snapshot: webdock_agent_protocol::build_storage::BuildStorageReservation,
        epoch: u64,
        maps: BTreeMap<String, Value>,
        handle: Option<Value>,
        held: bool,
        lost_bind: bool,
        counter: u32,
        retired: bool,
        retire_ttl_seconds: i64,
    }
    fn storage_http(s: &mut Remote, method: &str, path: &str, mut body: Value) -> (u16, Value) {
        use webdock_agent_protocol::build_storage::{
            BuildStorageAction, BuildVolumeHandle, BuildVolumePhase, BuildVolumeReply,
            BuildVolumeResult, SignedBuildStorageReservation,
        };
        let absent = || {
            (
                404,
                json!({"kind":"Status","apiVersion":"v1","code":404,"reason":"NotFound","message":"owned absence"}),
            )
        };
        let no_consumers = s.job.is_none() && !s.pods_remain;
        let h = s.storage.as_mut().unwrap();
        if path.contains("/persistentvolumeclaims/") {
            let Some(handle) = &h.handle else {
                return absent();
            };
            let handle: BuildVolumeHandle = serde_json::from_value(handle.clone()).unwrap();
            let r = &handle.reservation;
            return (
                200,
                json!({"kind":"PersistentVolumeClaim","apiVersion":"v1","metadata":{"name":handle.pvc_name,"namespace":handle.namespace,"uid":handle.pvc_uid,"resourceVersion":"1",
                "labels":{"app.kubernetes.io/managed-by":"webdock-build-storage","webdock.dev/customer-id":r.customer_id,"webdock.dev/project-id":r.project_id,"webdock.dev/build-id":r.build_id,"webdock.dev/generation":r.generation.to_string(),"webdock.dev/volume-id":r.volume_id},
                "annotations":{"webdock.dev/allocation-sha256":r.allocation_fingerprint().unwrap()}},"spec":{"volumeName":handle.pv_name,"accessModes":["ReadWriteOnce"],"volumeMode":"Filesystem","storageClassName":"","resources":{"requests":{"storage":r.volume_bytes.to_string()}}},"status":{"phase":"Bound"}}),
            );
        }
        if method == "POST" {
            h.counter += 1;
            body["metadata"]["uid"] = json!(format!("request-{}", h.counter));
            body["metadata"]["resourceVersion"] = json!("1");
            let key = format!("{path}/{}", body["metadata"]["name"].as_str().unwrap());
            h.maps.insert(key, body.clone());
            return (201, body);
        }
        if method == "DELETE" {
            let object = h.maps.get(path).unwrap();
            assert_eq!(body["preconditions"]["uid"], object["metadata"]["uid"]);
            h.maps.remove(path);
            return (
                200,
                json!({"kind":"Status","apiVersion":"v1","code":200,"status":"Success"}),
            );
        }
        if path.contains("storage-receipts") && !h.maps.contains_key(path) && !h.held {
            let request_path = path.replace("storage-receipts", "storage-requests");
            let Some(request) = h.maps.get(&request_path).cloned() else {
                return absent();
            };
            let signed: SignedBuildStorageReservation =
                serde_json::from_str(request["data"]["request.json"].as_str().unwrap()).unwrap();
            signed
                .verify(
                    &ed25519_dalek::SigningKey::from_bytes(&[6; 32]).verifying_key(),
                    &h.snapshot.target,
                    chrono::Utc::now(),
                )
                .unwrap();
            let r = &signed.reservation;
            let result = if r.action == BuildStorageAction::Retire {
                if !no_consumers {
                    return absent();
                }
                h.retired = true;
                BuildVolumeResult {
                    phase: BuildVolumePhase::Retired,
                    handle: h.handle.clone().map(|v| serde_json::from_value(v).unwrap()),
                }
            } else {
                if h.handle.is_none() {
                    h.handle = Some(
                        serde_json::to_value(
                            BuildVolumeHandle::new(
                                r.clone(),
                                "webdock-build-qualification",
                                "pv-owned",
                                "pvc-owned",
                                "123e4567-e89b-42d3-a456-426614174001",
                            )
                            .unwrap(),
                        )
                        .unwrap(),
                    );
                }
                BuildVolumeResult {
                    phase: BuildVolumePhase::Ready,
                    handle: Some(serde_json::from_value(h.handle.clone().unwrap()).unwrap()),
                }
            };
            let reply = BuildVolumeReply {
                version: 1,
                request_uid: request["metadata"]["uid"].as_str().unwrap().into(),
                request_fingerprint: r.fingerprint().unwrap(),
                result: Some(result),
                failure_code: None,
            };
            let object = json!({"kind":"ConfigMap","apiVersion":"v1","metadata":{"name":r.queue_name().unwrap(),"namespace":"webdock-build-storage-receipts","uid":format!("reply-{}",h.counter),"resourceVersion":"1","annotations":{"webdock.dev/request-sha256":r.fingerprint().unwrap()}},"immutable":true,"data":{"reply.json":serde_json::to_string(&reply).unwrap()}});
            h.maps.insert(path.into(), object);
        }
        h.maps
            .get(path)
            .cloned()
            .map(|v| (200, v))
            .unwrap_or_else(absent)
    }
    fn claim() -> Value {
        json!({"buildID":"200","customerID":"100","projectID":"300","generation":1,"attempt":1,
            "clusterID":"400","namespace":"webdock-builds","sourceSHA":"a".repeat(40),
            "leaseUntil":(chrono::Utc::now()+chrono::Duration::minutes(27)).to_rfc3339(),
            "request":{"recipe":"dockerfile","rootDirectory":".","buildEnvironment":{},
                "limits":{"cpu":2,"memoryMiB":4096,"diskBytes":10737418240u64,"durationSeconds":1500}}})
    }
    #[test]
    fn absent_lease_is_cleanup_authority_only_for_explicitly_cancelled_attempts() {
        let mut value = claim();
        value["leaseUntil"] = Value::Null;
        assert!(DispatchAttempt::claim(&value).is_err());
        value["buildStatus"] = json!("running");
        value["cancelRequested"] = json!(false);
        assert!(DispatchAttempt::claim(&value).is_err());
        value["cancelRequested"] = json!(true);
        let attempt = DispatchAttempt::claim(&value).unwrap();
        assert!(attempt.cancel_requested && attempt.expired());
        value["leaseUntil"] = json!("not-a-timestamp");
        assert!(DispatchAttempt::claim(&value).is_err());
    }
    fn source() -> Vec<u8> {
        Archive::from_files(BTreeMap::from([(
            "Dockerfile".into(),
            b"FROM scratch\n".to_vec(),
        )]))
        .unwrap()
        .encode()
        .unwrap()
    }
    fn pod(job: &Value) -> Value {
        let mut pod = json!({"apiVersion":"v1","kind":"Pod","metadata":job["spec"]["template"]["metadata"],"spec":job["spec"]["template"]["spec"]});
        pod["metadata"]["name"] = json!("runner-pod");
        pod["metadata"]["namespace"] = job["metadata"]["namespace"].clone();
        pod["metadata"]["uid"] = json!("pod-one");
        pod["metadata"]["ownerReferences"] = json!([{"apiVersion":"batch/v1","kind":"Job","name":job["metadata"]["name"],"uid":"uid-one","controller":true}]);
        pod
    }
    async fn control(
        State(state): State<Arc<Mutex<Remote>>>,
        request: Request,
    ) -> axum::response::Response {
        let path = request.uri().path().to_owned();
        let bytes = axum::body::to_bytes(request.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        let mut s = state.lock().unwrap();
        if path.ends_with("/archive") {
            s.events.push("archive".into());
            return AxumBody::from(source()).into_response();
        }
        if path.ends_with("/lease") {
            return if s.active {
                Json(json!({"active":true})).into_response()
            } else {
                (StatusCode::CONFLICT, Json(json!({"error":"lease_changed"}))).into_response()
            };
        }
        let result = match path.as_str() {
            "/kubernetes-attempts" => json!({"attempts":s.attempts}),
            "/kubernetes-claim" => {
                s.events.push("claim".into());
                let claim = s.claim.take();
                if !claim.is_null() {
                    s.attempts.push(json!({"build_id":"200","customer_id":"100","project_id":"300","generation":1,
                        "cluster_id":"400","namespace":claim["namespace"],"job_name":claim["jobName"],"runtime_fingerprint":claim["runtimeFingerprint"],
                        "spec_fingerprint":null,"job_uid":null,"sourceSHA":claim["sourceSHA"],"request":claim["request"],
                        "containerBuild":claim["containerBuild"],"kubernetes_limits":claim["kubernetesLimits"],"storage_request":claim["storageRequest"],"leaseUntil":claim["leaseUntil"],"cancelRequested":false,"buildStatus":"running"}));
                }
                claim
            }
            "/kubernetes-environment" => {
                s.events.push("environment-handoff".into());
                json!({"FIXTURE_SECRET":"owned-private-fixture"})
            }
            "/kubernetes-storage" => {
                let storage = s.storage.as_mut().unwrap();
                storage.epoch += 1;
                let mut request = storage.snapshot.clone();
                request.epoch = storage.epoch;
                request.action = serde_json::from_value(body["action"].clone()).unwrap();
                request.expires_at = chrono::Utc::now()
                    + chrono::Duration::seconds(
                        if request.action
                            == webdock_agent_protocol::build_storage::BuildStorageAction::Retire
                        {
                            storage.retire_ttl_seconds
                        } else {
                            240
                        },
                    );
                serde_json::to_value(request).unwrap()
            }
            "/kubernetes-volume" => {
                s.events.push("volume-bind".into());
                s.attempts[0]["volume_handle"] = body["handle"].clone();
                if s.storage.as_ref().unwrap().lost_bind {
                    s.storage.as_mut().unwrap().lost_bind = false;
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":"response_lost"})),
                    )
                        .into_response();
                }
                json!({"recorded":true})
            }
            "/kubernetes-volume-retired" => {
                assert!(s.job.is_none() && !s.pods_remain && s.storage.as_ref().unwrap().retired);
                s.events.push("volume-retired".into());
                json!({"retired":true})
            }
            "/kubernetes-intent" => {
                s.events.push("intent".into());
                s.attempts[0]["spec_fingerprint"] = body["specFingerprint"].clone();
                json!({"recorded":true})
            }
            "/kubernetes-job" => {
                if s.revoke_on_bind {
                    s.active = false;
                    return (StatusCode::CONFLICT, Json(json!({"error":"lease_changed"})))
                        .into_response();
                }
                s.events.push("bind".into());
                s.attempts[0]["job_uid"] = body["uid"].clone();
                json!({"recorded":true})
            }
            "/complete" => {
                if !s.active {
                    return (StatusCode::CONFLICT, Json(json!({"error":"lease_changed"})))
                        .into_response();
                }
                s.events.push("complete".into());
                s.completed.push(body);
                s.attempts[0]["buildStatus"] = json!("artifact-ready");
                if s.lost_completion_response {
                    s.lost_completion_response = false;
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":"response_lost"})),
                    )
                        .into_response();
                }
                json!({"status":"artifact-ready"})
            }
            "/kubernetes-retire" => {
                s.events.push("retire".into());
                if !body["infrastructureFailure"].is_null() {
                    assert_eq!(body["infrastructureFailure"], "KUBERNETES_API_UNAVAILABLE");
                    s.events.push("retry-api".into());
                }
                assert!(s.job.is_none());
                assert!(!s.pods_remain);
                if s.storage.is_some() {
                    assert!(s.events.contains(&"volume-retired".into()));
                }
                s.attempts.clear();
                json!({"retired":true})
            }
            "/kubernetes-log" => {
                s.delivered_logs.push(body);
                s.events.push("log".into());
                if s.lost_log_response {
                    s.lost_log_response = false;
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":"response_lost"})),
                    )
                        .into_response();
                }
                json!({"appended":true})
            }
            _ => panic!("unexpected control path {path}"),
        };
        Json(result).into_response()
    }
    struct Transfer {
        state: Arc<Mutex<Remote>>,
        invalid: bool,
    }
    #[async_trait::async_trait]
    impl KubernetesArtifactTransfer for Transfer {
        async fn bind(&self, _handle: &KubernetesJobHandle) -> Result<()> {
            let mut s = self.state.lock().unwrap();
            s.events.push("upload-bind".into());
            if s.upload_bind_fails {
                return Err(error("upload_grant_missing"));
            }
            Ok(())
        }
        async fn revoke(&self, _request: &KubernetesBuildRequest) -> Result<()> {
            self.state
                .lock()
                .unwrap()
                .events
                .push("upload-revoke".into());
            Ok(())
        }
        async fn cleanup(&self, _request: &KubernetesBuildRequest) -> Result<()> {
            let mut s = self.state.lock().unwrap();
            assert!(s.job.is_none());
            assert!(!s.pods_remain);
            s.events.push("cleanup".into());
            Ok(())
        }
        async fn stage(
            &self,
            request: &KubernetesBuildRequest,
            guest: &GuestRequest,
            settings: &crate::contracts::ContainerSettings,
            source: &[u8],
            sha256: &str,
        ) -> Result<()> {
            assert_eq!(
                guest.build_environment["FIXTURE_SECRET"],
                "owned-private-fixture"
            );
            assert_eq!(request.commit_sha, "a".repeat(40));
            assert_eq!(hex::encode(Sha256::digest(source)), sha256);
            self.state.lock().unwrap().events.push("stage".into());
            self.state.lock().unwrap().staged_strategy = Some(settings.strategy.clone());
            if self.state.lock().unwrap().uncertain_stage {
                return Err(error("transfer_uncertain"));
            }
            Ok(())
        }
        async fn collect(
            &self,
            handle: &KubernetesJobHandle,
        ) -> Result<KubernetesTransferredOutput> {
            self.state.lock().unwrap().events.push("collect".into());
            {
                let mut s = self.state.lock().unwrap();
                if s.revoke_on_collect {
                    s.active = false;
                }
            }
            Ok(KubernetesTransferredOutput {
                build_id: handle.request.build_id.clone(),
                generation: handle.request.generation,
                job_uid: handle.uid.clone(),
                source_sha: handle.request.commit_sha.clone(),
                source_archive_sha256: if self.state.lock().unwrap().wrong_source_digest {
                    "0".repeat(64)
                } else {
                    hex::encode(Sha256::digest(source()))
                },
                image_tar: if self.invalid {
                    b"invalid OCI".to_vec()
                } else {
                    oci()
                },
                logs: "retained build log".into(),
            })
        }
    }
    fn oci() -> Vec<u8> {
        let config = b"{}";
        let config_hash = hex::encode(Sha256::digest(config));
        let manifest=serde_json::to_vec(&json!({"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{config_hash}")},"layers":[]})).unwrap();
        let hash = hex::encode(Sha256::digest(&manifest));
        let index=serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{hash}")}]})).unwrap();
        Archive::from_files(BTreeMap::from([
            (
                "oci-layout".into(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            ("index.json".into(), index),
            (format!("blobs/sha256/{config_hash}"), config.to_vec()),
            (format!("blobs/sha256/{hash}"), manifest),
        ]))
        .unwrap()
        .encode()
        .unwrap()
    }
    async fn fixture(
        uncertain_create: bool,
        invalid: bool,
    ) -> (
        KubernetesBuildDispatcher,
        Arc<Mutex<Remote>>,
        tempfile::TempDir,
        tokio::task::JoinHandle<()>,
    ) {
        fixture_with_storage(uncertain_create, invalid, false).await
    }
    async fn fixture_with_storage(
        uncertain_create: bool,
        invalid: bool,
        hard: bool,
    ) -> (
        KubernetesBuildDispatcher,
        Arc<Mutex<Remote>>,
        tempfile::TempDir,
        tokio::task::JoinHandle<()>,
    ) {
        use webdock_agent_protocol::build_storage::{
            BuildStorageAction, BuildStorageReservation, BuildStorageTarget,
        };
        let state = Arc::new(Mutex::new(Remote {
            storage: None,
            claim: claim(),
            attempts: vec![],
            events: vec![],
            job: None,
            creates: 0,
            uncertain_create,
            api_rejected_create: false,
            api_offline: false,
            uncertain_stage: false,
            lost_completion_response: false,
            wrong_source_digest: false,
            revoke_on_bind: false,
            upload_bind_fails: false,
            revoke_on_collect: false,
            staged_strategy: None,
            log_tail: None,
            delivered_logs: vec![],
            lost_log_response: false,
            pods_remain: false,
            completed: vec![],
            active: true,
        }));
        if hard {
            let snapshot = BuildStorageReservation {
                version: 1,
                audience: "webdock-build-storage-v1".into(),
                volume_id: "600".into(),
                customer_id: "100".into(),
                project_id: "300".into(),
                build_id: "200".into(),
                generation: 1,
                target: BuildStorageTarget {
                    cluster_id: "400".into(),
                    node: "build-node-1".into(),
                    pool_id: "123e4567-e89b-42d3-a456-426614174000".into(),
                },
                volume_bytes: 27 * 1024 * 1024 * 1024,
                action: BuildStorageAction::Reserve,
                epoch: 1,
                expires_at: chrono::Utc::now() + chrono::Duration::seconds(240),
            };
            let mut s = state.lock().unwrap();
            s.claim["storageRequest"] = serde_json::to_value(&snapshot).unwrap();
            s.claim["namespace"] = json!("webdock-build-qualification");
            s.claim["kubernetesLimits"] = json!({"cpu_millis":2000,"memory_mib":4096,"workspace_mib":10240,"cache_mib":16384,"deadline_seconds":1500});
            s.storage = Some(StorageRemote {
                snapshot,
                epoch: 1,
                maps: BTreeMap::new(),
                handle: None,
                held: true,
                lost_bind: true,
                counter: 0,
                retired: false,
                retire_ttl_seconds: 240,
            });
        }
        let shared = state.clone();
        let client = Client::new(
            service_fn(move |req: http::Request<Body>| {
                let shared = shared.clone();
                async move {
                    let method = req.method().to_string();
                    let path = req.uri().to_string();
                    let bytes = req.into_body().collect().await.unwrap().to_bytes();
                    let body: Value = if bytes.is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_slice(&bytes).unwrap()
                    };
                    let mut s = shared.lock().unwrap();
                    if s.api_offline {
                        s.events.push("api-offline".into());
                        return Ok::<_,std::convert::Infallible>(http::Response::builder().status(503).body(Full::new(serde_json::to_vec(&json!({"kind":"Status","apiVersion":"v1","code":503,"reason":"ServiceUnavailable","message":"owned API offline"})).unwrap().into())).unwrap());
                    }
                    if method == "GET" && path.split('?').next().unwrap().ends_with("/jobs") {
                        assert!(path.contains("limit=1"));
                        s.events.push("availability".into());
                        return Ok::<_,std::convert::Infallible>(http::Response::new(Full::new(serde_json::to_vec(&json!({"kind":"JobList","apiVersion":"batch/v1","metadata":{},"items":[]})).unwrap().into())));
                    }
                    if path.contains("/log?") {
                        return Ok::<_, std::convert::Infallible>(http::Response::new(Full::new(
                            s.log_tail.clone().unwrap_or_default().into(),
                        )));
                    }
                    let (code, value) = if s.storage.is_some()
                        && (path.contains("/configmaps")
                            || path.contains("/persistentvolumeclaims/"))
                    {
                        storage_http(&mut s, &method, path.split('?').next().unwrap(), body)
                    } else if method == "POST" {
                        assert!(
                            s.events.contains(&"intent".into()),
                            "create before durable intent"
                        );
                        s.events.push("create".into());
                        s.creates += 1;
                        let mut job = body;
                        job["metadata"]["uid"] = json!("uid-one");
                        job["metadata"]["resourceVersion"] = json!("1");
                        if !s.api_rejected_create {
                            s.job = Some(job.clone());
                        }
                        if s.uncertain_create || s.api_rejected_create {
                            s.uncertain_create = false;
                            (
                                500,
                                json!({"kind":"Status","apiVersion":"v1","code":500,"reason":"InternalError","message":"response lost"}),
                            )
                        } else {
                            (201, job)
                        }
                    } else if method == "PATCH" {
                        assert!(s.events.contains(&"bind".into()));
                        assert!(s.events.contains(&"upload-bind".into()));
                        assert_eq!(body["metadata"]["uid"], "uid-one");
                        assert_eq!(body["metadata"]["resourceVersion"], "1");
                        s.events.push("release".into());
                        let job = s.job.as_mut().unwrap();
                        job["spec"]["suspend"] = json!(false);
                        (200, job.clone())
                    } else if method == "DELETE" {
                        assert_eq!(body["preconditions"]["uid"], "uid-one");
                        s.events.push("delete".into());
                        s.job = None;
                        (
                            200,
                            json!({"kind":"Status","apiVersion":"v1","status":"Success","code":200}),
                        )
                    } else if path.contains("/pods?") {
                        if let Some(job) = s
                            .job
                            .as_ref()
                            .filter(|_| s.log_tail.is_some() || s.storage.is_some())
                        {
                            (
                                200,
                                json!({"kind":"PodList","apiVersion":"v1","metadata":{},"items":[pod(job)]}),
                            )
                        } else {
                            (
                                200,
                                json!({"kind":"PodList","apiVersion":"v1","metadata":{},"items":if s.pods_remain{vec![json!({"metadata":{"name":"retained"}})]}else{vec![]}}),
                            )
                        }
                    } else if let Some(job) =
                        s.job.as_ref().filter(|_| path.contains("/pods/runner-pod"))
                    {
                        (200, pod(job))
                    } else if let Some(job) = &s.job {
                        (200, job.clone())
                    } else {
                        (
                            404,
                            json!({"kind":"Status","apiVersion":"v1","code":404,"reason":"NotFound","message":"gone"}),
                        )
                    };
                    Ok::<_, std::convert::Infallible>(
                        http::Response::builder()
                            .status(code)
                            .body(Full::new(serde_json::to_vec(&value).unwrap().into()))
                            .unwrap(),
                    )
                }
            }),
            "default",
        );
        let executor = KubernetesJobBuildExecutor::new(
            client.clone(),
            KubernetesBuildConfig {
                profile: if hard {
                    KubernetesBuildProfile::RootlessIdmapSandbox
                } else {
                    KubernetesBuildProfile::Restricted
                },
                namespace: if hard {
                    "webdock-build-qualification"
                } else {
                    "webdock-builds"
                }
                .into(),
                service_account: "build-runner".into(),
                sandbox_runtime_class: "webdock-build-sandbox".into(),
                runner_image: format!("registry.example/runner@sha256:{}", "a".repeat(64)),
                storage_cluster_id: if hard { Some("400".into()) } else { None },
                hold_execution: hard,
                trusted_projects: BTreeSet::from([TrustedProject {
                    customer_id: "100".into(),
                    project_id: "300".into(),
                }]),
            },
        )
        .unwrap();
        {
            let mut s = state.lock().unwrap();
            let parsed = DispatchAttempt::claim(&s.claim).unwrap();
            s.claim["jobName"] = json!(executor.job_name(&parsed.request).unwrap());
            s.claim["runtimeFingerprint"] = json!(executor.runtime_fingerprint());
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/{*path}", any(control))
            .with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let artifacts = root.path().join("artifacts");
        std::fs::create_dir(&artifacts).unwrap();
        std::fs::set_permissions(&artifacts, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut dispatcher = KubernetesBuildDispatcher::new(
            ControlClient::loopback_test(&format!("http://{address}"), "a".repeat(43)).unwrap(),
            executor,
            Arc::new(Transfer {
                state: state.clone(),
                invalid,
            }),
            ArtifactStore::open(artifacts, MAX_OUTPUT).unwrap(),
            root.path().into(),
            "400".into(),
        )
        .unwrap();
        if hard {
            let target = state
                .lock()
                .unwrap()
                .storage
                .as_ref()
                .unwrap()
                .snapshot
                .target
                .clone();
            dispatcher = dispatcher.with_storage(
                crate::build_storage::KubernetesBuildStorage::new(
                    client,
                    "webdock-build-qualification".into(),
                    "webdock-build-storage-requests".into(),
                    "webdock-build-storage-receipts".into(),
                    target,
                    ed25519_dalek::SigningKey::from_bytes(&[6; 32]),
                )
                .unwrap(),
            );
        }
        (dispatcher, state, root, server)
    }
    #[tokio::test]
    async fn intent_precedes_create_and_validated_output_precedes_cleanup_and_retirement() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        }
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.creates, 1);
        assert!(s.attempts.is_empty());
        assert_eq!(s.completed[0]["status"], "succeeded");
        let position = |event: &str| s.events.iter().position(|v| v == event).unwrap();
        assert!(position("stage") < position("intent"));
        assert!(position("complete") < position("delete"));
        assert!(position("delete") < position("retire"));
        server.abort();
    }
    #[tokio::test]
    async fn fixed_volume_waits_for_readiness_and_recovers_lost_binding_ack_before_checkout_or_job()
    {
        let (mut dispatcher, state, _root, server) = fixture_with_storage(false, false, true).await;
        assert!(dispatcher.turn().await.is_err());
        {
            let s = state.lock().unwrap();
            assert_eq!(s.creates, 0);
            assert!(!s.events.contains(&"archive".into()));
            assert!(!s.events.contains(&"stage".into()));
        }
        let control = dispatcher.control.clone();
        let executor = dispatcher.executor.clone();
        let transfer = dispatcher.transfer.clone();
        let store = dispatcher.store.clone();
        let storage = dispatcher.storage.clone();
        let root = dispatcher.root.clone();
        drop(dispatcher);
        let mut dispatcher =
            KubernetesBuildDispatcher::new(control, executor, transfer, store, root, "400".into())
                .unwrap();
        dispatcher.storage = storage;
        state.lock().unwrap().storage.as_mut().unwrap().held = false;
        assert!(
            dispatcher.turn().await.is_err(),
            "lost Core binding response must remain recoverable"
        );
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.creates, 1);
        let binding = s.events.iter().position(|e| e == "volume-bind").unwrap();
        let archive = s.events.iter().position(|e| e == "archive").unwrap();
        assert!(binding < archive);
        assert!(s.attempts[0]["volume_handle"].is_object());
        server.abort();
    }
    #[tokio::test]
    async fn missing_upload_authority_cancels_held_job_without_releasing_it() {
        let (mut dispatcher, state, _root, server) = fixture_with_storage(false, false, true).await;
        {
            let mut s = state.lock().unwrap();
            let h = s.storage.as_mut().unwrap();
            h.held = false;
            h.lost_bind = false;
            s.upload_bind_fails = true;
        }
        assert_eq!(
            dispatcher.turn().await.unwrap_err().code,
            "upload_grant_missing"
        );
        {
            let s = state.lock().unwrap();
            assert!(s.job.is_none());
            assert!(!s.events.contains(&"release".into()));
            assert!(s.events.contains(&"upload-revoke".into()));
        }
        dispatcher.turn().await.unwrap();
        assert!(state.lock().unwrap().attempts.is_empty());
        server.abort();
    }
    #[tokio::test]
    async fn resumed_untrusted_logs_are_redacted_before_local_receipt_and_completion() {
        let (mut dispatcher, state, root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
            s.log_tail =
                Some(b"{\"phase\":\"build\",\"logs\":\"owned-private-fixture\"}\n".to_vec());
        }
        dispatcher.turn().await.unwrap();
        for entry in std::fs::read_dir(root.path()).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().is_some_and(|e| e == "json") {
                assert!(
                    !std::fs::read_to_string(p)
                        .unwrap()
                        .contains("owned-private-fixture")
                );
            }
        }
        let s = state.lock().unwrap();
        assert!(
            !serde_json::to_string(&s.delivered_logs)
                .unwrap()
                .contains("owned-private-fixture")
        );
        assert!(
            !serde_json::to_string(&s.completed)
                .unwrap()
                .contains("owned-private-fixture")
        );
        server.abort();
    }
    #[tokio::test]
    async fn hostile_log_event_cancels_execution_instead_of_pinning_capacity() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        let mut flood =
            serde_json::to_vec(&json!({"phase":"build","logs":"x".repeat(24001)})).unwrap();
        flood.push(b'\n');
        state.lock().unwrap().log_tail = Some(flood);
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert!(s.job.is_none());
        assert!(s.attempts.is_empty());
        server.abort();
    }
    #[tokio::test]
    async fn expired_retirement_gets_fresh_core_epoch_without_releasing_capacity_early() {
        let (mut dispatcher, state, _root, server) = fixture_with_storage(false, false, true).await;
        {
            let mut s = state.lock().unwrap();
            let h = s.storage.as_mut().unwrap();
            h.held = false;
            h.lost_bind = false;
        }
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.active = false;
            s.attempts[0]["cancelRequested"] = json!(true);
            s.storage.as_mut().unwrap().held = true;
            s.storage.as_mut().unwrap().retire_ttl_seconds = 1;
        }
        assert_eq!(
            dispatcher.turn().await.unwrap_err().code,
            "build_storage_pending"
        );
        let attempt = DispatchAttempt::claim(&state.lock().unwrap().attempts[0]).unwrap();
        let path = dispatcher.path(&attempt.request).unwrap();
        let receipt = KubernetesBuildDispatcher::read_receipt(&path)
            .unwrap()
            .unwrap();
        let old = receipt.storage_operation.as_ref().unwrap().clone();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        {
            let mut s = state.lock().unwrap();
            assert_eq!(s.attempts.len(), 1);
            assert!(!s.storage.as_ref().unwrap().retired);
            let h = s.storage.as_mut().unwrap();
            h.held = false;
            h.retire_ttl_seconds = 240;
        }
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert!(s.attempts.is_empty());
        assert!(s.storage.as_ref().unwrap().retired);
        assert!(s.storage.as_ref().unwrap().epoch > old.signed.reservation.epoch);
        server.abort();
    }
    #[tokio::test]
    async fn fixed_volume_capacity_waits_for_all_pods_and_node_retirement_receipt() {
        let (mut dispatcher, state, _root, server) = fixture_with_storage(false, false, true).await;
        {
            let mut s = state.lock().unwrap();
            let h = s.storage.as_mut().unwrap();
            h.held = false;
            h.lost_bind = false;
        }
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
            s.pods_remain = true;
        }
        dispatcher.turn().await.unwrap();
        {
            let s = state.lock().unwrap();
            assert_eq!(s.attempts.len(), 1);
            assert!(!s.storage.as_ref().unwrap().retired);
            assert!(!s.events.contains(&"volume-retired".into()));
        }
        state.lock().unwrap().pods_remain = false;
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert!(s.attempts.is_empty());
        assert!(s.storage.as_ref().unwrap().retired);
        let physical = s.events.iter().position(|e| e == "volume-retired").unwrap();
        let retired = s.events.iter().position(|e| e == "retire").unwrap();
        assert!(physical < retired);
        server.abort();
    }
    #[tokio::test]
    async fn cancelling_pending_volume_never_checks_out_source_and_waits_for_node_fence() {
        let (mut dispatcher, state, _root, server) = fixture_with_storage(false, false, true).await;
        assert!(dispatcher.turn().await.is_err());
        {
            let mut s = state.lock().unwrap();
            s.active = false;
            s.attempts[0]["cancelRequested"] = json!(true);
        }
        assert!(dispatcher.turn().await.is_err());
        {
            let s = state.lock().unwrap();
            assert_eq!(s.attempts.len(), 1);
            assert_eq!(s.creates, 0);
            assert!(!s.events.contains(&"archive".into()));
        }
        state.lock().unwrap().storage.as_mut().unwrap().held = false;
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert!(s.attempts.is_empty());
        assert!(s.storage.as_ref().unwrap().retired);
        assert_eq!(s.creates, 0);
        server.abort();
    }
    #[tokio::test]
    async fn uncertain_create_is_observed_and_bound_without_a_second_create() {
        let (mut dispatcher, state, _root, server) = fixture(true, false).await;
        assert!(dispatcher.turn().await.is_err());
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.creates, 1);
        assert_eq!(s.attempts[0]["job_uid"], "uid-one");
        assert_eq!(s.events.iter().filter(|v| v.as_str() == "stage").count(), 1);
        server.abort();
    }
    #[tokio::test]
    async fn explicit_api_failure_is_retryable_only_after_absence_and_retirement() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        state.lock().unwrap().api_rejected_create = true;
        assert_eq!(
            dispatcher.turn().await.unwrap_err().code,
            "kubernetes_api_unavailable"
        );
        assert!(!state.lock().unwrap().events.contains(&"retry-api".into()));
        dispatcher.turn().await.unwrap();
        {
            let s = state.lock().unwrap();
            assert!(s.job.is_none());
            assert!(s.attempts.is_empty());
            assert!(s.events.contains(&"retry-api".into()));
        }
        server.abort();
    }
    #[tokio::test]
    async fn revoked_attempt_is_cancelled_but_slot_waits_for_pod_absence() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.active = false;
            s.attempts[0]["cancelRequested"] = json!(true);
            // Core clears the execution lease on user cancellation. Cleanup
            // still has the immutable attempt/UID authority, never execution authority.
            s.attempts[0]["buildStatus"] = json!("canceled");
            s.attempts[0]["leaseUntil"] = Value::Null;
            s.pods_remain = true;
        }
        dispatcher.turn().await.unwrap();
        assert!(!state.lock().unwrap().attempts.is_empty());
        state.lock().unwrap().pods_remain = false;
        dispatcher.turn().await.unwrap();
        assert!(state.lock().unwrap().attempts.is_empty());
        assert!(state.lock().unwrap().completed.is_empty());
        server.abort();
    }
    #[tokio::test]
    async fn job_completion_with_invalid_oci_is_retained_as_failure() {
        let (mut dispatcher, state, _root, server) = fixture(false, true).await;
        dispatcher.turn().await.unwrap();
        state.lock().unwrap().job.as_mut().unwrap()["status"] =
            json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.completed[0]["status"], "failed");
        assert!(s.completed[0]["artifact"].is_null());
        assert!(s.attempts.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn uncertain_source_stage_is_retired_without_staging_again_or_creating_a_job() {
        let (mut dispatcher, state, root, server) = fixture(false, false).await;
        state.lock().unwrap().uncertain_stage = true;
        assert!(dispatcher.turn().await.is_err());
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.creates, 0);
        assert!(s.attempts.is_empty());
        assert_eq!(s.events.iter().filter(|e| e.as_str() == "stage").count(), 1);
        let receipt =
            KubernetesBuildDispatcher::read_receipt(&root.path().join("attempt-200-1.json"))
                .unwrap()
                .unwrap();
        assert!(receipt.retired);
        assert!(receipt.outcome.is_some());
        assert!(
            receipt.logs.contains("transfer_uncertain"),
            "recovery preserves the original preparation failure"
        );
        assert!(
            s.delivered_logs
                .iter()
                .any(|entry| entry["phase"] == "provisioning"
                    && entry["text"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("transfer_uncertain")),
            "the original safe failure code outlives local receipt cleanup"
        );
        server.abort();
    }

    #[tokio::test]
    async fn completion_survives_lost_response_and_restart_before_slot_retirement() {
        let (mut dispatcher, state, root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.lost_completion_response = true;
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        }
        assert!(dispatcher.turn().await.is_err());
        assert!(state.lock().unwrap().job.is_none());
        assert!(!state.lock().unwrap().attempts.is_empty());
        let control = dispatcher.control.clone();
        let executor = dispatcher.executor.clone();
        let transfer = dispatcher.transfer.clone();
        drop(dispatcher);
        let mut restarted = KubernetesBuildDispatcher::new(
            control,
            executor,
            transfer,
            ArtifactStore::open(root.path().join("artifacts"), MAX_OUTPUT).unwrap(),
            root.path().into(),
            "400".into(),
        )
        .unwrap();
        restarted.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.creates, 1);
        assert!(s.attempts.is_empty());
        assert!(s.completed.len() >= 2);
        assert!(s.completed.iter().all(|body| body == &s.completed[0]));
        assert_eq!(
            s.events.iter().filter(|e| e.as_str() == "collect").count(),
            1
        );
        server.abort();
    }

    #[tokio::test]
    async fn valid_oci_from_a_different_source_archive_cannot_complete_successfully() {
        let (mut dispatcher, state, root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.wrong_source_digest = true;
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        }
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.completed[0]["status"], "failed");
        let receipt =
            KubernetesBuildDispatcher::read_receipt(&root.path().join("attempt-200-1.json"))
                .unwrap()
                .unwrap();
        assert_eq!(
            receipt.source_archive_sha256,
            Some(hex::encode(Sha256::digest(source())))
        );
        server.abort();
    }

    #[tokio::test]
    async fn losing_authority_during_uid_binding_cancels_the_observed_job_before_returning() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        state.lock().unwrap().revoke_on_bind = true;
        assert!(dispatcher.turn().await.is_err());
        assert!(state.lock().unwrap().job.is_none());
        assert!(state.lock().unwrap().completed.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn losing_authority_during_result_transfer_cancels_without_sending_completion() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.revoke_on_collect = true;
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        }
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert!(s.job.is_none());
        assert!(s.completed.is_empty());
        assert!(s.attempts.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn explicit_container_strategy_survives_claim_staging_and_reconciliation() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        state.lock().unwrap().claim["containerBuild"] = json!({"strategy":"railpack","contextDirectory":"service","dockerfile":"Containerfile","buildArgs":{"APP_MODE":"production"}});
        dispatcher.turn().await.unwrap();
        assert_eq!(
            state.lock().unwrap().staged_strategy.as_deref(),
            Some("railpack")
        );
        dispatcher.turn().await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn structured_log_acknowledgements_survive_lost_response_before_completion_and_cleanup() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.lost_log_response = true;
            s.log_tail=Some(format!("runtime noise with confidential text\n{}\n{{\"phase\":\"build\",\"logs\":\"unfinished",json!({"phase":"build","logs":format!("safe [REDACTED] {}","ü".repeat(5000))})).into_bytes());
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        }
        assert!(dispatcher.turn().await.is_err());
        assert!(state.lock().unwrap().job.is_some());
        assert!(state.lock().unwrap().completed.is_empty());
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert!(s.attempts.is_empty());
        assert!(s.delivered_logs.len() >= 3);
        assert_eq!(s.delivered_logs[0], s.delivered_logs[1]);
        assert!(
            s.delivered_logs
                .iter()
                .all(|v| v["text"].as_str().unwrap().len() <= 8192
                    && v["phase"] == "building"
                    && !v["text"].as_str().unwrap().contains("confidential"))
        );
        let chunks: Vec<_> = s
            .delivered_logs
            .iter()
            .skip(1)
            .map(|v| v["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            chunks.concat(),
            format!("safe [REDACTED] {}", "ü".repeat(5000))
        );
        server.abort();
    }

    #[tokio::test]
    async fn cancelled_attempt_abandons_fenced_completion_without_pinning_the_capacity_slot() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        {
            let mut s = state.lock().unwrap();
            s.lost_completion_response = true;
            s.job.as_mut().unwrap()["status"] =
                json!({"succeeded":1,"conditions":[{"type":"Complete","status":"True"}]});
        }
        assert!(dispatcher.turn().await.is_err());
        {
            let mut s = state.lock().unwrap();
            s.active = false;
            s.attempts[0]["cancelRequested"] = json!(true);
        }
        dispatcher.turn().await.unwrap();
        assert!(state.lock().unwrap().attempts.is_empty());
        assert!(!dispatcher.outbox.has_pending());
        server.abort();
    }
    #[tokio::test]
    async fn offline_api_polls_do_not_claim_source_or_jobs_and_recovery_claims_once() {
        let (mut dispatcher, state, _root, server) = fixture(false, false).await;
        let original = state.lock().unwrap().claim.clone();
        state.lock().unwrap().api_offline = true;
        for _ in 0..3 {
            assert_eq!(
                dispatcher.turn().await.unwrap_err().code,
                "kubernetes_api_unavailable"
            );
        }
        {
            let s = state.lock().unwrap();
            assert_eq!(
                s.claim, original,
                "the queued build remains unclaimed in Core"
            );
            assert!(s.attempts.is_empty());
            assert!(s.job.is_none());
            assert_eq!(s.creates, 0);
            assert_eq!(
                s.events
                    .iter()
                    .filter(|event| event.as_str() == "api-offline")
                    .count(),
                3
            );
            assert!(!s.events.iter().any(|event| {
                ["claim", "archive", "stage", "environment-handoff", "create"]
                    .contains(&event.as_str())
            }));
        }
        state.lock().unwrap().api_offline = false;
        dispatcher.turn().await.unwrap();
        dispatcher.turn().await.unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.creates, 1);
        assert_eq!(s.attempts.len(), 1);
        assert_eq!(
            s.events
                .iter()
                .filter(|event| event.as_str() == "claim")
                .count(),
            1
        );
        assert_eq!(
            s.events
                .iter()
                .filter(|event| event.as_str() == "availability")
                .count(),
            1
        );
        assert_eq!(
            s.events
                .iter()
                .filter(|event| event.as_str() == "archive")
                .count(),
            1
        );
        server.abort();
    }
    #[tokio::test]
    async fn api_outage_with_active_attempt_keeps_identity_capacity_and_skips_fresh_claims() {
        let (mut dispatcher, state, root, server) = fixture(false, false).await;
        dispatcher.turn().await.unwrap();
        let before = state.lock().unwrap().job.clone();
        let fingerprint = std::fs::read(root.path().join("attempt-200-1.json")).unwrap();
        state.lock().unwrap().api_offline = true;
        for _ in 0..3 {
            assert_eq!(
                dispatcher.turn().await.unwrap_err().code,
                "kubernetes_api_unavailable"
            );
        }
        let s = state.lock().unwrap();
        assert_eq!(s.job, before);
        assert_eq!(s.attempts.len(), 1);
        assert_eq!(s.creates, 1);
        assert!(s.completed.is_empty());
        assert_eq!(
            s.events
                .iter()
                .filter(|event| event.as_str() == "claim")
                .count(),
            1
        );
        assert_eq!(
            s.events
                .iter()
                .filter(|event| event.as_str() == "availability")
                .count(),
            1
        );
        assert!(
            !s.events
                .iter()
                .any(|event| ["retire", "cleanup", "delete"].contains(&event.as_str()))
        );
        let retained: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("attempt-200-1.json")).unwrap())
                .unwrap();
        let prior: Value = serde_json::from_slice(&fingerprint).unwrap();
        assert_eq!(retained["uid"], prior["uid"]);
        assert_eq!(retained["spec_fingerprint"], prior["spec_fingerprint"]);
        assert_eq!(retained["retired"], false);
        server.abort();
    }
}
