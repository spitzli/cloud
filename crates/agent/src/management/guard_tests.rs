use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::Response;
use wire::agent_control_server::{AgentControl, AgentControlServer};

struct LateReply {
    replied: Arc<AtomicBool>,
}

#[tonic::async_trait]
impl AgentControl for LateReply {
    async fn register(
        &self,
        _: Request<wire::Hello>,
    ) -> std::result::Result<Response<wire::Welcome>, Status> {
        Err(Status::unimplemented("guard-only fixture"))
    }
    async fn heartbeat(
        &self,
        _: Request<wire::HeartbeatRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Err(Status::unimplemented("guard-only fixture"))
    }
    async fn poll(
        &self,
        _: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::Dispatch>, Status> {
        Err(Status::unimplemented("guard-only fixture"))
    }
    async fn complete(
        &self,
        _: Request<wire::Receipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Err(Status::unimplemented("guard-only fixture"))
    }
    async fn guard(
        &self,
        request: Request<wire::GuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        while timestamp().unwrap() <= request.get_ref().lease_until {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.replied.store(true, Ordering::SeqCst);
        Ok(Response::new(wire::GuardReply { allowed: true }))
    }
}

#[tokio::test]
async fn delayed_hosting_guard_cannot_extend_authority_past_lease() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let replied = Arc::new(AtomicBool::new(false));
    let service = LateReply {
        replied: replied.clone(),
    };
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(service))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let channel = Endpoint::from_shared(format!("unix://{}", path.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let authority = RemoteAuthority {
        #[cfg(feature = "docker")]
        recovery_executor: std::sync::OnceLock::new(),
        session: Arc::new(RwLock::new(Some(Session {
            #[cfg(feature = "docker")]
            recovery_client: wire::recovery_control_client::RecoveryControlClient::new(
                channel.clone(),
            ),
            client: AgentControlClient::new(channel.clone()),
            mail_client: wire::mail_control_client::MailControlClient::new(channel.clone()),
            configuration_client:
                wire::mail_configuration_control_client::MailConfigurationControlClient::new(
                    channel.clone(),
                ),
            configuration_available: Arc::new(AtomicBool::new(true)),
            #[cfg(feature = "kubernetes")]
            plugin_client: wire::plugin_runtime_control_client::PluginRuntimeControlClient::new(
                channel.clone(),
            ),
            #[cfg(feature = "kubernetes")]
            plugin_domain: Arc::new(std::sync::atomic::AtomicU8::new(plugin::DOMAIN_UNKNOWN)),
            #[cfg(feature = "kubernetes")]
            plugin_spec_version: Arc::new(std::sync::atomic::AtomicU8::new(1)),
            #[cfg(feature = "kubernetes")]
            workload_client: wire::workload_control_client::WorkloadControlClient::new(
                channel.clone(),
            ),
            #[cfg(feature = "kubernetes")]
            database_client: wire::database_control_client::DatabaseControlClient::new(channel),
            identity: wire::AgentIdentity {
                installation: "guard-fixture".into(),
                agent_id: "guard-fixture".into(),
                generation: 1,
            },
            endpoint: ControlEndpoint::Unix { path },
            stamp: "unix".into(),
            expires_at: None,
            revoked: Arc::new(AtomicBool::new(false)),
            disconnected: Arc::new(AtomicBool::new(false)),
        }))),
        mail_outbox: Mutex::new(
            crate::outbox::MailOutbox::open(directory.path().join("mail.json")).unwrap(),
        ),
        #[cfg(feature = "docker")]
        recovery_outbox: Mutex::new(
            crate::outbox::RecoveryOutbox::open(directory.path().join("recovery.json")).unwrap(),
        ),
        mail_retry_at: Mutex::new(None),
        #[cfg(feature = "kubernetes")]
        plugin_outbox: Mutex::new(
            crate::outbox::PluginRuntimeOutbox::open(directory.path().join("plugin.json")).unwrap(),
        ),
        #[cfg(feature = "kubernetes")]
        plugin_retry_at: Mutex::new(None),
        #[cfg(feature = "kubernetes")]
        plugin_checkpoint: Mutex::new(None),
        configuration_outbox: Mutex::new(
            crate::outbox::MailConfigurationOutbox::open(
                directory.path().join("configuration.json"),
            )
            .unwrap(),
        ),
        configuration_retry_at: Mutex::new(None),
        configuration_transient: AtomicBool::new(false),
        #[cfg(feature = "kubernetes")]
        database_outbox: Mutex::new(
            crate::outbox::DatabaseOutbox::open(directory.path().join("database.json")).unwrap(),
        ),
        #[cfg(feature = "kubernetes")]
        database_retry_at: Mutex::new(None),
    };
    let result = authority
        .guard(
            "read-fixture".into(),
            1,
            timestamp().unwrap() + 2,
            "guard-reference".into(),
            wire::Intent::Logs,
            "a".repeat(64),
        )
        .await;
    drop(authority);
    stop.send(()).unwrap();
    server.await.unwrap();
    assert!(replied.load(Ordering::SeqCst));
    assert!(matches!(result, Err(ExecutionError::Expired)));
}
