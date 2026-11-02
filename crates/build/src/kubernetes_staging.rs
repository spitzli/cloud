//! Trusted coordinator for exact source and result objects. Credentials never enter build Pods.
use crate::kubernetes_upload::AttemptUploadGateway;
use crate::{
    Result,
    artifact::{MAX_OUTPUT, MAX_SOURCE},
    container_runner::{ContainerBuildRequest, Strategy, read_input_file},
    contracts::ContainerSettings,
    error,
    execution::GuestRequest,
    io,
    kubernetes::{KubernetesBuildRequest, KubernetesJobBuildExecutor, KubernetesJobHandle},
    kubernetes_dispatcher::{KubernetesArtifactTransfer, KubernetesTransferredOutput},
    kubernetes_transfer::{AttemptTransfer, MAX_HANDOFF_BYTES, TransferPolicy, verify_result},
};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use webdock_object_storage::{StagingMethod, Store};

pub struct KubernetesStagingTransfer {
    store: Store,
    executor: Arc<KubernetesJobBuildExecutor>,
    policy: TransferPolicy,
    directory: PathBuf,
    upload_gateway: Option<Arc<AttemptUploadGateway>>,
}
fn scope(request: &KubernetesBuildRequest) -> Result<String> {
    for id in [&request.customer_id, &request.project_id, &request.build_id] {
        crate::contracts::id(id)?;
    }
    if request.generation == 0 {
        return Err(error("invalid_generation"));
    }
    Ok(format!(
        "build-attempts/{}/{}/{}/{}",
        request.customer_id, request.project_id, request.build_id, request.generation
    ))
}
impl KubernetesStagingTransfer {
    pub fn new(
        _store: Store,
        _executor: Arc<KubernetesJobBuildExecutor>,
        _policy: TransferPolicy,
        _directory: PathBuf,
    ) -> Result<Self> {
        Err(error("upload_gateway_required"))
    }
    pub fn new_with_upload_gateway(
        store: Store,
        executor: Arc<KubernetesJobBuildExecutor>,
        policy: TransferPolicy,
        directory: PathBuf,
        upload_gateway: Arc<AttemptUploadGateway>,
    ) -> Result<Self> {
        Self::construct(store, executor, policy, directory, Some(upload_gateway))
    }
    /// Compatibility for the closed pre-multitenancy qualification fixture only. This transport
    /// has no provider-enforced result byte cap/revocation and MUST NOT admit untrusted projects.
    pub fn trusted_pilot_only_unbounded_provider_put(
        store: Store,
        executor: Arc<KubernetesJobBuildExecutor>,
        policy: TransferPolicy,
        directory: PathBuf,
    ) -> Result<Self> {
        Self::construct(store, executor, policy, directory, None)
    }
    fn construct(
        store: Store,
        executor: Arc<KubernetesJobBuildExecutor>,
        policy: TransferPolicy,
        directory: PathBuf,
        upload_gateway: Option<Arc<AttemptUploadGateway>>,
    ) -> Result<Self> {
        crate::kubernetes_transfer::TransferClient::new(policy.clone())?;
        if let Some(gateway) = &upload_gateway
            && (gateway.max_bytes() != policy.max_result_bytes
                || !policy.allowed_origins.iter().any(|origin| {
                    origin.trim_end_matches('/') == gateway.origin().trim_end_matches('/')
                }))
        {
            return Err(error("upload_gateway_policy_conflict"));
        }
        let meta = directory.symlink_metadata().map_err(io)?;
        // SAFETY: geteuid takes no arguments and has no preconditions.
        if !meta.is_dir() || meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(error("private_staging_state_required"));
        }
        Ok(Self {
            store,
            executor,
            policy,
            directory: directory.canonicalize().map_err(io)?,
            upload_gateway,
        })
    }
    fn record(&self, request: &KubernetesBuildRequest) -> Result<PathBuf> {
        let exact_scope = scope(request)?;
        Ok(self.directory.join(format!(
            "{}.json",
            hex::encode(Sha256::digest(exact_scope.as_bytes()))
        )))
    }
    fn read(&self, request: &KubernetesBuildRequest) -> Result<AttemptTransfer> {
        let bytes = read_input_file(&self.record(request)?, MAX_HANDOFF_BYTES)?;
        let attempt: AttemptTransfer =
            serde_json::from_slice(&bytes).map_err(|_| error("invalid_transfer_state"))?;
        attempt.validate(&self.policy)?;
        if attempt.customer_id != request.customer_id
            || attempt.project_id != request.project_id
            || attempt.build_id != request.build_id
            || attempt.generation != request.generation
            || attempt.request.source_sha != request.commit_sha
        {
            return Err(error("transfer_identity_mismatch"));
        }
        Ok(attempt)
    }
    fn persist(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        // A failed stage cannot be silently retried with fresh credentials or different source.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(io)?;
        file.write_all(bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        std::fs::File::open(&self.directory)
            .map_err(io)?
            .sync_all()
            .map_err(io)
    }
}
#[async_trait::async_trait]
impl KubernetesArtifactTransfer for KubernetesStagingTransfer {
    async fn stage(
        &self,
        request: &KubernetesBuildRequest,
        guest: &GuestRequest,
        settings: &ContainerSettings,
        source: &[u8],
        archive_sha256: &str,
    ) -> Result<()> {
        let scope = scope(request)?;
        settings.validate()?;
        let expected = hex::encode(Sha256::digest(source));
        if expected != archive_sha256 || source.len() as u64 > self.policy.max_source_bytes {
            return Err(error("source_digest_mismatch"));
        }
        crate::artifact::Archive::read(source, MAX_SOURCE)?;
        let ttl = Duration::from_secs(1800);
        let source_url = self
            .store
            .presign_build_source(&scope, ttl)
            .await
            .map_err(|_| error("staging_unavailable"))?;
        let result_url = if self.upload_gateway.is_none() {
            self.store
                .presign_staging(&scope, "result.tar", StagingMethod::Put, ttl)
                .await
                .map_err(|_| error("staging_unavailable"))?
                .to_string()
        } else {
            String::new()
        };
        let strategy = match settings.strategy.as_str() {
            "auto" => Strategy::Auto,
            "dockerfile" => Strategy::Dockerfile,
            "railpack" => Strategy::Railpack,
            _ => return Err(error("invalid_build_strategy")),
        };
        let dockerfile = if strategy == Strategy::Auto && settings.dockerfile == "Dockerfile" {
            None
        } else {
            Some(settings.dockerfile.clone())
        };
        let mut attempt = AttemptTransfer {
            customer_id: request.customer_id.clone(),
            project_id: request.project_id.clone(),
            build_id: request.build_id.clone(),
            generation: request.generation,
            request: ContainerBuildRequest {
                source_sha: request.commit_sha.clone(),
                archive_sha256: format!("sha256:{expected}"),
                root_directory: guest.root_directory.clone(),
                context_directory: settings.context_directory.clone(),
                dockerfile,
                strategy,
                build_args: settings.build_args.clone(),
                limits: guest.limits.clone(),
            },
            source_size_bytes: source.len() as u64,
            source_url: source_url.to_string(),
            result_put_url: result_url,
            build_secrets: guest.build_environment.clone(),
        };
        if let Some(gateway) = &self.upload_gateway {
            attempt.result_put_url = gateway.issue(&attempt, ttl.as_secs())?.as_str().into();
        }
        attempt.validate(&self.policy)?;
        let bytes = serde_json::to_vec(&attempt).map_err(|_| error("invalid_transfer_state"))?;
        if bytes.len() as u64 > MAX_HANDOFF_BYTES {
            return Err(error("handoff_limit"));
        }
        self.persist(&self.record(request)?, &bytes)?;
        self.store
            .write_staging(&scope, "source.tar", source, self.policy.max_source_bytes)
            .await
            .map_err(|_| error("staging_unavailable"))?;
        self.executor
            .stage_handoff(request, &bytes)
            .await
            .map_err(|_| error("kubernetes_handoff_unavailable"))?;
        Ok(())
    }
    async fn bind(&self, handle: &KubernetesJobHandle) -> Result<()> {
        if let Some(gateway) = &self.upload_gateway {
            gateway.bind(&self.read(&handle.request)?, &handle.uid)?;
        }
        Ok(())
    }
    async fn revoke(&self, request: &KubernetesBuildRequest) -> Result<()> {
        // Missing/corrupt handoff state must never keep an already-issued capability live.
        if let Some(gateway) = &self.upload_gateway {
            gateway.revoke_scope(
                &request.customer_id,
                &request.project_id,
                &request.build_id,
                request.generation,
            )?;
        }
        Ok(())
    }
    async fn collect(&self, handle: &KubernetesJobHandle) -> Result<KubernetesTransferredOutput> {
        let attempt = self.read(&handle.request)?;
        let bytes = if let Some(gateway) = &self.upload_gateway {
            gateway.read(&attempt, &handle.uid)?
        } else {
            self.store
                .read_staging(
                    &scope(&handle.request)?,
                    "result.tar",
                    self.policy.max_result_bytes,
                )
                .await
                .map_err(|_| error("staging_result_unavailable"))?
        };
        let result = verify_result(&attempt, &handle.uid, &bytes)?;
        if result.artifact.len() as u64 > MAX_OUTPUT {
            return Err(error("artifact_limit"));
        }
        Ok(KubernetesTransferredOutput {
            build_id: attempt.build_id,
            generation: attempt.generation,
            job_uid: handle.uid.clone(),
            source_sha: attempt.request.source_sha,
            source_archive_sha256: attempt
                .request
                .archive_sha256
                .trim_start_matches("sha256:")
                .into(),
            image_tar: result.artifact,
            logs: result.logs,
        })
    }
    async fn cleanup(&self, request: &KubernetesBuildRequest) -> Result<()> {
        // The dispatcher already proved the exact Job and all Pods absent before this call.
        if let Some(gateway) = &self.upload_gateway {
            gateway.retire_scope(
                &request.customer_id,
                &request.project_id,
                &request.build_id,
                request.generation,
            )?;
        }
        let record = self.record(request)?;
        let digest = match record.symlink_metadata() {
            Ok(_) => hex::encode(Sha256::digest(read_input_file(&record, MAX_HANDOFF_BYTES)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "0".repeat(64),
            Err(e) => return Err(io(e)),
        };
        if let Some(handoff) = self
            .executor
            .observe_handoff(request, &digest)
            .await
            .map_err(|_| error("kubernetes_handoff_unavailable"))?
        {
            self.executor
                .delete_handoff(&handoff)
                .await
                .map_err(|_| error("kubernetes_handoff_unavailable"))?;
        }
        // Staging objects are not retained release artifacts. Retention cleanup is separate.
        self.store
            .delete_staging(&scope(request)?, "source.tar")
            .await
            .map_err(|_| error("staging_unavailable"))?;
        if self.upload_gateway.is_none() {
            self.store
                .delete_staging(&scope(request)?, "result.tar")
                .await
                .map_err(|_| error("staging_unavailable"))?;
        }
        let path = self.record(request)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
        std::fs::File::open(&self.directory)
            .map_err(io)?
            .sync_all()
            .map_err(io)
    }
}
