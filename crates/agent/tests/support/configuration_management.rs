use super::*;
use spitzli_agent::mail::{
    MailAuthority, MailCapabilities, MailRuntime,
    configuration::{ConfigurationAuthority, ConfigurationRuntime},
};
use std::sync::atomic::AtomicUsize;
use wire::mail::{MailClaim, MailLease, MailProof, MailResources};
use wire::mail_configuration::*;
use wire::mail_configuration_control_server::{
    MailConfigurationControl, MailConfigurationControlServer,
};
struct IdleMail;
#[tonic::async_trait]
impl wire::mail_control_server::MailControl for IdleMail {
    async fn poll_mail(
        &self,
        _: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::MailDispatch>, Status> {
        Ok(Response::new(wire::MailDispatch { command: None }))
    }
    async fn guard_mail(
        &self,
        _: Request<wire::MailGuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        Ok(Response::new(wire::GuardReply { allowed: false }))
    }
    async fn checkpoint_mail(
        &self,
        _: Request<wire::MailCheckpointRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Ok(Response::new(wire::Ack { accepted: false }))
    }
    async fn complete_mail(
        &self,
        _: Request<wire::MailReceipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Ok(Response::new(wire::Ack { accepted: false }))
    }
}
#[async_trait::async_trait]
impl MailRuntime for IdleMail {
    async fn capabilities(&self) -> Result<Option<MailCapabilities>> {
        Ok(Some(MailCapabilities {
            version: 1,
            image: format!("registry.example.test/mail@sha256:{}", "a".repeat(64)),
        }))
    }
    async fn execute(&self, _: &MailClaim, _: &dyn MailAuthority) -> Result<MailProof> {
        panic!("configuration must never invoke lifecycle")
    }
}
struct ConfigRuntime(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl ConfigurationRuntime for ConfigRuntime {
    async fn configuration_capable(&self) -> Result<bool> {
        Ok(true)
    }
    async fn configure(
        &self,
        claim: &ConfigurationClaim,
        authority: &dyn ConfigurationAuthority,
    ) -> Result<ConfigurationProof> {
        authority.guard(claim).await?;
        authority
            .checkpoint(
                claim,
                ConfigurationCheckpoint {
                    lease: claim.lease.clone(),
                    runtime_id: "normal-fixture".into(),
                    phase: ConfigurationCheckpointPhase::Bound {},
                },
            )
            .await?;
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ConfigurationProof {
            runtime_id: "normal-fixture".into(),
            domains: Default::default(),
            mailboxes: Default::default(),
            identities: Default::default(),
            dkim: Default::default(),
        })
    }
}
struct ConfigurationCore {
    peer: Arc<Core>,
    command: Mutex<Option<wire::MailConfigurationCommand>>,
    receipts: Mutex<Vec<wire::MailConfigurationReceipt>>,
    checkpoints: Mutex<Vec<wire::MailConfigurationCheckpointRequest>>,
    lose_receipt: AtomicBool,
    unavailable_guard: AtomicBool,
    polls: AtomicUsize,
}
struct ConfigurationService(Arc<ConfigurationCore>);
#[tonic::async_trait]
impl MailConfigurationControl for ConfigurationService {
    async fn poll_mail_configuration(
        &self,
        request: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::MailConfigurationDispatch>, Status> {
        self.0.peer.peer(&request)?;
        self.0.polls.fetch_add(1, Ordering::SeqCst);
        Ok(Response::new(wire::MailConfigurationDispatch {
            command: self.0.command.lock().unwrap().take(),
        }))
    }
    async fn guard_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationGuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        self.0.peer.peer(&request)?;
        if self.0.unavailable_guard.load(Ordering::SeqCst) {
            return Err(Status::unavailable(
                "configuration authority store unavailable",
            ));
        }
        Ok(Response::new(wire::GuardReply {
            allowed: self.0.peer.allowed.load(Ordering::SeqCst),
        }))
    }
    async fn checkpoint_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationCheckpointRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.peer.peer(&request)?;
        self.0
            .checkpoints
            .lock()
            .unwrap()
            .push(request.into_inner());
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationReceipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.peer.peer(&request)?;
        self.0.receipts.lock().unwrap().push(request.into_inner());
        if self.0.lose_receipt.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("lost configuration receipt ACK"));
        }
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
fn peer() -> Arc<Core> {
    Arc::new(Core {
        commands: Mutex::new(VecDeque::new()),
        receipts: Mutex::new(Vec::new()),
        sequences: Mutex::new(Vec::new()),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: Default::default(),
    })
}
fn config(directory: &std::path::Path, socket: std::path::PathBuf, enabled: bool) -> AgentConfig {
    let runtime = directory.join("mail-runtime.json");
    std::fs::write(&runtime, "{}").unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o600)).unwrap();
    AgentConfig {
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: directory.into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        mail_runtime: Some(runtime),
        recovery_runtime: None,
        mail_configuration_enabled: enabled,
        database_runtime: None,
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    }
}
fn command() -> wire::MailConfigurationCommand {
    let material = ConfigurationMaterial {
        administrator: ConfigurationCredential {
            username: "webdock-admin@mail-123.example.test".into(),
            password: "Synthetic-Private-Only-Password".into(),
        },
        identities: Default::default(),
    };
    let spec = ConfigurationSpec {
        version: 1,
        operation_id: "7001".into(),
        customer_id: "123".into(),
        cluster_id: "789".into(),
        revision: "1".into(),
        service_revision: "1".into(),
        agent_generation: 1,
        mode: ConfigurationMode::Apply,
        target: ConfigurationTarget {
            instance_key: "mail-123".into(),
            hostname: "mail-123.example.test".into(),
            image: format!("registry.example.test/mail@sha256:{}", "a".repeat(64)),
            resources: MailResources::FIXED,
            management_sha256: material.management_fingerprint().unwrap(),
            certificate_sha256: "b".repeat(64),
        },
        material_sha256: material.fingerprint().unwrap(),
        snapshot: ConfigurationSnapshot {
            domains: vec![],
            mailboxes: vec![],
            identities: vec![],
            grants: vec![],
        },
    };
    wire::MailConfigurationCommand {
        request_sha256: spec.fingerprint().unwrap(),
        spec_json: serde_json::to_vec(&spec).unwrap(),
        lease_json: serde_json::to_vec(&MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        })
        .unwrap(),
        lease_until: timestamp().unwrap() + 60,
        material_json: serde_json::to_vec(&material).unwrap(),
        checkpoints_json: b"[]".to_vec(),
    }
}
#[tokio::test]
async fn optional_configuration_rpc_absence_preserves_hosting_and_heartbeats() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = peer();
    core.commands.lock().unwrap().push_back(super::command(
        wire::Intent::Logs,
        "hosting-before-optional-domain",
    ));
    let service = core.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .add_service(wire::mail_control_server::MailControlServer::new(IdleMail))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let executed = Arc::new(AtomicUsize::new(0));
    let config = config(directory.path(), socket, true);
    let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled))
        .unwrap()
        .with_mail_runtime(Arc::new(IdleMail))
        .with_configuration_runtime(Arc::new(ConfigRuntime(executed.clone())))
        .unwrap();
    daemon.connect().await.unwrap();
    assert!(daemon.cycle().await.unwrap());
    assert!(!daemon.cycle().await.unwrap());
    assert_eq!(core.receipts.lock().unwrap().len(), 1);
    assert!(core.sequences.lock().unwrap().len() >= 2);
    assert_eq!(executed.load(Ordering::SeqCst), 0);
    drop(daemon);
    stop.send(()).unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn configuration_disabled_and_unavailable_do_not_ack_denial_or_block_other_domains() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let peer = peer();
    let core = Arc::new(ConfigurationCore {
        peer: peer.clone(),
        command: Mutex::new(None),
        receipts: Mutex::new(vec![]),
        checkpoints: Mutex::new(vec![]),
        lose_receipt: AtomicBool::new(false),
        unavailable_guard: AtomicBool::new(false),
        polls: AtomicUsize::new(0),
    });
    let service = core.clone();
    let control = peer.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(control)))
            .add_service(wire::mail_control_server::MailControlServer::new(IdleMail))
            .add_service(MailConfigurationControlServer::new(ConfigurationService(
                service,
            )))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let executed = Arc::new(AtomicUsize::new(0));
    let config = config(directory.path(), socket, true);
    let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled))
        .unwrap()
        .with_mail_runtime(Arc::new(IdleMail))
        .with_configuration_runtime(Arc::new(ConfigRuntime(executed.clone())))
        .unwrap();
    daemon.connect().await.unwrap();
    assert!(!daemon.cycle().await.unwrap()); // New Core deliberately has no configuration work.
    *core.command.lock().unwrap() = Some(command());
    core.unavailable_guard.store(true, Ordering::SeqCst);
    peer.commands.lock().unwrap().push_back(super::command(
        wire::Intent::Logs,
        "hosting-during-configuration-outage",
    ));
    assert!(daemon.cycle().await.unwrap());
    assert!(!daemon.cycle().await.unwrap());
    assert_eq!(executed.load(Ordering::SeqCst), 0);
    assert!(core.receipts.lock().unwrap().is_empty());
    assert_eq!(peer.receipts.lock().unwrap().len(), 1);
    assert!(peer.sequences.lock().unwrap().len() >= 3);
    drop(daemon);
    stop.send(()).unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn configuration_completion_outbox_replays_exactly_after_restart_without_execution() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let peer = peer();
    let core = Arc::new(ConfigurationCore {
        peer: peer.clone(),
        command: Mutex::new(Some(command())),
        receipts: Mutex::new(vec![]),
        checkpoints: Mutex::new(vec![]),
        lose_receipt: AtomicBool::new(true),
        unavailable_guard: AtomicBool::new(false),
        polls: AtomicUsize::new(0),
    });
    let service = core.clone();
    let control = peer.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(control)))
            .add_service(wire::mail_control_server::MailControlServer::new(IdleMail))
            .add_service(MailConfigurationControlServer::new(ConfigurationService(
                service,
            )))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let config = config(directory.path(), socket, true);
    let executed = Arc::new(AtomicUsize::new(0));
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled))
            .unwrap()
            .with_mail_runtime(Arc::new(IdleMail))
            .with_configuration_runtime(Arc::new(ConfigRuntime(executed.clone())))
            .unwrap();
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.unwrap());
    }
    assert_eq!(executed.load(Ordering::SeqCst), 1);
    let outbox =
        std::fs::read_to_string(directory.path().join("mail-configuration-outbox.json")).unwrap();
    assert!(!outbox.contains("Synthetic-Private-Only-Password"));
    peer.allowed.store(false, Ordering::SeqCst);
    let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled))
        .unwrap()
        .with_mail_runtime(Arc::new(IdleMail))
        .with_configuration_runtime(Arc::new(ConfigRuntime(executed.clone())))
        .unwrap();
    daemon.connect().await.unwrap();
    assert!(!daemon.cycle().await.unwrap());
    assert_eq!(executed.load(Ordering::SeqCst), 1);
    {
        let receipts = core.receipts.lock().unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0], receipts[1]);
    }
    drop(daemon);
    stop.send(()).unwrap();
    server.await.unwrap();
}
