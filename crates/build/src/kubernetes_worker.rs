//! Explicit trusted Kubernetes worker configuration. No ambient discovery or VM backend.
use crate::{
    Result,
    artifact::{ArtifactStore, MAX_OUTPUT},
    error, io,
    kubernetes::{
        KubernetesBuildConfig, KubernetesBuildProfile, KubernetesJobBuildExecutor, TrustedProject,
    },
    kubernetes_dispatcher::KubernetesBuildDispatcher,
    kubernetes_staging::KubernetesStagingTransfer,
    kubernetes_transfer::{TransferClient, TransferPolicy},
    publisher::{RegistryCredential, RegistryPublisher},
    worker::{ControlClient, Outbox, publish_once_without_builder},
};
use base64::Engine;
use kube::config::Kubeconfig;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
pub const NAMESPACE: &str = "webdock-build-qualification";
pub const RUNTIME_CLASS: &str = "webdock-build-sandbox";
const SERVICE_ACCOUNT: &str = "webdock-build-job";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KubernetesWorkerConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(rename = "controlURL")]
    control_url: String,
    credential_file: PathBuf,
    kubeconfig_file: PathBuf,
    kube_context: String,
    api_endpoint: String,
    #[serde(rename = "caSHA256")]
    ca_sha256: String,
    #[serde(rename = "buildClusterID")]
    build_cluster_id: String,
    runner_image: String,
    trusted_projects: BTreeSet<TrustedProject>,
    state_root: PathBuf,
    artifact_root: PathBuf,
    staging_root: PathBuf,
    transfer_policy_file: PathBuf,
    staging_storage: StorageConfig,
    #[serde(default = "store_limit")]
    artifact_store_limit: u64,
    #[serde(default = "poll_seconds")]
    poll_seconds: u64,
    #[serde(default)]
    publication: Option<PublicationConfig>,
    #[serde(default)]
    storage: Option<BuildStorageConfig>,
    #[serde(default)]
    upload_gateway: Option<UploadGatewayConfig>,
    #[serde(default)]
    owned_token_file: Option<PathBuf>,
    #[serde(default)]
    identity: Option<crate::kubernetes_identity::KubernetesIdentityConfig>,
    #[serde(default)]
    trusted_pilot_only: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UploadGatewayConfig {
    root: PathBuf,
    filesystem_ceiling_bytes: u64,
    origin: String,
    listen: std::net::SocketAddr,
    timeout_seconds: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuildStorageConfig {
    target: spitzli_agent_protocol::build_storage::BuildStorageTarget,
    signing_key_file: PathBuf,
    requests_namespace: String,
    receipts_namespace: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageConfig {
    storage: spitzli_object_storage::S3Config,
    credential_file: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationConfig {
    state_root: PathBuf,
    projects: BTreeMap<String, PublicationProject>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublicationProject {
    repository: String,
    credential_file: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrySecret {
    username: String,
    password: String,
}
fn store_limit() -> u64 {
    20 * 1024 * 1024 * 1024
}
fn poll_seconds() -> u64 {
    5
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn absolute(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(error("untrusted_worker_path"));
    }
    Ok(())
}
fn owned_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    absolute(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| error("untrusted_worker_path"))?;
    if parent.canonicalize().map_err(io)? != parent {
        return Err(error("untrusted_worker_path"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    // SAFETY: geteuid takes no arguments and has no preconditions.
    let owner = unsafe { libc::geteuid() };
    if !meta.is_file()
        || meta.uid() != owner
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
        || meta.len() > limit
    {
        return Err(error("untrusted_worker_configuration"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > limit {
        return Err(error("worker_configuration_limit"));
    }
    Ok(bytes)
}
fn private_directory(path: &Path) -> Result<()> {
    absolute(path)?;
    let meta = path.symlink_metadata().map_err(io)?;
    // SAFETY: geteuid takes no arguments and has no preconditions.
    let owner = unsafe { libc::geteuid() };
    if !meta.is_dir()
        || meta.uid() != owner
        || meta.mode() & 0o7777 != 0o700
        || path.canonicalize().map_err(io)? != path
    {
        return Err(error("private_worker_directory_required"));
    }
    Ok(())
}
fn directory_lock(root: &Path, name: &str) -> Result<std::fs::File> {
    private_directory(root)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(name))
        .map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    // SAFETY: geteuid takes no arguments and has no preconditions.
    let owner = unsafe { libc::geteuid() };
    if !meta.is_file() || meta.uid() != owner || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
        return Err(error("untrusted_worker_lock"));
    }
    // SAFETY: the live File owns its descriptor; flock retains no borrowed pointers.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(error("kubernetes_worker_already_running"));
    }
    Ok(file)
}
fn https_endpoint(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value).map_err(|_| error("invalid_kubernetes_endpoint"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(error("invalid_kubernetes_endpoint"));
    }
    Ok(url)
}
/// Trusted callers must establish input ownership before supplying bytes.
/// Build directly: ConfigLoader reads ambient HTTPS_PROXY even with an explicit file.
pub fn pinned_kubernetes_config(
    bytes: &[u8],
    context: &str,
    endpoint: &str,
    ca_sha256: &str,
) -> Result<kube::Config> {
    let expected = https_endpoint(endpoint)?;
    if context.is_empty() || context.len() > 128 || !digest(ca_sha256) {
        return Err(error("invalid_kubernetes_context"));
    }
    let document = std::str::from_utf8(bytes).map_err(|_| error("invalid_kubeconfig"))?;
    let kube = Kubeconfig::from_yaml(document).map_err(|_| error("invalid_kubeconfig"))?;
    if kube.contexts.len() != 1
        || kube.clusters.len() != 1
        || kube.auth_infos.len() != 1
        || !kube.other.is_empty()
        || kube.extensions.is_some()
        || kube.kind.as_deref() != Some("Config")
        || kube.api_version.as_deref() != Some("v1")
    {
        return Err(error("invalid_kubeconfig"));
    }
    let named = &kube.contexts[0];
    if named.name != context || !named.other.is_empty() {
        return Err(error("invalid_kubernetes_context"));
    }
    let selected = named
        .context
        .as_ref()
        .ok_or_else(|| error("invalid_kubernetes_context"))?;
    if !selected.other.is_empty()
        || selected.extensions.is_some()
        || selected
            .namespace
            .as_deref()
            .is_some_and(|n| n != NAMESPACE)
    {
        return Err(error("invalid_kubernetes_context"));
    }
    let named_cluster = &kube.clusters[0];
    let named_user = &kube.auth_infos[0];
    if selected.cluster != named_cluster.name
        || selected.user.as_deref() != Some(named_user.name.as_str())
        || !named_cluster.other.is_empty()
        || !named_user.other.is_empty()
    {
        return Err(error("invalid_kubernetes_context"));
    }
    let cluster = named_cluster
        .cluster
        .as_ref()
        .ok_or_else(|| error("invalid_kubeconfig"))?;
    let cluster_json = serde_json::to_value(cluster).map_err(|_| error("invalid_kubeconfig"))?;
    if cluster_json.as_object().is_none_or(|fields| {
        fields.keys().any(|key| {
            ![
                "server",
                "certificate-authority-data",
                "insecure-skip-tls-verify",
                "disable-compression",
            ]
            .contains(&key.as_str())
        })
    }) || cluster.insecure_skip_tls_verify == Some(true)
    {
        return Err(error("unsupported_kubernetes_transport"));
    }
    let actual = https_endpoint(
        cluster
            .server
            .as_deref()
            .ok_or_else(|| error("invalid_kubernetes_endpoint"))?,
    )?;
    if actual != expected {
        return Err(error("kubernetes_endpoint_mismatch"));
    }
    let auth = named_user
        .auth_info
        .as_ref()
        .ok_or_else(|| error("unsupported_kubernetes_auth"))?;
    let auth_json = serde_json::to_value(auth).map_err(|_| error("unsupported_kubernetes_auth"))?;
    if auth_json
        .as_object()
        .is_none_or(|fields| fields.len() != 1 || !fields.contains_key("token"))
        || auth_json["token"].as_str().is_none_or(|token| {
            token.is_empty() || token.len() > 16384 || token.chars().any(char::is_control)
        })
    {
        return Err(error("unsupported_kubernetes_auth"));
    }
    let ca = base64::engine::general_purpose::STANDARD
        .decode(
            cluster
                .certificate_authority_data
                .as_deref()
                .ok_or_else(|| error("invalid_kubernetes_ca"))?,
        )
        .map_err(|_| error("invalid_kubernetes_ca"))?;
    if ca.len() > 64 * 1024 || hex::encode(Sha256::digest(&ca)) != ca_sha256 {
        return Err(error("kubernetes_ca_mismatch"));
    }
    let certificates = CertificateDer::pem_slice_iter(&ca)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| error("invalid_kubernetes_ca"))?;
    if certificates.is_empty() {
        return Err(error("invalid_kubernetes_ca"));
    }
    let mut roots = rustls::RootCertStore::empty();
    let mut der = Vec::new();
    for certificate in certificates {
        der.push(certificate.as_ref().to_vec());
        roots
            .add(certificate)
            .map_err(|_| error("invalid_kubernetes_ca"))?;
    }
    let mut config = kube::Config::new(
        expected
            .as_str()
            .parse()
            .map_err(|_| error("invalid_kubernetes_endpoint"))?,
    );
    config.default_namespace = NAMESPACE.into();
    config.root_cert = Some(der);
    config.auth_info = auth.clone();
    config.proxy_url = None;
    config.default_retry = false;
    config.connect_timeout = Some(Duration::from_secs(5));
    config.read_timeout = Some(Duration::from_secs(15));
    config.write_timeout = Some(Duration::from_secs(15));
    Ok(config)
}
impl KubernetesWorkerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&owned_file(path, 128 * 1024)?)
            .map_err(|_| error("invalid_kubernetes_worker_configuration"))
    }
    fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Err(error("kubernetes_worker_disabled"));
        }
        match (&self.owned_token_file, &self.identity) {
            (None, None) => {}
            (Some(path), Some(identity)) => {
                absolute(path)?;
                identity.validate()?;
            }
            _ => return Err(error("invalid_kubernetes_identity_configuration")),
        }
        if !self.trusted_pilot_only && (self.storage.is_none() || self.upload_gateway.is_none()) {
            return Err(error("hardened_build_configuration_required"));
        }
        if !self.trusted_pilot_only && self.identity.is_none() {
            return Err(error("managed_kubernetes_identity_required"));
        }
        if let Some(upload) = &self.upload_gateway {
            absolute(&upload.root)?;
            if !upload.listen.ip().is_loopback()
                || upload.listen.port() == 0
                || !cfg!(feature = "service")
            {
                return Err(error("invalid_upload_gateway_listener"));
            }
        }
        crate::contracts::id(&self.build_cluster_id)?;
        if self.trusted_projects.is_empty()
            || self.trusted_projects.len() > 256
            || !(1..=60).contains(&self.poll_seconds)
            || !(MAX_OUTPUT..=200 * 1024 * 1024 * 1024).contains(&self.artifact_store_limit)
        {
            return Err(error("invalid_kubernetes_worker_configuration"));
        }
        for project in &self.trusted_projects {
            crate::contracts::id(&project.customer_id)?;
            crate::contracts::id(&project.project_id)?;
        }
        if let Some(storage) = &self.storage {
            storage
                .target
                .validate()
                .map_err(|_| error("invalid_build_storage_target"))?;
            if storage.target.cluster_id != self.build_cluster_id {
                return Err(error("build_storage_target_conflict"));
            }
            absolute(&storage.signing_key_file)?;
        }
        let Some((repository, hash)) = self.runner_image.split_once("@sha256:") else {
            return Err(error("invalid_runner_image"));
        };
        if repository.is_empty()
            || repository.ends_with(":latest")
            || repository.contains("..")
            || !repository
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"/._-:".contains(&b))
            || !digest(hash)
        {
            return Err(error("invalid_runner_image"));
        }
        https_endpoint(&self.api_endpoint)?;
        if !digest(&self.ca_sha256) || self.kube_context.is_empty() {
            return Err(error("invalid_kubernetes_worker_configuration"));
        }
        for path in [&self.state_root, &self.artifact_root, &self.staging_root] {
            private_directory(path)?;
        }
        let mut directories: Vec<&Path> =
            vec![&self.state_root, &self.artifact_root, &self.staging_root];
        if let Some(publication) = &self.publication {
            private_directory(&publication.state_root)?;
            directories.push(&publication.state_root);
        }
        if let Some(path) = &self.owned_token_file {
            directories.push(
                path.parent()
                    .ok_or_else(|| error("untrusted_worker_path"))?,
            );
        }
        if directories.iter().enumerate().any(|(i, left)| {
            directories
                .iter()
                .skip(i + 1)
                .any(|right| left.starts_with(right) || right.starts_with(left))
        }) {
            return Err(error("worker_directory_overlap"));
        }
        Ok(())
    }
    pub async fn build(self) -> Result<KubernetesWorker> {
        self.validate()?;
        let lock = directory_lock(&self.state_root, "kubernetes-worker.lock")?;
        let mut kubeconfig = pinned_kubernetes_config(
            &owned_file(&self.kubeconfig_file, 256 * 1024)?,
            &self.kube_context,
            &self.api_endpoint,
            &self.ca_sha256,
        )?;
        let identity = match (self.owned_token_file, self.identity) {
            (None, None) => None,
            (Some(path), Some(identity)) => Some(
                crate::kubernetes_identity::OwnedKubernetesIdentity::prepare(
                    &mut kubeconfig,
                    path,
                    identity,
                )
                .await?,
            ),
            _ => return Err(error("invalid_kubernetes_identity_configuration")),
        };
        let credential = String::from_utf8(owned_file(&self.credential_file, 256)?)
            .map_err(|_| error("invalid_credential"))?;
        let control = ControlClient::new(&self.control_url, credential.trim().into())?;
        let client = kube::Client::try_from(kubeconfig)
            .map_err(|_| error("kubernetes_client_unavailable"))?;
        let executor = KubernetesJobBuildExecutor::new(
            client.clone(),
            KubernetesBuildConfig {
                profile: KubernetesBuildProfile::RootlessIdmapSandbox,
                namespace: NAMESPACE.into(),
                service_account: SERVICE_ACCOUNT.into(),
                sandbox_runtime_class: RUNTIME_CLASS.into(),
                runner_image: self.runner_image,
                trusted_projects: self.trusted_projects.clone(),
                storage_cluster_id: self.storage.as_ref().map(|_| self.build_cluster_id.clone()),
                hold_execution: self.upload_gateway.is_some(),
            },
        )
        .map_err(|_| error("invalid_kubernetes_worker_configuration"))?;
        let policy: TransferPolicy =
            serde_json::from_slice(&owned_file(&self.transfer_policy_file, 16 * 1024)?)
                .map_err(|_| error("invalid_transfer_policy"))?;
        TransferClient::new(policy.clone())?;
        let endpoint = https_endpoint(&self.staging_storage.storage.endpoint)?;
        if !policy
            .allowed_origins
            .iter()
            .any(|origin| https_endpoint(origin).is_ok_and(|url| url.origin() == endpoint.origin()))
        {
            return Err(error("staging_origin_mismatch"));
        }
        let credentials =
            serde_json::from_slice(&owned_file(&self.staging_storage.credential_file, 8192)?)
                .map_err(|_| error("invalid_staging_credentials"))?;
        let staging = if let Some(ca) = &policy.certificate_authority_pem {
            spitzli_object_storage::Store::s3_with_ca(
                self.staging_storage.storage,
                credentials,
                ca.as_bytes(),
            )
        } else {
            spitzli_object_storage::Store::s3(self.staging_storage.storage, credentials)
        }
        .map_err(|_| error("invalid_staging_storage"))?;
        let upload_gateway = if let Some(config) = &self.upload_gateway {
            Some(crate::kubernetes_upload::AttemptUploadGateway::open(
                &config.root,
                config.filesystem_ceiling_bytes,
                &config.origin,
                policy.max_result_bytes,
                config.timeout_seconds,
            )?)
        } else {
            None
        };
        let transfer = Arc::new(if let Some(gateway) = &upload_gateway {
            KubernetesStagingTransfer::new_with_upload_gateway(
                staging,
                Arc::new(executor.clone()),
                policy,
                self.staging_root,
                gateway.clone(),
            )?
        } else {
            if !self.trusted_pilot_only {
                return Err(error("upload_gateway_required"));
            }
            KubernetesStagingTransfer::trusted_pilot_only_unbounded_provider_put(
                staging,
                Arc::new(executor.clone()),
                policy,
                self.staging_root,
            )?
        });
        let upload_listener = if let Some(config) = &self.upload_gateway {
            Some(
                tokio::net::TcpListener::bind(config.listen)
                    .await
                    .map_err(|_| error("upload_gateway_listener_unavailable"))?,
            )
        } else {
            None
        };
        let store = Arc::new(ArtifactStore::open(
            self.artifact_root,
            self.artifact_store_limit,
        )?);
        let publication = if let Some(config) = self.publication {
            Some(PublicationWorker::new(
                control.clone(),
                store.clone(),
                config,
                &self.trusted_projects,
            )?)
        } else {
            None
        };
        let storage = if let Some(storage) = self.storage {
            let material = owned_file(&storage.signing_key_file, 256)?;
            let seed: [u8; 32] = hex::decode(
                std::str::from_utf8(&material)
                    .map_err(|_| error("invalid_build_storage_signer"))?
                    .trim(),
            )
            .map_err(|_| error("invalid_build_storage_signer"))?
            .try_into()
            .map_err(|_| error("invalid_build_storage_signer"))?;
            Some(crate::build_storage::KubernetesBuildStorage::new(
                client,
                NAMESPACE.into(),
                storage.requests_namespace,
                storage.receipts_namespace,
                storage.target,
                ed25519_dalek::SigningKey::from_bytes(&seed),
            )?)
        } else {
            None
        };
        let mut dispatcher = KubernetesBuildDispatcher::new(
            control,
            executor,
            transfer,
            store,
            self.state_root,
            self.build_cluster_id,
        )?;
        if let Some(storage) = storage {
            dispatcher = dispatcher.with_storage(storage);
        }
        Ok(KubernetesWorker {
            _lock: lock,
            dispatcher,
            publication,
            poll_interval: Duration::from_secs(self.poll_seconds),
            upload_gateway,
            upload_listener,
            identity,
        })
    }
}
struct PublicationWorker {
    _lock: std::fs::File,
    control: ControlClient,
    store: Arc<ArtifactStore>,
    outbox: Outbox,
    publishers: BTreeMap<String, (String, Arc<RegistryPublisher>)>,
}
impl PublicationWorker {
    fn new(
        control: ControlClient,
        store: Arc<ArtifactStore>,
        config: PublicationConfig,
        trusted: &BTreeSet<TrustedProject>,
    ) -> Result<Self> {
        if config.projects.is_empty() || config.projects.len() > 256 {
            return Err(error("invalid_publication_configuration"));
        }
        let lock = directory_lock(&config.state_root, "kubernetes-publication.lock")?;
        let outbox = Outbox::open(config.state_root)?;
        let mut publishers = BTreeMap::new();
        for (project, policy) in config.projects {
            crate::contracts::id(&project)?;
            let targets: Vec<_> = trusted.iter().filter(|p| p.project_id == project).collect();
            let parts: Vec<_> = policy.repository.split('/').collect();
            if targets.len() != 1
                || parts.len() != 5
                || parts[1] != "customers"
                || parts[2] != targets[0].customer_id
                || parts[3] != "projects"
                || parts[4] != project
            {
                return Err(error("invalid_registry_scope"));
            }
            let auth: RegistrySecret =
                serde_json::from_slice(&owned_file(&policy.credential_file, 16384)?)
                    .map_err(|_| error("invalid_registry_credential"))?;
            if auth.username.is_empty()
                || auth.password.is_empty()
                || auth
                    .username
                    .chars()
                    .chain(auth.password.chars())
                    .any(char::is_control)
            {
                return Err(error("invalid_registry_credential"));
            }
            let publisher = RegistryPublisher::new(
                parts[0].into(),
                RegistryCredential::Basic(auth.username, auth.password),
            )?;
            publishers.insert(project, (policy.repository, Arc::new(publisher)));
        }
        Ok(Self {
            _lock: lock,
            control,
            store,
            outbox,
            publishers,
        })
    }
    async fn turn(&self) -> Result<bool> {
        publish_once_without_builder(
            &self.control,
            &self.store,
            &self.outbox,
            &self.publishers,
            &BTreeMap::new(),
        )
        .await
    }
    async fn run(self, poll: Duration, once: bool) -> Result<()> {
        loop {
            if let Err(error) = self.turn().await {
                if once || error.code == "worker_revoked" {
                    return Err(error);
                }
                eprintln!("{}", error.code);
            }
            if once {
                return Ok(());
            }
            tokio::time::sleep(poll).await;
        }
    }
}
pub struct KubernetesWorker {
    _lock: std::fs::File,
    dispatcher: KubernetesBuildDispatcher,
    publication: Option<PublicationWorker>,
    poll_interval: Duration,
    upload_gateway: Option<Arc<crate::kubernetes_upload::AttemptUploadGateway>>,
    upload_listener: Option<tokio::net::TcpListener>,
    identity: Option<Arc<crate::kubernetes_identity::OwnedKubernetesIdentity>>,
}
struct BackgroundTask(Option<tokio::task::JoinHandle<Result<()>>>);
impl Drop for BackgroundTask {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}
async fn background_finished(task: &mut BackgroundTask, failure: &'static str) -> Result<()> {
    match &mut task.0 {
        Some(task) => task.await.map_err(|_| error(failure))?,
        None => std::future::pending().await,
    }
}
impl KubernetesWorker {
    async fn stop_local_jobs(&self, result: Result<()>) -> Result<()> {
        // Keep the gateway alive until exact local attempts have been revoked/canceled.
        // This path does not contact Core to retire or purge their retained volumes.
        if let Err(cleanup) = self.dispatcher.cancel_local_on_shutdown().await {
            eprintln!("{}", cleanup.code);
            if result.is_ok() {
                return Err(cleanup);
            }
        }
        result
    }
    /// Stop claiming first, revoke/cancel exact local Jobs, then drop background services.
    /// Fsynced intents and retained volumes preserve uncertain operations for reconciliation.
    pub async fn run(
        mut self,
        once: bool,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<()> {
        if once && (self.upload_gateway.is_some() || self.upload_listener.is_some()) {
            return self
                .stop_local_jobs(Err(error("upload_gateway_requires_persistent_worker")))
                .await;
        }
        #[cfg(feature = "service")]
        let upload = if let (Some(gateway), Some(listener)) =
            (self.upload_gateway.clone(), self.upload_listener.take())
        {
            BackgroundTask(Some(tokio::spawn(async move {
                let router = crate::kubernetes_upload::router(gateway.clone());
                let server = axum::serve(listener, router).into_future();
                tokio::pin!(server);
                let mut expiry = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tokio::select! {
                        result = &mut server => return Err(error(if result.is_err() {"upload_gateway_server_failed"} else {"upload_gateway_server_stopped"})),
                        _ = expiry.tick() => gateway.reconcile_expired()?,
                    }
                }
            })))
        } else {
            BackgroundTask(None)
        };
        #[cfg(not(feature = "service"))]
        let upload = {
            if self.upload_gateway.is_some() || self.upload_listener.is_some() {
                return self
                    .stop_local_jobs(Err(error("upload_gateway_service_required")))
                    .await;
            }
            BackgroundTask(None)
        };
        let publication = BackgroundTask(
            self.publication
                .take()
                .map(|publisher| tokio::spawn(publisher.run(self.poll_interval, once))),
        );
        self.supervise(once, shutdown, upload, publication).await
    }
    async fn supervise(
        &mut self,
        once: bool,
        shutdown: impl std::future::Future<Output = ()>,
        mut upload: BackgroundTask,
        mut publication: BackgroundTask,
    ) -> Result<()> {
        tokio::pin!(shutdown);
        let mut identity = BackgroundTask(
            self.identity
                .clone()
                .map(|identity| tokio::spawn(identity.run())),
        );
        let result = async {
            if once {
                if let Some(identity) = &self.identity { identity.audit()?; }
                let result = tokio::select! {
                    biased;
                    done = background_finished(&mut identity, "kubernetes_identity_task_failed") => return done,
                    _ = &mut shutdown => return Ok(()),
                    done = background_finished(&mut upload, "upload_gateway_task_failed") => return done,
                    result = self.dispatcher.turn() => result,
                };
                result?;
                if publication.0.is_some() {
                    tokio::select! {
                    biased;
                    done = background_finished(&mut identity, "kubernetes_identity_task_failed") => return done,
                        _ = &mut shutdown => return Ok(()),
                        result = background_finished(&mut publication, "publication_task_failed") => result?,
                    }
                }
                return Ok(());
            }
            loop {
                if let Some(identity) = &self.identity { identity.audit()?; }
                let outcome = tokio::select! {
                    biased;
                    done = background_finished(&mut identity, "kubernetes_identity_task_failed") => return done,
                    _ = &mut shutdown => return Ok(()),
                    done = background_finished(&mut upload, "upload_gateway_task_failed") => return done,
                    done = background_finished(&mut publication, "publication_task_failed") => return done,
                    result = self.dispatcher.turn() => result,
                };
                if let Err(error) = outcome {
                    if error.code == "worker_revoked" {
                        return Err(error);
                    }
                    eprintln!("{}", error.code);
                }
                tokio::select! {
                    biased;
                    done = background_finished(&mut identity, "kubernetes_identity_task_failed") => return done,
                    _ = &mut shutdown => return Ok(()),
                    done = background_finished(&mut upload, "upload_gateway_task_failed") => return done,
                    done = background_finished(&mut publication, "publication_task_failed") => return done,
                    _ = tokio::time::sleep(self.poll_interval) => {},
                }
            }
        }.await;
        let result = self.stop_local_jobs(result).await;
        drop(identity);
        drop(publication);
        drop(upload);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;
    const CA: &str = include_str!("../tests/fixtures/kubernetes-worker-ca.crt");
    fn configuration() -> serde_json::Value {
        json!({"trustedPilotOnly":true,"controlURL":"https://control.example/private/build-worker","credentialFile":"/private/worker.token","kubeconfigFile":"/private/build.kubeconfig","kubeContext":"build-dispatcher","apiEndpoint":"https://build-api.example:6443","caSHA256":hex::encode(Sha256::digest(CA)),"buildClusterID":"1","runnerImage":format!("registry.example/runner@sha256:{}","a".repeat(64)),"trustedProjects":[{"customer_id":"1","project_id":"2"}],"stateRoot":"/private/state","artifactRoot":"/private/artifacts","stagingRoot":"/private/staging","transferPolicyFile":"/private/transfer-policy.json","stagingStorage":{"storage":{"id":"staging","endpoint":"https://objects.example","region":"eu-central-1","bucket":"build-staging","namespace":"worker-staging"},"credentialFile":"/private/s3.json"}})
    }
    fn kubeconfig() -> serde_json::Value {
        json!({"apiVersion":"v1","kind":"Config","current-context":"ignored-default","clusters":[{"name":"build","cluster":{"server":"https://build-api.example:6443","certificate-authority-data":base64::engine::general_purpose::STANDARD.encode(CA)}}],"users":[{"name":"dispatcher","user":{"token":"test-only-not-a-real-credential"}}],"contexts":[{"name":"build-dispatcher","context":{"cluster":"build","user":"dispatcher","namespace":"webdock-build-qualification"}}]})
    }
    #[test]
    fn hardened_factory_rejects_static_identity_even_with_storage_and_upload() {
        let mut value = configuration();
        value["enabled"] = json!(true);
        value["trustedPilotOnly"] = json!(false);
        value["storage"] = json!({"target":{"clusterID":"1","node":"owned-build","poolID":"123e4567-e89b-42d3-a456-426614174000"},"signingKeyFile":"/private/storage-signer","requestsNamespace":"webdock-build-storage-requests","receiptsNamespace":"webdock-build-storage-receipts"});
        value["uploadGateway"] = json!({"root":"/private/upload","filesystemCeilingBytes":1073741824_u64,"origin":"https://upload.invalid","listen":"127.0.0.1:3188","timeoutSeconds":60});
        let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            config.validate().unwrap_err().code,
            "managed_kubernetes_identity_required"
        );
    }
    #[test]
    fn owned_identity_is_opt_in_and_requires_both_explicit_inputs() {
        let config: KubernetesWorkerConfig = serde_json::from_value(configuration()).unwrap();
        assert!(config.owned_token_file.is_none() && config.identity.is_none());
        for fields in [
            json!({"ownedTokenFile":"/private/identity/token"}),
            json!({"identity":{"serviceAccountUID":"owned-sa-uid","revocationSecretUID":"owned-revocation-uid","audience":"https://kubernetes.default.svc.cluster.local"}}),
        ] {
            let mut value = configuration();
            value["enabled"] = json!(true);
            for (name, field) in fields.as_object().unwrap() {
                value[name] = field.clone();
            }
            let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
            assert_eq!(
                config.validate().unwrap_err().code,
                "invalid_kubernetes_identity_configuration"
            );
        }
    }
    #[tokio::test]
    async fn omitted_enable_is_disabled_before_any_credential_or_ambient_cluster_access() {
        let config: KubernetesWorkerConfig = serde_json::from_value(configuration()).unwrap();
        assert_eq!(
            config.build().await.err().unwrap().code,
            "kubernetes_worker_disabled"
        );
        let mut bad = configuration();
        bad["enabled"] = true.into();
        bad["execBackend"] = "/bin/sh".into();
        assert!(serde_json::from_value::<KubernetesWorkerConfig>(bad).is_err());
    }
    #[test]
    fn kube_factory_uses_only_explicit_context_endpoint_and_pinned_ca() {
        let bytes = kubeconfig().to_string();
        let ca = hex::encode(Sha256::digest(CA));
        let config = pinned_kubernetes_config(
            bytes.as_bytes(),
            "build-dispatcher",
            "https://build-api.example:6443",
            &ca,
        )
        .unwrap();
        assert_eq!(
            config.cluster_url.to_string(),
            "https://build-api.example:6443/"
        );
        assert_eq!(config.default_namespace, "webdock-build-qualification");
        assert!(config.proxy_url.is_none());
        assert!(!config.accept_invalid_certs);
        assert!(
            config
                .root_cert
                .as_ref()
                .is_some_and(|roots| roots.len() == 1)
        );
        assert_eq!(
            pinned_kubernetes_config(
                bytes.as_bytes(),
                "ambient-default",
                "https://build-api.example:6443",
                &ca
            )
            .err()
            .unwrap()
            .code,
            "invalid_kubernetes_context"
        );
        assert_eq!(
            pinned_kubernetes_config(
                bytes.as_bytes(),
                "build-dispatcher",
                "https://foreign.example:6443",
                &ca
            )
            .err()
            .unwrap()
            .code,
            "kubernetes_endpoint_mismatch"
        );
        assert_eq!(
            pinned_kubernetes_config(
                bytes.as_bytes(),
                "build-dispatcher",
                "https://build-api.example:6443",
                &"0".repeat(64)
            )
            .err()
            .unwrap()
            .code,
            "kubernetes_ca_mismatch"
        );
    }
    #[test]
    fn kube_factory_rejects_exec_proxy_admin_certificates_and_external_auth_files() {
        let ca = hex::encode(Sha256::digest(CA));
        for field in [
            json!({"exec":{"command":"/bin/sh","args":["-c","exit 9"]}}),
            json!({"client-certificate-data":"YQ==","client-key-data":"YQ=="}),
            json!({"tokenFile":"/private/other.token"}),
            json!({"auth-provider":{"name":"oidc"}}),
            json!({"as":"system:admin"}),
        ] {
            let mut kube = kubeconfig();
            let user = kube["users"][0]["user"].as_object_mut().unwrap();
            user.extend(field.as_object().unwrap().clone());
            assert_eq!(
                pinned_kubernetes_config(
                    kube.to_string().as_bytes(),
                    "build-dispatcher",
                    "https://build-api.example:6443",
                    &ca
                )
                .err()
                .unwrap()
                .code,
                "unsupported_kubernetes_auth"
            );
        }
        for field in [
            "proxy-url",
            "certificate-authority",
            "tls-server-name",
            "unexpected-option",
        ] {
            let mut kube = kubeconfig();
            kube["clusters"][0]["cluster"][field] = "forbidden".into();
            assert!(
                pinned_kubernetes_config(
                    kube.to_string().as_bytes(),
                    "build-dispatcher",
                    "https://build-api.example:6443",
                    &ca
                )
                .is_err()
            );
        }
        let mut kube = kubeconfig();
        kube["clusters"][0]["cluster"]["insecure-skip-tls-verify"] = true.into();
        assert!(
            pinned_kubernetes_config(
                kube.to_string().as_bytes(),
                "build-dispatcher",
                "https://build-api.example:6443",
                &ca
            )
            .is_err()
        );
    }
    #[test]
    fn owned_private_files_reject_readable_secrets_links_and_public_directories() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("private.json");
        std::fs::write(&file, b"{}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(owned_file(&file, 16).unwrap(), b"{}");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(owned_file(&file, 16).is_err());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = root.path().join("alias");
        symlink(&file, &alias).unwrap();
        assert!(owned_file(&alias, 16).is_err());
        std::fs::hard_link(&file, root.path().join("hardlink")).unwrap();
        assert!(owned_file(&file, 16).is_err());
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(private_directory(root.path()).is_ok());
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(root.path()).is_err());
    }
    fn private_json(path: &Path, value: &serde_json::Value) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, value.to_string()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn configured_files(origin: &str) -> (tempfile::TempDir, serde_json::Value) {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config = configuration();
        config["enabled"] = true.into();
        config["controlURL"] = format!("{origin}/private/build-worker").into();
        config["apiEndpoint"] = origin.into();
        for (field, name) in [
            ("stateRoot", "state"),
            ("artifactRoot", "artifacts"),
            ("stagingRoot", "staging"),
        ] {
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            config[field] = dir.to_str().unwrap().into();
        }
        let mut kube = kubeconfig();
        kube["clusters"][0]["cluster"]["server"] = origin.into();
        let path = root.path().join("kubeconfig.json");
        private_json(&path, &kube);
        config["kubeconfigFile"] = path.to_str().unwrap().into();
        let path = root.path().join("worker.token");
        std::fs::write(&path, "a".repeat(43)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        config["credentialFile"] = path.to_str().unwrap().into();
        let path = root.path().join("storage.json");
        private_json(
            &path,
            &json!({"accessKeyId":"TEST_ONLY_ACCESS","secretAccessKey":"TEST_ONLY_SECRET"}),
        );
        config["stagingStorage"]["credentialFile"] = path.to_str().unwrap().into();
        config["stagingStorage"]["storage"]["endpoint"] = origin.into();
        let path = root.path().join("policy.json");
        private_json(
            &path,
            &json!({"allowedOrigins":[origin],"certificateAuthorityPEM":CA,"maxSourceBytes":crate::kubernetes_transfer::MAX_TRANSFER_SOURCE,"maxResultBytes":crate::kubernetes_transfer::MAX_TRANSFER_RESULT,"timeoutSeconds":30}),
        );
        config["transferPolicyFile"] = path.to_str().unwrap().into();
        (root, config)
    }
    #[tokio::test]
    async fn enabled_factory_constructs_without_network_and_excludes_duplicate_daemons() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let (_root, value) = configured_files(&origin);
        let config: KubernetesWorkerConfig = serde_json::from_value(value.clone()).unwrap();
        let worker = tokio::time::timeout(Duration::from_secs(2), config.build())
            .await
            .unwrap()
            .unwrap();
        assert!(listener.accept().is_err());
        let config: KubernetesWorkerConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            config.build().await.err().unwrap().code,
            "kubernetes_worker_already_running"
        );
        drop(worker);
        let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
        assert!(config.build().await.is_ok());
        assert!(listener.accept().is_err());
    }
    #[tokio::test]
    async fn enabled_factory_requires_explicit_file_and_independent_staging_policy() {
        let (root, mut value) = configured_files("https://127.0.0.1:9");
        value["kubeconfigFile"] = root.path().join("missing-config").to_str().unwrap().into();
        let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            config.build().await.err().unwrap().code,
            "storage_unavailable"
        );
        let (root, value) = configured_files("https://127.0.0.1:9");
        let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
        private_json(
            &root.path().join("policy.json"),
            &json!({"allowedOrigins":["https://foreign.example"],"maxSourceBytes":1024,"maxResultBytes":1024,"timeoutSeconds":5}),
        );
        assert_eq!(
            config.build().await.err().unwrap().code,
            "staging_origin_mismatch"
        );
        let (_root, mut value) = configured_files("https://127.0.0.1:9");
        value["publication"] = json!({"stateRoot":value["artifactRoot"],"projects":{}});
        let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
        assert_eq!(
            config.build().await.err().unwrap().code,
            "worker_directory_overlap"
        );
    }
    #[tokio::test]
    async fn shutdown_interrupts_pending_control_io_and_independent_publication_task() {
        use std::os::unix::fs::PermissionsExt;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let (root, mut value) = configured_files(&origin);
        let publication = root.path().join("publication");
        std::fs::create_dir(&publication).unwrap();
        std::fs::set_permissions(&publication, std::fs::Permissions::from_mode(0o700)).unwrap();
        let credential = root.path().join("registry.json");
        private_json(
            &credential,
            &json!({"username":"TEST_ONLY","password":"TEST_ONLY_SECRET"}),
        );
        value["publication"] = json!({"stateRoot":publication,"projects":{"2":{"repository":"registry.example/customers/1/projects/2","credentialFile":credential}}});
        let config: KubernetesWorkerConfig = serde_json::from_value(value).unwrap();
        let worker = config.build().await.unwrap();
        assert!(worker.publication.is_some());
        tokio::time::timeout(
            Duration::from_secs(1),
            worker.run(false, tokio::time::sleep(Duration::from_millis(30))),
        )
        .await
        .unwrap()
        .unwrap();
    }
    #[tokio::test]
    async fn shutdown_does_not_skip_local_receipt_cancellation_failures() {
        let (root, value) = configured_files("https://127.0.0.1:9");
        let worker = serde_json::from_value::<KubernetesWorkerConfig>(value)
            .unwrap()
            .build()
            .await
            .unwrap();
        private_json(
            &root.path().join("state/attempt-3-1.json"),
            &json!({"version": 99}),
        );
        let result = worker.run(false, std::future::ready(())).await;
        assert_eq!(result.unwrap_err().code, "invalid_dispatch_receipt");
        assert!(root.path().join("state/attempt-3-1.json").exists());
    }

    #[tokio::test]
    async fn once_with_upload_listener_rejects_before_dispatch_or_serving() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (_root, value) = configured_files("https://127.0.0.1:9");
        let mut worker = serde_json::from_value::<KubernetesWorkerConfig>(value)
            .unwrap()
            .build()
            .await
            .unwrap();
        worker.upload_listener = Some(listener);
        let result = worker.run(true, std::future::pending()).await;
        assert_eq!(
            result.unwrap_err().code,
            "upload_gateway_requires_persistent_worker"
        );
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }

    #[cfg(not(feature = "service"))]
    #[tokio::test]
    async fn minimal_build_rejects_upload_server_before_credentials_or_directory_access() {
        let mut value = configuration();
        value["enabled"] = json!(true);
        value["uploadGateway"] = json!({"root":"/private/not-mounted","filesystemCeilingBytes":2147483648_u64,"origin":"https://upload.example","listen":"127.0.0.1:3188","timeoutSeconds":30});
        let config = serde_json::from_value::<KubernetesWorkerConfig>(value).unwrap();
        assert_eq!(
            config.build().await.err().unwrap().code,
            "invalid_upload_gateway_listener"
        );
    }

    #[derive(Default)]
    struct LocalAttempt {
        job: Option<serde_json::Value>,
        events: Vec<&'static str>,
    }
    struct LocalTransfer(Arc<std::sync::Mutex<LocalAttempt>>);
    #[async_trait::async_trait]
    impl crate::kubernetes_dispatcher::KubernetesArtifactTransfer for LocalTransfer {
        async fn revoke(&self, _: &crate::kubernetes::KubernetesBuildRequest) -> Result<()> {
            self.0.lock().unwrap().events.push("revoke");
            Ok(())
        }
        async fn stage(
            &self,
            _: &crate::kubernetes::KubernetesBuildRequest,
            _: &crate::execution::GuestRequest,
            _: &crate::contracts::ContainerSettings,
            _: &[u8],
            _: &str,
        ) -> Result<()> {
            panic!("shutdown must not stage new execution")
        }
        async fn collect(
            &self,
            _: &crate::kubernetes::KubernetesJobHandle,
        ) -> Result<crate::kubernetes_dispatcher::KubernetesTransferredOutput> {
            panic!("shutdown must not collect or complete an attempt")
        }
        async fn cleanup(&self, _: &crate::kubernetes::KubernetesBuildRequest) -> Result<()> {
            panic!("Core-unavailable shutdown must retain transfer and volume state")
        }
    }
    fn local_attempt_worker() -> (
        tempfile::TempDir,
        KubernetesWorker,
        Arc<std::sync::Mutex<LocalAttempt>>,
    ) {
        local_attempt_worker_with_control("https://127.0.0.1:9/private/build-worker")
    }
    fn local_attempt_worker_with_control(
        control_url: &str,
    ) -> (
        tempfile::TempDir,
        KubernetesWorker,
        Arc<std::sync::Mutex<LocalAttempt>>,
    ) {
        use http_body_util::{BodyExt, Full};
        use kube::client::Body;
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        for name in ["state", "artifacts"] {
            let path = root.path().join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let local = Arc::new(std::sync::Mutex::new(LocalAttempt::default()));
        let shared = local.clone();
        let client = kube::Client::new(
            tower::service_fn(move |request: http::Request<Body>| {
                let shared = shared.clone();
                async move {
                    let method = request.method().clone();
                    let path = request.uri().path().to_string();
                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                    let mut local = shared.lock().unwrap();
                    assert!(
                        local.events.contains(&"revoke"),
                        "API mutation before upload revocation"
                    );
                    let value = if method == http::Method::DELETE {
                        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(body["preconditions"]["uid"], "exact-local-job");
                        assert_eq!(body["preconditions"]["resourceVersion"], "7");
                        local.events.push("delete");
                        local.job = None;
                        json!({"kind":"Status","apiVersion":"v1","status":"Success","code":200})
                    } else if path.ends_with("/pods") {
                        json!({"kind":"PodList","apiVersion":"v1","metadata":{},"items":[]})
                    } else {
                        assert_eq!(method, http::Method::GET);
                        local.job.clone().unwrap()
                    };
                    Ok::<_, std::convert::Infallible>(http::Response::new(Full::new(
                        serde_json::to_vec(&value).unwrap().into(),
                    )))
                }
            }),
            "ignored-ambient-namespace",
        );
        let executor = KubernetesJobBuildExecutor::new(
            client,
            KubernetesBuildConfig {
                profile: KubernetesBuildProfile::RootlessIdmapSandbox,
                namespace: NAMESPACE.into(),
                service_account: SERVICE_ACCOUNT.into(),
                sandbox_runtime_class: RUNTIME_CLASS.into(),
                runner_image: format!("registry.example/runner@sha256:{}", "a".repeat(64)),
                trusted_projects: BTreeSet::from([TrustedProject {
                    customer_id: "1".into(),
                    project_id: "2".into(),
                }]),
                storage_cluster_id: None,
                hold_execution: false,
            },
        )
        .unwrap();
        let request = crate::kubernetes::KubernetesBuildRequest {
            customer_id: "1".into(),
            project_id: "2".into(),
            build_id: "3".into(),
            generation: 1,
            attempt: 1,
            commit_sha: "b".repeat(40),
            volume: None,
            limits: crate::kubernetes::KubernetesBuildLimits {
                cpu_millis: 1000,
                memory_mib: 512,
                workspace_mib: 512,
                cache_mib: 64,
                deadline_seconds: 30,
            },
        };
        let mut job = serde_json::to_value(executor.manifest(&request).unwrap()).unwrap();
        job["metadata"]["uid"] = json!("exact-local-job");
        job["metadata"]["resourceVersion"] = json!("7");
        local.lock().unwrap().job = Some(job);
        private_json(
            &root.path().join("state/attempt-3-1.json"),
            &json!({
                "version":1,"cluster_id":"1","runtime_fingerprint":executor.runtime_fingerprint(),"request":request,
                "spec_fingerprint":executor.fingerprint(&request).unwrap(),"source_archive_sha256":null,"uid":"exact-local-job",
                "logs":"","outcome":null,"completion":null,"acknowledged":false,"retired":false,
            }),
        );
        let store =
            Arc::new(ArtifactStore::open(root.path().join("artifacts"), store_limit()).unwrap());
        let dispatcher = KubernetesBuildDispatcher::new(
            ControlClient::new(control_url, "a".repeat(43)).unwrap(),
            executor,
            Arc::new(LocalTransfer(local.clone())),
            store,
            root.path().join("state"),
            "1".into(),
        )
        .unwrap();
        let worker = KubernetesWorker {
            _lock: directory_lock(root.path(), "worker.lock").unwrap(),
            dispatcher,
            publication: None,
            poll_interval: Duration::from_secs(1),
            upload_gateway: None,
            upload_listener: None,
            identity: None,
        };
        (root, worker, local)
    }
    fn retained_canceled_attempt(root: &Path, local: &std::sync::Mutex<LocalAttempt>) {
        let local = local.lock().unwrap();
        assert!(local.job.is_none());
        assert_eq!(local.events.last(), Some(&"delete"));
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("state/attempt-3-1.json")).unwrap())
                .unwrap();
        assert_eq!(receipt["uid"], "exact-local-job");
        assert_eq!(receipt["outcome"], "control_unavailable");
        assert_eq!(receipt["retired"], false);
        assert_eq!(receipt["volume_retired"], false);
    }
    #[tokio::test]
    async fn successful_identity_rotation_keeps_worker_and_existing_job_alive() {
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "https://{}/private/build-worker",
            listener.local_addr().unwrap()
        );
        let (root, mut worker, local) = local_attempt_worker_with_control(&endpoint);
        let (_identity_root, identity, count) =
            crate::kubernetes_identity::tests::verified_fixture().await;
        worker.identity = Some(identity);
        let renewals_before = count.load(Ordering::SeqCst);
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(worker.run(false, async {
            let _ = shutdown.await;
        }));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            count.load(Ordering::SeqCst) > renewals_before,
            "renewal task was not independently supervised"
        );
        assert!(
            !task.is_finished(),
            "successful rotation stopped the worker"
        );
        assert!(
            local.lock().unwrap().events.is_empty(),
            "rotation canceled an existing attempt"
        );
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        retained_canceled_attempt(root.path(), &local);
    }

    #[tokio::test]
    async fn unsafe_identity_file_interrupts_pending_dispatch_and_cancels_exact_attempt() {
        use std::os::unix::fs::PermissionsExt;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "https://{}/private/build-worker",
            listener.local_addr().unwrap()
        );
        let (root, mut worker, local) = local_attempt_worker_with_control(&endpoint);
        let (identity_root, identity, _) =
            crate::kubernetes_identity::tests::verified_fixture().await;
        worker.identity = Some(identity);
        let task = tokio::spawn(worker.run(false, std::future::pending()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::set_permissions(
            identity_root.path().join("identity/token"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let failure = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(failure.code, "kubernetes_identity_file_untrusted");
        assert!(failure.logs.is_empty());
        retained_canceled_attempt(root.path(), &local);
    }

    #[tokio::test]
    async fn shutdown_revokes_and_deletes_exact_local_job_without_core_or_retirement() {
        let (root, worker, local) = local_attempt_worker();
        worker.run(false, std::future::ready(())).await.unwrap();
        retained_canceled_attempt(root.path(), &local);
    }
    #[tokio::test]
    async fn shutdown_keeps_upload_task_alive_until_exact_job_cancellation_finishes() {
        struct ServiceLifetime(Arc<std::sync::Mutex<LocalAttempt>>);
        impl Drop for ServiceLifetime {
            fn drop(&mut self) {
                self.0.lock().unwrap().events.push("gateway_stopped");
            }
        }
        let (root, mut worker, local) = local_attempt_worker();
        let shared = local.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _service = ServiceLifetime(shared);
            started.send(()).unwrap();
            std::future::pending::<Result<()>>().await
        });
        ready.await.unwrap();
        let finished = task.abort_handle();
        worker
            .supervise(
                false,
                std::future::ready(()),
                BackgroundTask(Some(task)),
                BackgroundTask(None),
            )
            .await
            .unwrap();
        while !finished.is_finished() {
            tokio::task::yield_now().await;
        }
        let events = &local.lock().unwrap().events;
        assert!(
            events.iter().position(|e| *e == "delete").unwrap()
                < events.iter().position(|e| *e == "gateway_stopped").unwrap()
        );
        let receipt: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("state/attempt-3-1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["retired"], false);
    }
    #[tokio::test]
    async fn gateway_listener_expiry_and_publisher_failures_cancel_exact_local_job() {
        for code in [
            "upload_gateway_server_failed",
            "upload_gateway_unavailable",
            "worker_revoked",
        ] {
            let (root, mut worker, local) = local_attempt_worker();
            let task = BackgroundTask(Some(tokio::spawn(async move { Err(error(code)) })));
            while !task.0.as_ref().unwrap().is_finished() {
                tokio::task::yield_now().await;
            }
            let (upload, publication) = if code == "worker_revoked" {
                (BackgroundTask(None), task)
            } else {
                (task, BackgroundTask(None))
            };
            assert_eq!(
                worker
                    .supervise(false, std::future::pending(), upload, publication)
                    .await
                    .unwrap_err()
                    .code,
                code
            );
            retained_canceled_attempt(root.path(), &local);
        }
    }
    #[tokio::test]
    async fn fatal_background_error_survives_cleanup_failure_and_retains_receipt() {
        let (root, mut worker, _local) = local_attempt_worker();
        private_json(
            &root.path().join("state/attempt-3-1.json"),
            &json!({"version": 99}),
        );
        let upload = BackgroundTask(Some(tokio::spawn(async {
            Err(error("upload_gateway_server_failed"))
        })));
        let result = worker
            .supervise(false, std::future::pending(), upload, BackgroundTask(None))
            .await;
        assert_eq!(result.unwrap_err().code, "upload_gateway_server_failed");
        assert!(root.path().join("state/attempt-3-1.json").exists());
    }
}
