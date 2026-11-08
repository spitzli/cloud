mod support;
use std::{
    collections::VecDeque,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status};
use webdock_agent::{
    config::{AgentConfig, ControlEndpoint, RuntimeConfiguration},
    management::{
        Daemon, wire,
        wire::agent_control_server::{AgentControl, AgentControlServer},
    },
};
use webdock_supervisor::*;
struct Core {
    commands: Mutex<VecDeque<wire::Command>>,
    receipts: Mutex<Vec<wire::Receipt>>,
    sequences: Mutex<Vec<u64>>,
    allowed: AtomicBool,
    certificate: Mutex<Option<Vec<u8>>>,
    revoked: AtomicBool,
    fail_complete: AtomicBool,
    terminal_guard: AtomicBool,
    slow_read_guard_calls: AtomicUsize,
}
impl Core {
    fn peer<T>(&self, request: &Request<T>) -> std::result::Result<(), Status> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(Status::permission_denied("revoked"));
        }
        let cert = self.certificate.lock().unwrap();
        let peer = webdock_agent_protocol::transport_identity(
            request,
            timestamp().map_err(|_| Status::unavailable("clock"))?,
        )?;
        let valid = match (cert.as_ref(), peer) {
            (Some(expected), webdock_agent_protocol::TransportIdentity::MutualTls(actual)) => {
                use sha2::{Digest, Sha256};
                actual.sha256 == format!("{:x}", Sha256::digest(expected))
            }
            (None, webdock_agent_protocol::TransportIdentity::Unix { uid, .. }) => {
                uid == rustix::process::geteuid().as_raw()
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(Status::permission_denied("peer"))
        }
    }
}
struct ControlService(Arc<Core>);
#[tonic::async_trait]
impl AgentControl for ControlService {
    async fn register(
        &self,
        r: Request<wire::Hello>,
    ) -> std::result::Result<Response<wire::Welcome>, Status> {
        self.0.peer(&r)?;
        let h = r.into_inner();
        Ok(Response::new(wire::Welcome {
            accepted: h
                .identity
                .as_ref()
                .is_some_and(|i| i.agent_id == "test-agent"),
            protocol_version: 1,
            generation: 1,
        }))
    }
    async fn heartbeat(
        &self,
        r: Request<wire::HeartbeatRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.peer(&r)?;
        let seq = r.into_inner().sequence;
        let mut values = self.0.sequences.lock().unwrap();
        if values.last().is_some_and(|v| *v >= seq) {
            return Err(Status::permission_denied("sequence"));
        }
        values.push(seq);
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn poll(
        &self,
        r: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::Dispatch>, Status> {
        self.0.peer(&r)?;
        Ok(Response::new(wire::Dispatch {
            command: self.0.commands.lock().unwrap().pop_front(),
        }))
    }
    async fn guard(
        &self,
        r: Request<wire::GuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        self.0.peer(&r)?;
        if r.get_ref().operation_id == "remote-latency-read"
            && self.0.slow_read_guard_calls.fetch_add(1, Ordering::SeqCst) > 0
        {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if r.get_ref().operation_id == "revoke-pending-read"
            && self.0.slow_read_guard_calls.fetch_add(1, Ordering::SeqCst) > 0
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            self.0.allowed.store(false, Ordering::SeqCst);
        }
        let terminal = self.0.terminal_guard.load(Ordering::SeqCst)
            && self.0.receipts.lock().unwrap().iter().any(|receipt| {
                receipt.operation_id == r.get_ref().operation_id
                    && receipt.outcome == wire::Outcome::Succeeded as i32
            });
        Ok(Response::new(wire::GuardReply {
            allowed: !terminal
                && self.0.allowed.load(Ordering::SeqCst)
                && r.get_ref().authorization == "test-reference",
        }))
    }
    async fn complete(
        &self,
        r: Request<wire::Receipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.peer(&r)?;
        self.0.receipts.lock().unwrap().push(r.into_inner());
        if self.0.fail_complete.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("lost completion acknowledgment"));
        }
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
struct Controlled;
struct DelayedMail {
    command: Mutex<Option<wire::MailCommand>>,
    until: u64,
    replied: Arc<AtomicBool>,
}
#[tonic::async_trait]
impl wire::mail_control_server::MailControl for DelayedMail {
    async fn poll_mail(
        &self,
        _: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::MailDispatch>, Status> {
        Ok(Response::new(wire::MailDispatch {
            command: self.command.lock().unwrap().take(),
        }))
    }
    async fn guard_mail(
        &self,
        _: Request<wire::MailGuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        while timestamp().unwrap() <= self.until {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        self.replied.store(true, Ordering::SeqCst);
        Ok(Response::new(wire::GuardReply { allowed: true }))
    }
    async fn checkpoint_mail(
        &self,
        _: Request<wire::MailCheckpointRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_mail(
        &self,
        _: Request<wire::MailReceipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
struct MailEffect(Arc<AtomicBool>);
#[async_trait::async_trait]
impl webdock_agent::mail::MailRuntime for MailEffect {
    async fn execute(
        &self,
        _: &wire::mail::MailClaim,
        _: &dyn webdock_agent::mail::MailAuthority,
    ) -> Result<wire::mail::MailProof> {
        self.0.store(true, Ordering::SeqCst);
        Err(ExecutionError::Runtime)
    }
}
#[async_trait::async_trait]
impl RuntimeAdapter for Controlled {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            logs: true,
            ..Capabilities::default()
        }
    }
    async fn reconcile(&self, c: &Command, g: &LeaseGuard) -> Result<ObservedState> {
        g.check_for(c).await?;
        self.inspect(&c.spec).await
    }
    async fn inspect(&self, s: &RuntimeSpec) -> Result<ObservedState> {
        Ok(ObservedState {
            autoscaling: None,
            resource_id: s.resource_id.to_string(),
            revision: s.revision,
            phase: Phase::Running,
            ready: true,
            observed_at: timestamp()?,
            runtime_id: Some("controlled-test".into()),
            conditions: vec![],
            storage: None,
        })
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String> {
        Ok("test log\n".into())
    }
}
struct ChangingRead(AtomicBool);
#[async_trait::async_trait]
impl RuntimeAdapter for ChangingRead {
    fn capabilities(&self) -> Capabilities {
        Controlled.capabilities()
    }
    async fn reconcile(&self, c: &Command, g: &LeaseGuard) -> Result<ObservedState> {
        Controlled.reconcile(c, g).await
    }
    async fn inspect(&self, s: &RuntimeSpec) -> Result<ObservedState> {
        let mut observed = Controlled.inspect(s).await?;
        if self.0.load(Ordering::SeqCst) {
            observed.runtime_id = Some("replaced-test".into());
        }
        Ok(observed)
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String> {
        self.0.store(true, Ordering::SeqCst);
        Ok("discard-this-read".into())
    }
}
fn command(intent: wire::Intent, id: &str) -> wire::Command {
    wire::Command {
        material_json: Vec::new(),
        operation_id: id.into(),
        generation: 1,
        lease_until: timestamp().unwrap() + 60,
        authorization: "test-reference".into(),
        intent: intent as i32,
        spec_json: serde_json::to_vec(&support::spec()).unwrap(),
        log_tail: if intent == wire::Intent::Logs { 10 } else { 0 },
        expected_runtime_id: None,
    }
}
#[tokio::test]
async fn outbound_unix_dispatch_revalidates_reads_and_replays_committed_work_after_restart() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::from([
            command(wire::Intent::Apply, "operation-1"),
            command(wire::Intent::Logs, "read-1"),
        ])),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled)).unwrap();
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert!(daemon.cycle().await.unwrap());
    }
    assert_eq!(
        core.receipts.lock().unwrap()[1].result,
        Some(wire::receipt::Result::LogsUtf8(b"test log\n".to_vec()))
    );
    core.commands
        .lock()
        .unwrap()
        .push_back(command(wire::Intent::Apply, "operation-1"));
    let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled)).unwrap();
    daemon.connect().await.unwrap();
    daemon.cycle().await.unwrap();
    {
        let receipts = core.receipts.lock().unwrap();
        assert_eq!(receipts[0].result, receipts[2].result);
    }
    core.fail_complete.store(true, Ordering::SeqCst);
    core.commands
        .lock()
        .unwrap()
        .push_back(command(wire::Intent::Apply, "operation-1"));
    assert!(daemon.cycle().await.is_err());
    daemon.connect().await.unwrap();
    core.commands
        .lock()
        .unwrap()
        .push_back(command(wire::Intent::Apply, "operation-1"));
    daemon.cycle().await.unwrap();
    {
        let receipts = core.receipts.lock().unwrap();
        assert_eq!(receipts[3].result, receipts[4].result);
    }
    let mut expired = command(wire::Intent::Logs, "expired-read");
    expired.lease_until = 0;
    core.commands.lock().unwrap().push_back(expired);
    daemon.cycle().await.unwrap();
    {
        let receipts = core.receipts.lock().unwrap();
        assert_eq!(receipts.last().unwrap().error_code, "lease_expired");
        assert!(receipts.last().unwrap().result.is_none());
    }
    core.allowed.store(false, Ordering::SeqCst);
    core.commands
        .lock()
        .unwrap()
        .push_back(command(wire::Intent::Logs, "read-denied"));
    daemon.cycle().await.unwrap();
    assert_eq!(
        core.receipts.lock().unwrap().last().unwrap().outcome,
        wire::Outcome::Rejected as i32
    );
    drop(daemon);
    core.allowed.store(true, Ordering::SeqCst);
    let changing = Arc::new(ChangingRead(AtomicBool::new(false)));
    let mut daemon = Daemon::with_adapter(config, changing.clone()).unwrap();
    daemon.connect().await.unwrap();
    let mut wrong = command(wire::Intent::Logs, "wrong-read-uid");
    wrong.expected_runtime_id = Some("foreign-test".into());
    core.commands.lock().unwrap().push_back(wrong);
    daemon.cycle().await.unwrap();
    assert!(!changing.0.load(Ordering::SeqCst));
    assert!(
        core.receipts
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .result
            .is_none()
    );
    let mut changed = command(wire::Intent::Logs, "changed-read-uid");
    changed.expected_runtime_id = Some("controlled-test".into());
    core.commands.lock().unwrap().push_back(changed);
    daemon.cycle().await.unwrap();
    assert!(changing.0.load(Ordering::SeqCst));
    assert!(
        core.receipts
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .result
            .is_none()
    );
    tx.send(()).unwrap();
    drop(daemon);
    server.await.unwrap();
}

fn ca() -> (rcgen::Certificate, rcgen::Issuer<'static, rcgen::KeyPair>) {
    let mut p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    (cert, rcgen::Issuer::new(p, key))
}
fn leaf(
    issuer: &rcgen::Issuer<'static, rcgen::KeyPair>,
    server: bool,
    expired: bool,
) -> (rcgen::Certificate, rcgen::KeyPair) {
    let mut p = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    p.extended_key_usages = vec![if server {
        rcgen::ExtendedKeyUsagePurpose::ServerAuth
    } else {
        rcgen::ExtendedKeyUsagePurpose::ClientAuth
    }];
    if expired {
        p.not_before = rcgen::date_time_ymd(2000, 1, 1);
        p.not_after = rcgen::date_time_ymd(2001, 1, 1);
    }
    let key = rcgen::KeyPair::generate().unwrap();
    (p.signed_by(&key, issuer).unwrap(), key)
}
fn replace_private(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
    file.write_all(bytes).unwrap();
    file.as_file().sync_all().unwrap();
    file.persist(path).unwrap();
}
#[tokio::test]
async fn outbound_mtls_rotates_certificates_and_stops_after_live_enrollment_revocation() {
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::{Certificate, Identity, ServerTlsConfig};
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let (root, issuer) = ca();
    let (server_cert, server_key) = leaf(&issuer, true, false);
    let (client, client_key) = leaf(&issuer, false, false);
    let (next, next_key) = leaf(&issuer, false, false);
    let ca_path = dir.path().join("ca.pem");
    let cert_path = dir.path().join("client.pem");
    let key_path = dir.path().join("client.key");
    replace_private(&ca_path, root.pem().as_bytes());
    replace_private(&cert_path, client.pem().as_bytes());
    replace_private(&key_path, client_key.serialize_pem().as_bytes());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::new()),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(Some(client.der().to_vec())),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(
            server_cert.pem(),
            server_key.serialize_pem(),
        ))
        .client_ca_root(Certificate::from_pem(root.pem()));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Mtls {
            origin,
            domain: "localhost".into(),
            ca: ca_path,
            certificate: cert_path.clone(),
            key: key_path.clone(),
        },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled)).unwrap();
    daemon.connect().await.unwrap();
    daemon.cycle().await.unwrap();
    replace_private(&cert_path, next.pem().as_bytes());
    replace_private(&key_path, next_key.serialize_pem().as_bytes());
    *core.certificate.lock().unwrap() = Some(next.der().to_vec());
    assert!(daemon.cycle().await.is_err());
    daemon.connect().await.unwrap();
    daemon.cycle().await.unwrap();
    assert_eq!(&*core.sequences.lock().unwrap(), &[1, 2]);
    core.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        daemon.cycle().await,
        Err(webdock_agent::management::AgentError::Revoked)
    ));
    drop(daemon);
    assert!(Daemon::with_adapter(config.clone(), Arc::new(Controlled)).is_err());
    let (expired, key) = leaf(&issuer, false, true);
    replace_private(&cert_path, expired.pem().as_bytes());
    replace_private(&key_path, key.serialize_pem().as_bytes());
    assert!(Daemon::with_adapter(config.clone(), Arc::new(Controlled)).is_err());
    core.revoked.store(false, Ordering::SeqCst);
    let (_, other) = ca();
    let (untrusted, key) = leaf(&other, false, false);
    replace_private(&cert_path, untrusted.pem().as_bytes());
    replace_private(&key_path, key.serialize_pem().as_bytes());
    let mut untrusted = Daemon::with_adapter(config, Arc::new(Controlled)).unwrap();
    assert!(untrusted.connect().await.is_err());
    drop(untrusted);
    tx.send(()).unwrap();
    server.await.unwrap();
}
#[test]
fn cli_is_runnable_and_documents_explicit_configuration() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_webdock-agent"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("--config")
    );
}

#[cfg(feature = "docker")]
#[tokio::test]
#[ignore = "requires explicit disposable WEBDOCK_DOCKER_TEST_IMAGE and local Engine"]
async fn real_docker_executor_obeys_outbound_control_leases_and_reports_observations() {
    use bollard::{Docker, query_parameters::*};
    let image = std::env::var("WEBDOCK_DOCKER_TEST_IMAGE").unwrap();
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut spec = support::spec();
    spec.installation = format!("test-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    spec.image = image;
    let mut apply = command(wire::Intent::Apply, "docker-apply");
    apply.spec_json = serde_json::to_vec(&spec).unwrap();
    let mut stop = command(wire::Intent::Stop, "docker-stop");
    stop.spec_json = apply.spec_json.clone();
    stop.generation = 2;
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::from([apply, stop])),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: spec.installation.clone(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    let outcome = tokio::spawn(async move {
        let mut daemon = Daemon::open(config).await.unwrap();
        daemon.connect().await.unwrap();
        daemon.cycle().await.unwrap();
        daemon.cycle().await.unwrap();
    })
    .await;
    let docker =
        Docker::connect_with_unix("/var/run/docker.sock", 10, bollard::API_DEFAULT_VERSION)
            .unwrap()
            .negotiate_version()
            .await
            .unwrap();
    let _ = docker
        .remove_container(
            &spec.name(),
            Some(
                RemoveContainerOptionsBuilder::default()
                    .force(true)
                    .v(false)
                    .build(),
            ),
        )
        .await;
    let _ = docker.remove_network(&format!("{}-net", spec.name())).await;
    tx.send(()).unwrap();
    server.await.unwrap();
    outcome.unwrap();
    let receipts = core.receipts.lock().unwrap();
    assert_eq!(receipts.len(), 2);
    for receipt in receipts.iter() {
        assert_eq!(receipt.outcome, wire::Outcome::Succeeded as i32);
    }
    let Some(wire::receipt::Result::ObservedJson(bytes)) = &receipts[1].result else {
        panic!("missing observation")
    };
    let observed: ObservedState = serde_json::from_slice(bytes).unwrap();
    assert_eq!(observed.phase, Phase::Stopped);
}

#[tokio::test]
async fn lost_terminal_completion_ack_replays_durable_outbox_before_poll_without_execution_guard() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::from([command(
            wire::Intent::Apply,
            "terminal-operation",
        )])),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(true),
        terminal_guard: AtomicBool::new(true),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled)).unwrap();
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.is_err());
    }
    assert_eq!(core.receipts.lock().unwrap().len(), 1);
    // The stored operation is terminal; receipt custody must survive continued delivery failure.
    core.commands
        .lock()
        .unwrap()
        .push_back(command(wire::Intent::Apply, "next-operation"));
    core.fail_complete.store(true, Ordering::SeqCst);
    let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled)).unwrap();
    daemon.connect().await.unwrap();
    let heartbeats = core.sequences.lock().unwrap().len();
    core.fail_complete.store(true, Ordering::SeqCst);
    assert!(!daemon.cycle().await.unwrap());
    assert!(core.sequences.lock().unwrap().len() > heartbeats);
    assert_eq!(
        core.commands.lock().unwrap().len(),
        1,
        "pending custody prevents new Hosting dispatch"
    );
    assert!(daemon.cycle().await.unwrap());
    {
        let receipts = core.receipts.lock().unwrap();
        assert_eq!(receipts.len(), 5);
        assert!(receipts[..4].iter().all(|receipt| receipt == &receipts[0]));
        assert_eq!(receipts[0].outcome, wire::Outcome::Succeeded as i32);
        assert_eq!(receipts[4].operation_id, "next-operation");
    }
    assert!(
        webdock_agent::outbox::ReceiptOutbox::open(dir.path().join("receipt-outbox.json"))
            .unwrap()
            .pending()
            .unwrap()
            .is_none()
    );
    tx.send(()).unwrap();
    drop(daemon);
    server.await.unwrap();
}

#[tokio::test]
async fn delayed_mail_guard_reply_cannot_authorize_runtime_after_lease_expiry() {
    use wire::mail::{MailAction, MailLease, MailMaterial, MailResources, MailSpec};
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::new()),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let material = MailMaterial::Credentials {
        bootstrap_password: Some("owned-fixture-bootstrap".into()),
        username: "operator@mail-123.example.test".into(),
        password: "owned-fixture-permanent".into(),
    };
    let spec = MailSpec {
        version: 1,
        operation_id: "456".into(),
        customer_id: "123".into(),
        cluster_id: "789".into(),
        revision: 1,
        agent_generation: 1,
        instance_key: "mail-123".into(),
        hostname: "mail-123.example.test".into(),
        image: format!("example.test/mail@sha256:{}", "a".repeat(64)),
        resources: MailResources::FIXED,
        material_sha256: material.fingerprint().unwrap(),
        action: MailAction::Ensure,
    };
    let until = timestamp().unwrap() + 2;
    let command = wire::MailCommand {
        request_sha256: spec.fingerprint().unwrap(),
        spec_json: serde_json::to_vec(&spec).unwrap(),
        lease_json: serde_json::to_vec(&MailLease {
            lease_token: uuid::Uuid::new_v4().to_string(),
        })
        .unwrap(),
        lease_until: until,
        material_json: serde_json::to_vec(&material).unwrap(),
        checkpoint_json: vec![],
    };
    let replied = Arc::new(AtomicBool::new(false));
    let reply = replied.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(core)))
            .add_service(wire::mail_control_server::MailControlServer::new(
                DelayedMail {
                    command: Mutex::new(Some(command)),
                    until,
                    replied: reply,
                },
            ))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let config = AgentConfig {
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: directory.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/unused/fixture.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled))
        .unwrap()
        .with_mail_runtime(Arc::new(MailEffect(executed.clone())));
    daemon.connect().await.unwrap();
    let result = daemon.cycle().await;
    drop(daemon);
    stop.send(()).unwrap();
    server.await.unwrap();
    assert!(
        replied.load(Ordering::SeqCst),
        "the controlled late reply was not reached"
    );
    assert!(matches!(
        result,
        Err(webdock_agent::management::AgentError::Execution(
            ExecutionError::Expired
        ))
    ));
    assert!(
        !executed.load(Ordering::SeqCst),
        "runtime executed after its guard reply crossed the lease deadline"
    );
}

#[cfg(feature = "kubernetes")]
#[path = "support/database_management.rs"]
mod database_management;

#[path = "support/configuration_management.rs"]
mod configuration_management;

#[cfg(feature = "kubernetes")]
#[path = "support/plugin_management.rs"]
mod plugin_management;

struct NetworkDelayedRead;
#[async_trait::async_trait]
impl RuntimeAdapter for NetworkDelayedRead {
    fn capabilities(&self) -> Capabilities {
        Controlled.capabilities()
    }
    async fn reconcile(&self, c: &Command, g: &LeaseGuard) -> Result<ObservedState> {
        Controlled.reconcile(c, g).await
    }
    async fn inspect(&self, s: &RuntimeSpec) -> Result<ObservedState> {
        Controlled.inspect(s).await
    }
    async fn logs(&self, s: &RuntimeSpec, tail: u32) -> Result<String> {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Controlled.logs(s, tail).await
    }
}
#[tokio::test]
async fn remote_authorization_latency_cannot_starve_a_live_runtime_read() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::from([{
            let mut value = command(wire::Intent::Logs, "remote-latency-read");
            value.expected_runtime_id = Some("controlled-test".into());
            value
        }])),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };

    let mut daemon = Daemon::with_adapter(config, Arc::new(NetworkDelayedRead)).unwrap();
    daemon.connect().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_millis(1700), daemon.cycle())
        .await
        .expect("periodic authorization blocked runtime progress")
        .unwrap();
    {
        let receipts = core.receipts.lock().unwrap();
        assert_eq!(
            receipts.last().unwrap().outcome,
            wire::Outcome::Succeeded as i32
        );
        assert_eq!(
            receipts.last().unwrap().result,
            Some(wire::receipt::Result::LogsUtf8(b"test log\n".to_vec()))
        );
    }
    drop(daemon);
    let _ = tx.send(());
    server.await.unwrap();
}

struct PendingRead(Arc<AtomicBool>);
#[async_trait::async_trait]
impl RuntimeAdapter for PendingRead {
    fn capabilities(&self) -> Capabilities {
        Controlled.capabilities()
    }
    async fn reconcile(&self, c: &Command, g: &LeaseGuard) -> Result<ObservedState> {
        Controlled.reconcile(c, g).await
    }
    async fn inspect(&self, s: &RuntimeSpec) -> Result<ObservedState> {
        Controlled.inspect(s).await
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String> {
        self.0.store(true, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Ok("must-not-escape".into())
    }
}
#[tokio::test]
async fn periodic_revocation_cancels_pending_runtime_reads_without_releasing_output() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::from([command(
            wire::Intent::Logs,
            "revoke-pending-read",
        )])),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };

    let started = Arc::new(AtomicBool::new(false));
    let mut daemon = Daemon::with_adapter(config, Arc::new(PendingRead(started.clone()))).unwrap();
    daemon.connect().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), daemon.cycle())
        .await
        .unwrap()
        .unwrap();
    assert!(
        started.load(Ordering::SeqCst),
        "runtime must have started before revocation"
    );
    {
        let receipts = core.receipts.lock().unwrap();
        let receipt = receipts.last().unwrap();
        assert_eq!(receipt.outcome, wire::Outcome::Rejected as i32);
        assert_eq!(receipt.error_code, "authorization_denied");
        assert!(receipt.result.is_none());
    }
    drop(daemon);
    let _ = tx.send(());
    server.await.unwrap();
}

struct RevokeAfterRead(Arc<Core>);
#[async_trait::async_trait]
impl RuntimeAdapter for RevokeAfterRead {
    fn capabilities(&self) -> Capabilities {
        Controlled.capabilities()
    }
    async fn reconcile(&self, c: &Command, g: &LeaseGuard) -> Result<ObservedState> {
        Controlled.reconcile(c, g).await
    }
    async fn inspect(&self, s: &RuntimeSpec) -> Result<ObservedState> {
        Controlled.inspect(s).await
    }
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> Result<String> {
        self.0.allowed.store(false, Ordering::SeqCst);
        Ok("must-not-leave-agent".into())
    }
}
#[tokio::test]
async fn fresh_final_guard_rejects_output_when_authority_changes_at_read_completion() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let core = Arc::new(Core {
        commands: Mutex::new(VecDeque::from([command(
            wire::Intent::Logs,
            "revoked-after-read",
        )])),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: AtomicUsize::new(0),
    });
    let (tx, rx) = tokio::sync::oneshot::channel();
    let service = core.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = rx.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.path().into(),
        control: ControlEndpoint::Unix { path: socket },
        runtime: RuntimeConfiguration::Docker {
            socket: "/var/run/docker.sock".into(),
            initializer_image: None,
            storage_socket: None,
        },
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };

    let mut daemon = Daemon::with_adapter(config, Arc::new(RevokeAfterRead(core.clone()))).unwrap();
    daemon.connect().await.unwrap();
    daemon.cycle().await.unwrap();
    {
        let receipts = core.receipts.lock().unwrap();
        let receipt = receipts.last().unwrap();
        assert_eq!(receipt.outcome, wire::Outcome::Rejected as i32);
        assert_eq!(receipt.error_code, "authorization_denied");
        assert!(receipt.result.is_none());
    }
    drop(daemon);
    let _ = tx.send(());
    server.await.unwrap();
}
