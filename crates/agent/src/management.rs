#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{0}")]
    Configuration(&'static str),
    #[error("Durable agent identity unavailable")]
    State,
    #[error("Agent enrollment revoked")]
    Revoked,
    #[error("Authenticated control connection unavailable")]
    Transport,
    #[error("Database delivery remains pending")]
    DatabasePending,
    #[error("Mail delivery remains pending")]
    MailPending,
    #[error("Mail configuration delivery remains pending")]
    ConfigurationPending,
    #[error("Recovery delivery remains pending")]
    RecoveryPending,
    #[error("Hosting delivery remains pending")]
    HostingPending,
    #[error("Plugin runtime delivery remains pending")]
    PluginPending,
    #[error(transparent)]
    Execution(#[from] webdock_supervisor::ExecutionError),
}
pub type Result<T> = std::result::Result<T, AgentError>;
pub use webdock_agent_protocol as wire;
#[cfg(feature = "kubernetes")]
mod database;
#[cfg(feature = "kubernetes")]
mod plugin;
#[cfg(feature = "kubernetes")]
mod workloads;
#[cfg(feature = "kubernetes")]
pub use database::database_claim;
#[cfg(feature = "kubernetes")]
pub use plugin::plugin_claim;
#[cfg(test)]
mod guard_tests;
mod mail;
pub use mail::mail_claim;
mod mail_configuration;
#[cfg(feature = "docker")]
mod recovery;
pub use mail_configuration::configuration_claim;

use crate::config::{AgentConfig, ControlEndpoint, IdentityState, private_directory, read_private};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};
use tonic::{
    Request, Status,
    transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity},
};
use webdock_supervisor::{
    Action, Authorizer, Command, ExecutionError, Executor, Journal, ReadCommand, ReadKind,
    RuntimeAdapter, timestamp,
};
use wire::agent_control_client::AgentControlClient;

struct Credentials {
    stamp: String,
    certificate_sha256: String,
    expires_at: Option<u64>,
    tls: Option<ClientTlsConfig>,
}
impl Credentials {
    fn load(endpoint: &ControlEndpoint) -> Result<Self> {
        match endpoint {
            ControlEndpoint::Unix { .. } => Ok(Self {
                stamp: "unix".into(),
                certificate_sha256: "unix".into(),
                expires_at: None,
                tls: None,
            }),
            ControlEndpoint::Mtls {
                domain,
                ca,
                certificate,
                key,
                ..
            } => {
                let ca = read_private(ca, 256 * 1024)?;
                let cert = read_private(certificate, 256 * 1024)?;
                let key = read_private(key, 256 * 1024)?;
                let (_, pem) = x509_parser::pem::parse_x509_pem(&cert)
                    .map_err(|_| AgentError::Configuration("Invalid client certificate"))?;
                let parsed = pem
                    .parse_x509()
                    .map_err(|_| AgentError::Configuration("Invalid client certificate"))?;
                let now = i64::try_from(timestamp()?)
                    .map_err(|_| AgentError::Configuration("Clock unavailable"))?;
                if parsed.validity().not_before.timestamp() > now
                    || parsed.validity().not_after.timestamp() <= now
                {
                    return Err(AgentError::Configuration(
                        "Client certificate is not currently valid",
                    ));
                }
                let expires_at = u64::try_from(parsed.validity().not_after.timestamp())
                    .map_err(|_| AgentError::Configuration("Invalid certificate lifetime"))?;
                let certificate_sha256 = format!("{:x}", Sha256::digest(&pem.contents));
                let mut hash = Sha256::new();
                for part in [&ca, &cert, &key] {
                    hash.update((part.len() as u64).to_be_bytes());
                    hash.update(part);
                }
                let stamp = format!("{:x}", hash.finalize());
                Ok(Self {
                    stamp,
                    certificate_sha256,
                    expires_at: Some(expires_at),
                    tls: Some(
                        ClientTlsConfig::new()
                            .domain_name(domain)
                            .ca_certificate(Certificate::from_pem(ca))
                            .identity(Identity::from_pem(cert, key)),
                    ),
                })
            }
        }
    }
}
#[derive(Clone)]
struct Session {
    #[cfg(feature = "docker")]
    recovery_client: wire::recovery_control_client::RecoveryControlClient<Channel>,
    client: AgentControlClient<Channel>,
    mail_client: wire::mail_control_client::MailControlClient<Channel>,
    configuration_client:
        wire::mail_configuration_control_client::MailConfigurationControlClient<Channel>,
    configuration_available: Arc<AtomicBool>,
    #[cfg(feature = "kubernetes")]
    database_client: wire::database_control_client::DatabaseControlClient<Channel>,
    #[cfg(feature = "kubernetes")]
    workload_client: wire::workload_control_client::WorkloadControlClient<Channel>,
    #[cfg(feature = "kubernetes")]
    plugin_client: wire::plugin_runtime_control_client::PluginRuntimeControlClient<Channel>,
    #[cfg(feature = "kubernetes")]
    plugin_domain: Arc<std::sync::atomic::AtomicU8>,
    #[cfg(feature = "kubernetes")]
    plugin_spec_version: Arc<std::sync::atomic::AtomicU8>,
    identity: wire::AgentIdentity,
    endpoint: ControlEndpoint,
    stamp: String,
    expires_at: Option<u64>,
    revoked: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
}
impl Session {
    fn valid(&self) -> Result<()> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(AgentError::Revoked);
        }
        if self.disconnected.load(Ordering::SeqCst)
            || self
                .expires_at
                .is_some_and(|v| timestamp().map_or(true, |now| v <= now))
        {
            return Err(AgentError::Transport);
        }
        if Credentials::load(&self.endpoint)?.stamp != self.stamp {
            return Err(AgentError::Transport);
        }
        Ok(())
    }
    fn rpc_error(&self, status: Status) -> AgentError {
        if matches!(
            status.code(),
            tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
        ) {
            self.revoked.store(true, Ordering::SeqCst);
            AgentError::Revoked
        } else {
            self.disconnected.store(true, Ordering::SeqCst);
            AgentError::Transport
        }
    }
}
fn request<T>(body: T) -> Request<T> {
    let mut request = Request::new(body);
    request.set_timeout(Duration::from_secs(5));
    request
}
fn accepted(ack: wire::Ack) -> Result<()> {
    if ack.accepted {
        Ok(())
    } else {
        Err(AgentError::Transport)
    }
}
struct RemoteAuthority {
    #[cfg(feature = "docker")]
    recovery_executor: std::sync::OnceLock<Arc<crate::recovery::NativeRecoveryExecutor>>,
    #[cfg(feature = "docker")]
    recovery_outbox: Mutex<crate::outbox::RecoveryOutbox>,
    #[cfg(feature = "kubernetes")]
    plugin_outbox: Mutex<crate::outbox::PluginRuntimeOutbox>,
    #[cfg(feature = "kubernetes")]
    plugin_retry_at: Mutex<Option<tokio::time::Instant>>,
    #[cfg(feature = "kubernetes")]
    plugin_checkpoint: Mutex<Option<wire::plugin::PluginRuntimeCheckpoint>>,
    session: Arc<RwLock<Option<Session>>>,
    mail_outbox: Mutex<crate::outbox::MailOutbox>,
    mail_retry_at: Mutex<Option<tokio::time::Instant>>,
    configuration_outbox: Mutex<crate::outbox::MailConfigurationOutbox>,
    configuration_retry_at: Mutex<Option<tokio::time::Instant>>,
    configuration_transient: AtomicBool,
    #[cfg(feature = "kubernetes")]
    database_outbox: Mutex<crate::outbox::DatabaseOutbox>,
    #[cfg(feature = "kubernetes")]
    database_retry_at: Mutex<Option<tokio::time::Instant>>,
}
impl RemoteAuthority {
    async fn guard(
        &self,
        operation_id: String,
        generation: u64,
        lease_until: u64,
        authorization: String,
        intent: wire::Intent,
        fingerprint: String,
    ) -> webdock_supervisor::Result<()> {
        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(ExecutionError::Denied)?;
        session.valid().map_err(|_| ExecutionError::Denied)?;
        if lease_until <= timestamp()? || lease_until.saturating_sub(timestamp()?) > 300 {
            return Err(ExecutionError::Expired);
        }
        let result = session
            .client
            .clone()
            .guard(request(wire::GuardRequest {
                identity: Some(session.identity.clone()),
                operation_id,
                generation,
                lease_until,
                authorization,
                intent: intent as i32,
                request_sha256: fingerprint,
            }))
            .await
            .map_err(|e| {
                session.rpc_error(e);
                ExecutionError::Denied
            })?
            .into_inner();
        session.valid().map_err(|_| ExecutionError::Denied)?;
        if lease_until <= timestamp()? {
            return Err(ExecutionError::Expired);
        }
        if result.allowed {
            Ok(())
        } else {
            Err(ExecutionError::Denied)
        }
    }
}
#[async_trait::async_trait]
impl Authorizer for RemoteAuthority {
    async fn authorize(&self, c: &Command) -> webdock_supervisor::Result<()> {
        self.guard(
            c.operation_id.clone(),
            c.generation,
            c.lease_until,
            c.authorization.clone(),
            match c.action {
                Action::Apply => wire::Intent::Apply,
                Action::Stop => wire::Intent::Stop,
                Action::Retire => wire::Intent::Retire,
                Action::PurgeStorage => wire::Intent::PurgeStorage,
            },
            c.fingerprint()?,
        )
        .await
    }
    async fn authorize_read(&self, c: &ReadCommand) -> webdock_supervisor::Result<()> {
        self.guard(
            c.operation_id.clone(),
            c.generation,
            c.lease_until,
            c.authorization.clone(),
            match c.kind {
                ReadKind::Inspect => wire::Intent::Inspect,
                ReadKind::Logs => wire::Intent::Logs,
                ReadKind::Metrics => wire::Intent::Metrics,
            },
            c.fingerprint()?,
        )
        .await
    }
}

pub struct Daemon {
    #[cfg(feature = "docker")]
    recovery_runtime: Option<Arc<crate::recovery::NativeRecoveryExecutor>>,
    #[cfg(feature = "kubernetes")]
    plugin_runtime: Option<Arc<dyn crate::plugin_runtime::PluginRuntimeExecutor>>,
    config: AgentConfig,
    state: Mutex<IdentityState>,
    outbox: Mutex<crate::outbox::ReceiptOutbox>,
    authority: Arc<RemoteAuthority>,
    adapter: Arc<dyn RuntimeAdapter>,
    executor: Executor,
    mail_runtime: Option<Arc<dyn crate::mail::MailRuntime>>,
    configuration_runtime: Option<Arc<dyn crate::mail::configuration::ConfigurationRuntime>>,
    #[cfg(feature = "kubernetes")]
    database_runtime: Option<Arc<dyn crate::database::DatabaseRuntime>>,
    #[cfg(feature = "kubernetes")]
    workload_runtime: Option<Arc<crate::workloads::WorkloadExecutor>>,
}
impl Daemon {
    /// Public injection supports independent adapters; authority always comes from authenticated Core.
    pub fn with_adapter(config: AgentConfig, adapter: Arc<dyn RuntimeAdapter>) -> Result<Self> {
        config.validate()?;
        let credentials = Credentials::load(&config.control)?;
        let journal = Journal::open(config.state_directory.join("commands.json"))?;
        let state = IdentityState::open(&config, &credentials.certificate_sha256)?;
        let outbox =
            crate::outbox::ReceiptOutbox::open(config.state_directory.join("receipt-outbox.json"))?;
        let authority = Arc::new(RemoteAuthority {
            #[cfg(feature = "docker")]
            recovery_executor: std::sync::OnceLock::new(),
            #[cfg(feature = "docker")]
            recovery_outbox: Mutex::new(crate::outbox::RecoveryOutbox::open(
                config.state_directory.join("recovery-outbox.json"),
            )?),
            #[cfg(feature = "kubernetes")]
            plugin_outbox: Mutex::new(crate::outbox::PluginRuntimeOutbox::open(
                config.state_directory.join("plugin-runtime-outbox.json"),
            )?),
            #[cfg(feature = "kubernetes")]
            plugin_retry_at: Mutex::new(None),
            #[cfg(feature = "kubernetes")]
            plugin_checkpoint: Mutex::new(None),
            session: Arc::new(RwLock::new(None)),
            mail_outbox: Mutex::new(crate::outbox::MailOutbox::open(
                config.state_directory.join("mail-outbox.json"),
            )?),
            mail_retry_at: Mutex::new(None),
            configuration_outbox: Mutex::new(crate::outbox::MailConfigurationOutbox::open(
                config
                    .state_directory
                    .join("mail-configuration-outbox.json"),
            )?),
            configuration_retry_at: Mutex::new(None),
            configuration_transient: AtomicBool::new(false),
            #[cfg(feature = "kubernetes")]
            database_outbox: Mutex::new(crate::outbox::DatabaseOutbox::open(
                config.state_directory.join("database-outbox.json"),
            )?),
            #[cfg(feature = "kubernetes")]
            database_retry_at: Mutex::new(None),
        });
        let executor = Executor::new(journal, authority.clone(), adapter.clone());
        Ok(Self {
            #[cfg(feature = "docker")]
            recovery_runtime: None,
            #[cfg(feature = "kubernetes")]
            plugin_runtime: None,
            config,
            state: Mutex::new(state),
            outbox: Mutex::new(outbox),
            authority,
            adapter,
            executor,
            mail_runtime: None,
            configuration_runtime: None,
            #[cfg(feature = "kubernetes")]
            database_runtime: None,
            #[cfg(feature = "kubernetes")]
            workload_runtime: None,
        })
    }
    pub async fn open(config: AgentConfig) -> Result<Self> {
        validate_configuration(&config)?;
        let policy = Arc::new(crate::policy::RuntimePolicy::load(config.policy.clone())?);
        match &config.runtime {
            #[cfg(feature = "docker")]
            crate::config::RuntimeConfiguration::Docker {
                socket,
                initializer_image,
                storage_socket,
            } => {
                let app_fences =
                    Arc::new(crate::recovery::effect_fence::AppEffectFenceRegistry::open(
                        config.state_directory.join("app-effects"),
                    )?);
                let adapter = crate::docker::DockerAdapter::connect(
                    socket,
                    config.installation.clone(),
                    initializer_image.clone(),
                )
                .await?
                .with_policy(policy.clone())
                .with_app_fences(app_fences.clone());
                let adapter = Arc::new(if let Some(socket) = storage_socket {
                    adapter.with_storage_socket(socket.clone()).await?
                } else {
                    adapter
                });
                let mail = if let Some(path) = &config.mail_runtime {
                    let mail: crate::mail::docker::DockerMailConfig =
                        serde_json::from_slice(&read_private(path, 65536)?).map_err(|_| {
                            AgentError::Configuration("Invalid Mail runtime configuration")
                        })?;
                    Some(
                        crate::mail::docker::DockerMailRuntime::connect(
                            socket,
                            mail.image,
                            mail.installation,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                let mut daemon = Self::with_adapter(config, adapter)?;
                if let Some(path) = &daemon.config.recovery_runtime {
                    let runtime = Arc::new(
                        crate::recovery::NativeRecoveryExecutor::open_with_app_fences(
                            path.clone(),
                            app_fences,
                        )
                        .await?,
                    );
                    #[cfg(feature = "kubernetes")]
                    if let Some(workloads) = &mut daemon.workload_runtime {
                        Arc::get_mut(workloads).ok_or(AgentError::Configuration("Recovery workload effect fence must be installed before sharing the executor"))?.set_mail_fence(runtime.mail_effect_fence());
                    }
                    daemon
                        .authority
                        .recovery_executor
                        .set(runtime.clone())
                        .map_err(|_| AgentError::State)?;
                    daemon.recovery_runtime = Some(runtime);
                }
                if let Some(runtime) = mail {
                    let runtime = Arc::new(runtime);
                    daemon.mail_runtime = Some(runtime.clone());
                    if daemon.config.mail_configuration_enabled {
                        daemon.configuration_runtime = Some(runtime);
                    }
                }
                Ok(daemon)
            }
            #[cfg(feature = "kubernetes")]
            crate::config::RuntimeConfiguration::Kubernetes {
                kubeconfig,
                context,
                helm,
                chart,
                chart_sha256,
                storage_socket,
                storage_node,
                public_routing,
            } => {
                let adapter = crate::kubernetes::KubernetesAdapter::connect(
                    kubeconfig.clone(),
                    context.clone(),
                    config.installation.clone(),
                    helm.clone(),
                    crate::kubernetes::ApprovedChart::verify(chart, chart_sha256)?,
                )
                .await?
                .with_policy(policy.clone());
                let adapter = adapter.with_public_routing(public_routing.clone())?;
                let database_runtime = config
                    .database_runtime
                    .as_ref()
                    .map(|path| {
                        crate::database::NativeDatabaseExecutor::open(
                            adapter.database_client(),
                            path.clone(),
                        )
                    })
                    .transpose()?;
                let plugin_runtime = config.plugin_runtime_enabled.then(|| {
                    Arc::new(crate::plugin_runtime::KubernetesPluginRuntime::new(
                        adapter.database_client(),
                        config.installation.clone(),
                    )) as Arc<dyn crate::plugin_runtime::PluginRuntimeExecutor>
                });
                let workload_runtime = config.workloads.then(|| {
                    Arc::new(crate::workloads::WorkloadExecutor::new(
                        adapter.database_client(),
                    ))
                });
                let adapter = Arc::new(if let Some(socket) = storage_socket {
                    adapter
                        .with_storage_node(socket.clone(), storage_node.as_deref())
                        .await?
                } else {
                    adapter
                });
                let mail_runtime = config
                    .mail_runtime
                    .as_ref()
                    .map(|path| adapter.mail_runtime(path.clone()))
                    .transpose()?;
                let mut daemon = Self::with_adapter(config, adapter)?;
                if let Some(runtime) = mail_runtime {
                    let runtime = Arc::new(runtime);
                    daemon.mail_runtime = Some(runtime.clone());
                    if daemon.config.mail_configuration_enabled {
                        daemon.configuration_runtime = Some(runtime);
                    }
                }
                daemon.database_runtime = database_runtime
                    .map(|runtime| Arc::new(runtime) as Arc<dyn crate::database::DatabaseRuntime>);
                daemon.workload_runtime = workload_runtime;
                daemon.plugin_runtime = plugin_runtime;
                #[cfg(feature = "docker")]
                if let Some(path) = &daemon.config.recovery_runtime {
                    let runtime = Arc::new(
                        crate::recovery::NativeRecoveryExecutor::open(path.clone()).await?,
                    );
                    if let Some(workloads) = &mut daemon.workload_runtime {
                        Arc::get_mut(workloads).ok_or(AgentError::Configuration("Recovery workload effect fence must be installed before sharing the executor"))?.set_mail_fence(runtime.mail_effect_fence());
                    }
                    daemon
                        .authority
                        .recovery_executor
                        .set(runtime.clone())
                        .map_err(|_| AgentError::State)?;
                    daemon.recovery_runtime = Some(runtime);
                }
                Ok(daemon)
            }
            #[cfg(not(all(feature = "docker", feature = "kubernetes")))]
            _ => {
                drop(policy);
                Err(AgentError::Configuration(
                    "Runtime adapter is not included in this build",
                ))
            }
        }
    }
    pub async fn connect(&mut self) -> Result<()> {
        *self.authority.session.write().await = None;
        let credentials = Credentials::load(&self.config.control)?;
        let endpoint = match &self.config.control {
            ControlEndpoint::Unix { path } => {
                private_directory(
                    path.parent()
                        .ok_or(AgentError::Configuration("Socket directory unavailable"))?,
                )?;
                let metadata =
                    std::fs::symlink_metadata(path).map_err(|_| AgentError::Transport)?;
                if !metadata.file_type().is_socket()
                    || metadata.uid() != rustix::process::geteuid().as_raw()
                    || metadata.mode() & 0o077 != 0
                {
                    return Err(AgentError::Configuration("Untrusted control socket"));
                }
                Endpoint::from_shared(format!(
                    "unix://{}",
                    path.to_str()
                        .ok_or(AgentError::Configuration("Socket path"))?
                ))
                .map_err(|_| AgentError::Configuration("Socket path"))?
            }
            ControlEndpoint::Mtls { origin, .. } => Endpoint::from_shared(origin.clone())
                .map_err(|_| AgentError::Configuration("Control origin"))?
                .tls_config(
                    credentials
                        .tls
                        .ok_or(AgentError::Configuration("TLS configuration"))?,
                )
                .map_err(|_| AgentError::Configuration("TLS identity"))?,
        };
        let channel = endpoint
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(5))
            .http2_keep_alive_interval(Duration::from_secs(15))
            .keep_alive_timeout(Duration::from_secs(5))
            .connect()
            .await
            .map_err(|_| AgentError::Transport)?;
        let client = AgentControlClient::new(channel.clone())
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES);
        let session = Session {
            #[cfg(feature = "docker")]
            recovery_client: wire::recovery_control_client::RecoveryControlClient::new(
                channel.clone(),
            )
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
            client,
            configuration_client:
                wire::mail_configuration_control_client::MailConfigurationControlClient::new(
                    channel.clone(),
                )
                .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
                .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
            configuration_available: Arc::new(AtomicBool::new(true)),
            #[cfg(feature = "kubernetes")]
            plugin_client: wire::plugin_runtime_control_client::PluginRuntimeControlClient::new(
                channel.clone(),
            )
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
            #[cfg(feature = "kubernetes")]
            plugin_domain: Arc::new(std::sync::atomic::AtomicU8::new(plugin::DOMAIN_UNKNOWN)),
            #[cfg(feature = "kubernetes")]
            plugin_spec_version: Arc::new(std::sync::atomic::AtomicU8::new(1)),
            mail_client: wire::mail_control_client::MailControlClient::new(channel.clone())
                .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
                .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
            #[cfg(feature = "kubernetes")]
            workload_client: wire::workload_control_client::WorkloadControlClient::new(
                channel.clone(),
            )
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
            #[cfg(feature = "kubernetes")]
            database_client: wire::database_control_client::DatabaseControlClient::new(channel)
                .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
                .max_encoding_message_size(wire::MAX_MESSAGE_BYTES),
            identity: self.config.identity(),
            endpoint: self.config.control.clone(),
            stamp: credentials.stamp,
            expires_at: credentials.expires_at,
            revoked: Arc::new(AtomicBool::new(false)),
            disconnected: Arc::new(AtomicBool::new(false)),
        };
        let hello = wire::Hello {
            identity: Some(session.identity.clone()),
            protocol_version: 1,
            certificate_sha256: if matches!(self.config.control, ControlEndpoint::Unix { .. }) {
                String::new()
            } else {
                credentials.certificate_sha256.clone()
            },
            capabilities_json: serde_json::to_vec(&self.adapter.capabilities())
                .map_err(|_| AgentError::Configuration("Capabilities"))?,
        };
        let welcome = match session.client.clone().register(request(hello)).await {
            Ok(value) => value.into_inner(),
            Err(status) => {
                let error = session.rpc_error(status);
                if matches!(error, AgentError::Revoked) {
                    self.state.lock().await.revoke()?;
                }
                return Err(error);
            }
        };
        if !welcome.accepted
            || welcome.protocol_version != 1
            || welcome.generation != self.config.generation
        {
            self.state.lock().await.revoke()?;
            return Err(AgentError::Revoked);
        }
        // An operator-approved certificate rotation retains sequence and re-enrolls the new identity.
        *self.state.lock().await =
            IdentityState::open(&self.config, &credentials.certificate_sha256)?;
        *self.authority.session.write().await = Some(session);
        let delivered = async {
            self.hosting_delivery_ready().await?;
            self.mail_delivery_ready().await?;
            self.configuration_delivery_ready().await?;
            #[cfg(feature = "kubernetes")]
            self.database_delivery_ready().await?;
            #[cfg(feature = "kubernetes")]
            self.plugin_delivery_ready().await?;
            Ok(())
        }
        .await;
        if let Err(error) = &delivered {
            if matches!(error, AgentError::Revoked) {
                self.state.lock().await.revoke()?;
            }
            *self.authority.session.write().await = None;
        }
        delivered
    }
    async fn session(&self) -> Result<Session> {
        let value = self
            .authority
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::Transport)?;
        value.valid()?;
        Ok(value)
    }
    async fn flush_receipt(&self) -> Result<()> {
        let pending = self.outbox.lock().await.pending()?;
        if let Some(receipt) = pending {
            let session = self.session().await?;
            if receipt.identity.as_ref() != Some(&session.identity) {
                return Err(AgentError::HostingPending);
            }
            // This is delivery of a committed result, never authorization to execute again.
            let result = session
                .client
                .clone()
                .complete(request(receipt.clone()))
                .await;
            match result {
                Ok(response) if response.get_ref().accepted => {}
                Err(status)
                    if matches!(
                        status.code(),
                        tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
                    ) =>
                {
                    return Err(session.rpc_error(status));
                }
                _ => return Err(AgentError::HostingPending),
            }
            self.outbox.lock().await.acknowledge(&receipt)?;
        }
        Ok(())
    }
    async fn hosting_delivery_ready(&self) -> Result<bool> {
        match self.flush_receipt().await {
            Ok(()) => Ok(true),
            Err(AgentError::HostingPending) => Ok(false),
            Err(error) => Err(error),
        }
    }
    async fn has_pending_delivery(&self) -> Result<bool> {
        if self.outbox.lock().await.pending()?.is_some() {
            return Ok(true);
        }
        if self.authority.mail_outbox.lock().await.pending()?.is_some() {
            return Ok(true);
        }
        if self
            .authority
            .configuration_outbox
            .lock()
            .await
            .pending()?
            .is_some()
        {
            return Ok(true);
        }
        #[cfg(feature = "docker")]
        if self
            .authority
            .recovery_outbox
            .lock()
            .await
            .pending()?
            .is_some()
        {
            return Ok(true);
        }
        #[cfg(feature = "kubernetes")]
        if self
            .authority
            .plugin_outbox
            .lock()
            .await
            .pending()?
            .is_some()
        {
            return Ok(true);
        }
        #[cfg(feature = "kubernetes")]
        if self
            .authority
            .database_outbox
            .lock()
            .await
            .pending()?
            .is_some()
        {
            return Ok(true);
        }
        Ok(false)
    }
    async fn heartbeat(&self) -> Result<()> {
        let session = self.session().await?;
        let snapshot =
            tokio::time::timeout(Duration::from_secs(10), self.adapter.target_observation())
                .await
                .map_err(|_| AgentError::Transport)??;
        let sequence = self.state.lock().await.next_sequence()?;
        let observation_json = if let Some(snapshot) = snapshot {
            #[cfg(feature = "kubernetes")]
            let database_capabilities = if let Some(runtime) = &self.database_runtime {
                let nodes = snapshot
                    .nodes
                    .iter()
                    .map(|node| node.id.clone())
                    .collect::<Vec<_>>();
                tokio::time::timeout(Duration::from_secs(10), runtime.capabilities(&nodes))
                    .await
                    .map_err(|_| AgentError::Transport)??
            } else {
                None
            };
            let mut value = serde_json::json!({"sequence":sequence,"generation":self.config.generation,"observedAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"version":snapshot.version,"nodes":snapshot.nodes,"runtimeKind":snapshot.runtime_kind,"capacity":{"apps":0,"cpuMillicores":0,"memoryBytes":0,"volumeBytes":0,"ephemeralBytes":0,"replicasPerApp":0,"concurrentDeployments":0},"capabilities":{"networkIsolation":false,"restrictedPods":false,"storage":false,"ingress":false,"provisioningGuardVersion":1}});
            #[cfg(feature = "docker")]
            if let Some(runtime) = &self.recovery_runtime {
                value["capabilities"]["nativeRecovery"] = runtime.capabilities()?;
            }
            if let Some(runtime) = &self.mail_runtime
                && let Some(capabilities) =
                    tokio::time::timeout(Duration::from_secs(10), runtime.capabilities())
                        .await
                        .map_err(|_| AgentError::Transport)??
            {
                value["capabilities"]["nativeMail"] =
                    serde_json::to_value(capabilities).map_err(|_| AgentError::Transport)?;
                if self.config.mail_configuration_enabled
                    && session.configuration_available.load(Ordering::SeqCst)
                    && let Some(runtime) = &self.configuration_runtime
                    && matches!(
                        tokio::time::timeout(
                            Duration::from_secs(10),
                            runtime.configuration_capable()
                        )
                        .await,
                        Ok(Ok(true))
                    )
                {
                    value["capabilities"]["nativeMail"]["configurationVersion"] = 1.into();
                }
            }
            #[cfg(feature = "kubernetes")]
            if let Some(capabilities) = database_capabilities {
                let fields =
                    serde_json::to_value(capabilities).map_err(|_| AgentError::Transport)?;
                value["capabilities"]
                    .as_object_mut()
                    .ok_or(AgentError::Transport)?
                    .extend(fields.as_object().ok_or(AgentError::Transport)?.clone());
            }
            #[cfg(feature = "kubernetes")]
            if self.config.plugin_runtime_enabled
                && self.plugin_runtime.is_some()
                && session.plugin_domain.load(Ordering::SeqCst) == plugin::DOMAIN_AVAILABLE
            {
                value["capabilities"]["pluginRuntimeVersion"] =
                    session.plugin_spec_version.load(Ordering::SeqCst).into();
            }
            value["capabilities"]["environment"] = snapshot.environment.into();
            value["capabilities"]["volumeQuotas"] = (snapshot.runtime_kind == "docker"
                && snapshot.storage.as_ref().is_some_and(|s| s.ready))
            .into();
            if let Some(storage) = snapshot.storage {
                value["capabilities"]["storage"] = storage.ready.into();
                value["capabilities"]["storageVersion"] = 1.into();
                value["capacity"]["volumeBytes"] = storage.capacity_bytes.into();
            }
            add_public_routing_capability(&mut value, snapshot.public_routing_version);
            if let Some(1) = snapshot.autoscaling_version {
                value["capabilities"]["autoscalingVersion"] = 1.into();
            }
            if let Some(version) = snapshot.registry_version {
                value["capabilities"]["registryVersion"] = version.into();
            }
            if let Some(version) = snapshot.service_bindings_version {
                value["capabilities"]["serviceBindingsVersion"] = version.into();
            }
            let bytes = serde_json::to_vec(&value).map_err(|_| AgentError::Transport)?;
            if bytes.len() > 65536 {
                return Err(AgentError::Transport);
            }
            bytes
        } else {
            Vec::new()
        };
        let ack = session
            .client
            .clone()
            .heartbeat(request(wire::HeartbeatRequest {
                identity: Some(session.identity.clone()),
                sequence,
                observed_at: timestamp()?,
                observation_json,
            }))
            .await
            .map_err(|e| session.rpc_error(e))?
            .into_inner();
        accepted(ack)
    }
    /// One bounded dispatch cycle. The HTTP/2 channel stays open across cycles.
    pub async fn cycle(&mut self) -> Result<bool> {
        let result = self.cycle_inner().await;
        if let Err(error) = &result {
            if matches!(error, AgentError::Revoked) {
                self.state.lock().await.revoke()?;
            }
            *self.authority.session.write().await = None;
        }
        result
    }
    async fn cycle_inner(&self) -> Result<bool> {
        let hosting_ready = self.hosting_delivery_ready().await?;
        self.heartbeat().await?;
        let session = self.session().await?;
        let dispatch = if hosting_ready {
            session
                .client
                .clone()
                .poll(request(wire::PollRequest {
                    identity: Some(session.identity.clone()),
                }))
                .await
                .map_err(|e| session.rpc_error(e))?
                .into_inner()
        } else {
            wire::Dispatch { command: None }
        };
        let work = async {
            let mut worked = false;
            #[cfg(feature = "docker")]
            if self.recovery_runtime.is_some() {
                // Recovery snapshots carry large bounded transfer futures; keep them off the
                // ordinary dispatch stack even when the recovery domain is disabled.
                match Box::pin(self.process_recovery()).await {
                    Ok(recovery_work) => worked |= recovery_work,
                    Err(AgentError::RecoveryPending) => worked = true,
                    Err(error) => return Err(error),
                }
            }
            if let Some(command) = dispatch.command {
                self.process(command).await?;
                worked = true;
            }
            #[cfg(feature = "kubernetes")]
            if self.plugin_delivery_ready().await? {
                match self.process_plugin().await {
                    Ok(work) => worked |= work,
                    Err(AgentError::PluginPending) => worked = true,
                    Err(error) => return Err(error),
                }
            }
            #[cfg(feature = "kubernetes")]
            if self.workload_runtime.is_some() {
                worked |= self.process_workloads().await?;
            }
            #[cfg(feature = "kubernetes")]
            {
                if self.database_delivery_ready().await? {
                    match self.process_database().await {
                        Ok(database_work) => worked |= database_work,
                        Err(AgentError::DatabasePending) => worked = true,
                        Err(error) => return Err(error),
                    }
                }
            }
            if self.mail_delivery_ready().await? {
                match self.process_mail().await {
                    Ok(mail_work) => worked |= mail_work,
                    Err(AgentError::MailPending) => worked = true,
                    Err(error) => return Err(error),
                }
            }
            if self.configuration_delivery_ready().await? {
                match self.process_configuration().await {
                    Ok(configuration_work) => worked |= configuration_work,
                    Err(AgentError::ConfigurationPending) => worked = true,
                    Err(error) => return Err(error),
                }
            }
            Ok(worked)
        };
        tokio::pin!(work);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
        heartbeat.tick().await;
        loop {
            tokio::select! { result=&mut work=>return result,_=heartbeat.tick()=>self.heartbeat().await?, }
        }
    }
    async fn process(&self, raw: wire::Command) -> Result<()> {
        if raw.spec_json.len() > 65536 {
            return Err(AgentError::Configuration("Command exceeds limit"));
        }
        let spec: webdock_supervisor::RuntimeSpec = serde_json::from_slice(&raw.spec_json)
            .map_err(|_| AgentError::Configuration("Invalid workload specification"))?;
        spec.validate()?;
        if spec.installation != self.config.installation {
            return Err(AgentError::Execution(ExecutionError::Ownership));
        }
        let intent = wire::Intent::try_from(raw.intent)
            .map_err(|_| AgentError::Configuration("Unknown command intent"))?;
        let (fingerprint, result, outcome) = match intent {
            wire::Intent::Apply
            | wire::Intent::Stop
            | wire::Intent::Retire
            | wire::Intent::PurgeStorage => {
                if raw.log_tail != 0 {
                    return Err(AgentError::Configuration("Invalid mutation parameters"));
                }
                if raw.material_json.len() > webdock_supervisor::MAX_RUNTIME_MATERIAL_BYTES {
                    return Err(AgentError::Configuration("Runtime material exceeds limit"));
                }
                let material = if raw.material_json.is_empty() {
                    None
                } else {
                    Some(
                        serde_json::from_slice::<webdock_supervisor::RuntimeMaterial>(
                            &raw.material_json,
                        )
                        .map_err(|_| AgentError::Configuration("Invalid runtime material"))?,
                    )
                };
                if material.is_some() && intent != wire::Intent::Apply {
                    return Err(AgentError::Configuration("Unexpected runtime material"));
                }
                let command = Command {
                    material,
                    operation_id: raw.operation_id.clone(),
                    generation: raw.generation,
                    lease_until: raw.lease_until,
                    authorization: raw.authorization,
                    action: match intent {
                        wire::Intent::Apply => Action::Apply,
                        wire::Intent::Stop => Action::Stop,
                        wire::Intent::Retire => Action::Retire,
                        wire::Intent::PurgeStorage => Action::PurgeStorage,
                        _ => unreachable!(),
                    },
                    spec,
                    expected_runtime_id: raw.expected_runtime_id,
                };
                command.validate()?;
                let fingerprint = command.fingerprint()?;
                let result = self.executor.execute(command).await.and_then(|value| {
                    serde_json::to_vec(&value)
                        .map(wire::receipt::Result::ObservedJson)
                        .map_err(|_| ExecutionError::Runtime)
                });
                (fingerprint, result, wire::Outcome::NeedsReconciliation)
            }
            wire::Intent::Inspect | wire::Intent::Logs | wire::Intent::Metrics => {
                if !raw.material_json.is_empty() {
                    return Err(AgentError::Configuration("Read has runtime material"));
                }
                let kind = match intent {
                    wire::Intent::Inspect => ReadKind::Inspect,
                    wire::Intent::Logs => ReadKind::Logs,
                    _ => ReadKind::Metrics,
                };
                let command = ReadCommand {
                    operation_id: raw.operation_id.clone(),
                    generation: raw.generation,
                    lease_until: raw.lease_until,
                    authorization: raw.authorization,
                    kind,
                    tail: raw.log_tail,
                    spec,
                    expected_runtime_id: raw.expected_runtime_id,
                };
                command.validate()?;
                let fingerprint = command.fingerprint()?;
                let result = self.read(&command).await;
                (fingerprint, result, wire::Outcome::Rejected)
            }
            wire::Intent::Unspecified => {
                return Err(AgentError::Configuration("Unspecified command intent"));
            }
        };
        let (outcome, error_code, result) = match result {
            Ok(value) => (wire::Outcome::Succeeded, String::new(), Some(value)),
            Err(error) => (outcome, error_code(&error).into(), None),
        };
        let session = self.session().await?;
        let receipt = wire::Receipt {
            identity: Some(session.identity.clone()),
            operation_id: raw.operation_id,
            generation: raw.generation,
            request_sha256: fingerprint,
            outcome: outcome as i32,
            error_code,
            result,
        };
        self.outbox.lock().await.put(&receipt)?;
        self.flush_receipt().await
    }
    async fn read(
        &self,
        command: &ReadCommand,
    ) -> webdock_supervisor::Result<wire::receipt::Result> {
        self.authority.authorize_read(command).await?;
        // This is read-only work. Authority is checked before, periodically while pending,
        // and freshly after it; ownership is independently checked on both sides of the read.
        let work = async {
            if let Some(expected) = &command.expected_runtime_id
                && self
                    .adapter
                    .inspect(&command.spec)
                    .await?
                    .runtime_id
                    .as_ref()
                    != Some(expected)
            {
                return Err(ExecutionError::Ownership);
            }
            let result = match command.kind {
                ReadKind::Inspect => {
                    serde_json::to_vec(&self.adapter.inspect(&command.spec).await?)
                        .map(wire::receipt::Result::ObservedJson)
                        .map_err(|_| ExecutionError::Runtime)
                }
                ReadKind::Logs => Ok(wire::receipt::Result::LogsUtf8(
                    self.adapter
                        .logs(&command.spec, command.tail)
                        .await?
                        .into_bytes(),
                )),
                ReadKind::Metrics => {
                    serde_json::to_vec(&self.adapter.metrics(&command.spec).await?)
                        .map(wire::receipt::Result::MetricsJson)
                        .map_err(|_| ExecutionError::Runtime)
                }
            }?;
            if let Some(expected) = &command.expected_runtime_id
                && self
                    .adapter
                    .inspect(&command.spec)
                    .await?
                    .runtime_id
                    .as_ref()
                    != Some(expected)
            {
                return Err(ExecutionError::Ownership);
            }
            Ok(result)
        };
        tokio::pin!(work);
        let deadline = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(deadline);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        // The initial guard just succeeded; the next periodic check is due after one interval.
        interval.reset();
        // Poll runtime work while a remote authorization check is in flight.
        let revalidation_failure = async {
            loop {
                interval.tick().await;
                if let Err(error) = self.authority.authorize_read(command).await {
                    return error;
                }
            }
        };
        let result = tokio::select! {
            result = &mut work => result?,
            _ = &mut deadline => return Err(ExecutionError::Expired),
            error = revalidation_failure => return Err(error),
        };
        self.authority.authorize_read(command).await?;
        let size = match &result {
            wire::receipt::Result::ObservedJson(v)
            | wire::receipt::Result::LogsUtf8(v)
            | wire::receipt::Result::MetricsJson(v) => v.len(),
        };
        if size > 65536 || (command.kind == ReadKind::Logs && size > crate::logs::LIMIT) {
            return Err(ExecutionError::Invalid("Read result exceeds limit"));
        }
        Ok(result)
    }
    /// Reconnects with bounded backoff; disconnects retain workloads and journaled operations.
    pub async fn run(
        mut self,
        shutdown: impl std::future::Future<Output = ()> + Send,
    ) -> Result<()> {
        tokio::pin!(shutdown);
        let mut delay = 1u64;
        loop {
            let result =
                tokio::select! {_= &mut shutdown=>return Ok(()),result=self.connect()=>result};
            match result {
                Ok(()) => {
                    let mut idle_delay = Duration::from_secs(1);
                    loop {
                        let result = tokio::select! {_= &mut shutdown=>return Ok(()),result=self.cycle()=>result};
                        match result {
                            Ok(worked) => {
                                delay = 1;
                                // Pending receipts are delivery work even when no new command ran.
                                let pending = !worked && self.has_pending_delivery().await?;
                                idle_delay = next_idle_cycle(idle_delay, worked, pending);
                            }
                            Err(AgentError::Revoked) => return Err(AgentError::Revoked),
                            Err(AgentError::State) => return Err(AgentError::State),
                            Err(_) => break,
                        }
                        tokio::select! {_= &mut shutdown=>return Ok(()),_=tokio::time::sleep(idle_delay)=>{}}
                    }
                }
                Err(AgentError::Revoked) => return Err(AgentError::Revoked),
                Err(AgentError::State) => return Err(AgentError::State),
                Err(_) => {}
            }
            tracing::warn!(
                delay_seconds = delay,
                "Control connection unavailable; retaining workloads"
            );
            tokio::select! {_= &mut shutdown=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(delay))=>{}}
            delay = (delay * 2).min(30);
        }
    }
}
fn next_idle_cycle(previous: Duration, worked: bool, pending_delivery: bool) -> Duration {
    if worked || pending_delivery {
        Duration::from_secs(1)
    } else {
        previous.saturating_mul(2).min(Duration::from_secs(5))
    }
}
fn error_code(error: &ExecutionError) -> &'static str {
    match error {
        ExecutionError::Denied => "authorization_denied",
        ExecutionError::Expired => "lease_expired",
        ExecutionError::Fenced => "generation_fenced",
        ExecutionError::Conflict => "operation_conflict",
        ExecutionError::Ownership => "ownership_conflict",
        ExecutionError::Unsupported(_) => "capability_unavailable",
        ExecutionError::Invalid(_) => "invalid_command",
        ExecutionError::Journal => "journal_unavailable",
        ExecutionError::Runtime => "runtime_unavailable",
    }
}

/// Offline validation checks private files, client validity and TLS key/certificate parsing.
pub fn validate_configuration(config: &AgentConfig) -> Result<()> {
    config.validate()?;
    #[cfg(feature = "docker")]
    if let Some(path) = &config.recovery_runtime {
        let recovery = crate::recovery::config::RecoveryRuntimeConfig::load(path)?;
        if recovery.installation != config.installation {
            return Err(AgentError::Configuration(
                "Recovery runtime installation differs from enrolled agent",
            ));
        }
    }
    #[cfg(not(feature = "docker"))]
    if config.recovery_runtime.is_some() {
        return Err(AgentError::Configuration(
            "Recovery runtime is not included in this build",
        ));
    }
    if let Some(path) = &config.mail_runtime {
        #[cfg(any(feature = "docker", feature = "kubernetes"))]
        {
            let installation = match &config.runtime {
                #[cfg(feature = "docker")]
                crate::config::RuntimeConfiguration::Docker { .. } => {
                    let mail: crate::mail::docker::DockerMailConfig =
                        serde_json::from_slice(&read_private(path, 65536)?).map_err(|_| {
                            AgentError::Configuration("Invalid Mail runtime configuration")
                        })?;
                    mail.validate()?;
                    mail.installation
                }
                #[cfg(feature = "kubernetes")]
                crate::config::RuntimeConfiguration::Kubernetes { storage_socket, .. } => {
                    if storage_socket.is_none() {
                        return Err(AgentError::Configuration(
                            "Mail requires verified node-local storage",
                        ));
                    }
                    crate::mail::kubernetes::KubernetesMailConfig::load(path)?.installation
                }
                #[cfg(not(all(feature = "docker", feature = "kubernetes")))]
                _ => {
                    return Err(AgentError::Configuration(
                        "Mail runtime is not included in this build",
                    ));
                }
            };
            if installation != config.installation {
                return Err(AgentError::Configuration(
                    "Mail runtime installation differs from enrolled agent",
                ));
            }
        }
        #[cfg(not(any(feature = "docker", feature = "kubernetes")))]
        {
            let _ = path;
            return Err(AgentError::Configuration(
                "Mail runtime is not included in this build",
            ));
        }
    }
    if let Some(path) = &config.database_runtime {
        #[cfg(feature = "kubernetes")]
        crate::database::DatabaseRuntimeConfig::load(path)?;
        #[cfg(not(feature = "kubernetes"))]
        {
            let _ = path;
            return Err(AgentError::Configuration(
                "Database runtime is not included in this build",
            ));
        }
    }
    let credentials = Credentials::load(&config.control)?;
    if let ControlEndpoint::Mtls { origin, .. } = &config.control {
        Endpoint::from_shared(origin.clone())
            .map_err(|_| AgentError::Configuration("Control origin"))?
            .tls_config(
                credentials
                    .tls
                    .ok_or(AgentError::Configuration("TLS required"))?,
            )
            .map_err(|_| AgentError::Configuration("TLS identity"))?;
    }
    crate::policy::RuntimePolicy::load(config.policy.clone())?;
    Ok(())
}
#[cfg(all(test, feature = "docker"))]
mod recovery_validation_tests {
    use super::*;
    #[test]
    fn offline_agent_validation_rejects_invalid_recovery_profile_before_daemon_start() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut config:AgentConfig=serde_json::from_value(serde_json::json!({"version":1,"installation":"unit","agentId":"unit-agent","generation":1,"stateDirectory":root.path(),"control":{"kind":"unix","path":root.path().join("control.sock")},"runtime":{"kind":"docker","socket":root.path().join("engine.sock"),"initializerImage":null}})).unwrap();
        validate_configuration(&config).unwrap();
        let path = root.path().join("recovery.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        config.recovery_runtime = Some(path);
        assert!(
            validate_configuration(&config).is_err(),
            "CLI only checked file presence, not the referenced recovery profile"
        );
    }
    #[cfg(feature = "local-recovery-fixture")]
    #[test]
    fn offline_recovery_validation_preserves_valid_profile_and_rejects_foreign_installation() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        for (name, bytes) in [
            (
                "reader",
                br#"{"accessKeyId":"unit-reader","secretAccessKey":"unit-reader-only"}"#.as_slice(),
            ),
            (
                "writer",
                br#"{"accessKeyId":"unit-writer","secretAccessKey":"unit-writer-only"}"#.as_slice(),
            ),
            ("identity", b"unit-key-not-used-for-encryption"),
            ("signers", b"unit-signers-not-used-for-signing"),
        ] {
            let p = root.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let runtime = serde_json::json!({"version":1,"installation":"unit","projectId":"123","resourceId":"124","revision":1,"image":format!("docker.io/test/ntfy@sha256:{}","a".repeat(64)),"arguments":["serve"],"cpuMillicores":100,"memoryBytes":67108864,"ephemeralBytes":1048576,"replicas":1,"port":8080,"healthPath":null,"volume":{"mountPath":"/data","sizeBytes":67108864}});
        let mut profile = serde_json::json!({"version":1,"installation":"unit","dockerSocket":root.path().join("engine.sock"),"storageSocket":root.path().join("storage.sock"),"stagingDirectory":root.path(),"custodyDirectory":root.path(),"replayDirectory":root.path(),"coreSigners":root.path().join("signers"),"operatorSigners":root.path().join("signers"),"targets":[],"profiles":[{"id":"ntfy-unit","profile":"ntfy-sqlite-offline-v1","keyCustodyID":"operator-unit","destination":{"id":"unit","endpoint":"http://127.0.0.1:9000","region":"local","bucket":"unit","namespace":"unit"},"writeCredentials":root.path().join("writer"),"restoreCredentials":root.path().join("reader"),"ageRecipient":"unit-unused","ageIdentity":root.path().join("identity"),"independentFiles":{},"source":{"kind":"ntfy","runtimeID":"a".repeat(64),"runtime":runtime}}]});
        let path = root.path().join("recovery.json");
        std::fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let config:AgentConfig=serde_json::from_value(serde_json::json!({"version":1,"installation":"unit","agentId":"unit-agent","generation":1,"stateDirectory":root.path(),"control":{"kind":"unix","path":root.path().join("control.sock")},"runtime":{"kind":"docker","socket":root.path().join("engine.sock"),"initializerImage":null},"recoveryRuntime":path})).unwrap();
        validate_configuration(&config).unwrap();
        profile["installation"] = serde_json::json!("foreign");
        std::fs::write(&path, serde_json::to_vec(&profile).unwrap()).unwrap();
        assert!(matches!(
            validate_configuration(&config),
            Err(AgentError::Configuration(
                "Recovery runtime installation differs from enrolled agent"
            ))
        ));
    }
}

fn add_public_routing_capability(value: &mut serde_json::Value, version: Option<u8>) {
    if let Some(version) = version {
        value["capabilities"]["publicRoutingVersion"] = version.into();
    }
}
#[cfg(test)]
mod public_routing_contract_tests {
    use super::*;
    #[test]
    fn advertised_public_routing_capability_matches_the_strict_core_contract() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../hosting/src/observation.schema.json"))
                .unwrap();
        let allowed = schema["properties"]["capabilities"]["properties"]
            .as_object()
            .unwrap();
        let mut value = serde_json::json!({"capabilities":{}});
        add_public_routing_capability(&mut value, Some(1));
        assert_eq!(value["capabilities"]["publicRoutingVersion"], 1);
        for key in value["capabilities"].as_object().unwrap().keys() {
            assert!(allowed.contains_key(key), "Undeclared capability {key}");
        }
        let mut disabled = serde_json::json!({"capabilities":{}});
        add_public_routing_capability(&mut disabled, None);
        assert_eq!(disabled, serde_json::json!({"capabilities":{}}));
    }
}

#[cfg(test)]
mod idle_cycle_tests {
    use super::*;

    struct UnusedRuntime;
    #[async_trait::async_trait]
    impl RuntimeAdapter for UnusedRuntime {
        fn capabilities(&self) -> webdock_supervisor::Capabilities {
            panic!("pending delivery inspection must not contact a runtime")
        }
        async fn reconcile(
            &self,
            _: &Command,
            _: &webdock_supervisor::LeaseGuard,
        ) -> std::result::Result<webdock_supervisor::ObservedState, ExecutionError> {
            panic!("pending delivery inspection must not execute work")
        }
        async fn inspect(
            &self,
            _: &webdock_supervisor::RuntimeSpec,
        ) -> std::result::Result<webdock_supervisor::ObservedState, ExecutionError> {
            panic!("pending delivery inspection must remain local")
        }
        async fn logs(
            &self,
            _: &webdock_supervisor::RuntimeSpec,
            _: u32,
        ) -> std::result::Result<String, ExecutionError> {
            panic!("pending delivery inspection must remain local")
        }
    }

    #[tokio::test]
    async fn every_retained_outbox_keeps_delivery_fast_without_an_attached_runtime() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let config: AgentConfig = serde_json::from_value(serde_json::json!({
            "version":1,"installation":"idle-test","agentId":"idle-agent","generation":1,
            "stateDirectory":root.path(),
            "control":{"kind":"unix","path":root.path().join("unused-control.sock")},
            "runtime":{"kind":"docker","socket":root.path().join("unused-engine.sock"),"initializerImage":null}
        }))
        .unwrap();
        let daemon = Daemon::with_adapter(config, Arc::new(UnusedRuntime)).unwrap();
        assert!(!daemon.has_pending_delivery().await.unwrap());
        assert!(daemon.mail_runtime.is_none());
        assert!(daemon.configuration_runtime.is_none());

        macro_rules! check_outbox {
            ($outbox:expr) => {{
                let delivery = Default::default();
                $outbox.lock().await.put(&delivery).unwrap();
                let pending = daemon.has_pending_delivery().await.unwrap();
                assert!(pending);
                assert_eq!(
                    next_idle_cycle(Duration::from_secs(5), false, pending),
                    Duration::from_secs(1)
                );
                $outbox.lock().await.acknowledge(&delivery).unwrap();
                assert!(!daemon.has_pending_delivery().await.unwrap());
            }};
        }
        check_outbox!(daemon.outbox);
        check_outbox!(daemon.authority.mail_outbox);
        check_outbox!(daemon.authority.configuration_outbox);
        #[cfg(feature = "docker")]
        check_outbox!(daemon.authority.recovery_outbox);
        #[cfg(feature = "kubernetes")]
        check_outbox!(daemon.authority.plugin_outbox);
        #[cfg(feature = "kubernetes")]
        check_outbox!(daemon.authority.database_outbox);
    }

    #[test]
    fn only_empty_cycles_back_off_and_pending_work_stays_fast() {
        let mut delay = Duration::from_secs(1);
        let mut observed = Vec::new();
        for _ in 0..5 {
            delay = next_idle_cycle(delay, false, false);
            observed.push(delay.as_secs());
        }
        assert_eq!(observed, [2, 4, 5, 5, 5]);
        assert_eq!(next_idle_cycle(delay, true, false), Duration::from_secs(1));
        assert_eq!(next_idle_cycle(delay, false, true), Duration::from_secs(1));
        assert_eq!(
            next_idle_cycle(Duration::MAX, false, false),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn idle_discovery_is_bounded_without_changing_active_cadence() {
        let mut elapsed = Duration::ZERO;
        let mut delay = Duration::from_secs(1);
        let mut cycles = 0;
        while elapsed < Duration::from_secs(20) {
            cycles += 1;
            delay = next_idle_cycle(delay, false, false);
            elapsed += delay;
        }
        assert_eq!(cycles, 5);
        for _ in 0..20 {
            delay = next_idle_cycle(delay, true, false);
            assert_eq!(delay, Duration::from_secs(1));
        }
    }
}
