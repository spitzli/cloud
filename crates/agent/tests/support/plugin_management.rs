use super::*;
use std::sync::atomic::AtomicUsize;
use webdock_agent::plugin_runtime::{PluginRuntimeAuthority, PluginRuntimeExecutor};
use wire::plugin::{
    PluginRuntimeCheckpoint, PluginRuntimeClaim, PluginRuntimeCompletion, PluginRuntimeOutcome,
};
mod fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/plugin_runtime.rs"
    ));
}
struct State {
    core: Arc<Core>,
    command: wire::PluginRuntimeCommand,
    receipts: Mutex<Vec<wire::PluginRuntimeReceipt>>,
    finished: AtomicBool,
    lose_receipt: AtomicBool,
    supported_spec_version: AtomicUsize,
    requested_versions: Mutex<Vec<u32>>,
}
struct Service(Arc<State>);
#[tonic::async_trait]
impl wire::plugin_runtime_control_server::PluginRuntimeControl for Service {
    async fn poll_plugin_runtime(
        &self,
        r: Request<wire::PluginRuntimePollRequest>,
    ) -> std::result::Result<Response<wire::PluginRuntimeDispatch>, Status> {
        self.0.core.peer(&r)?;
        self.0
            .requested_versions
            .lock()
            .unwrap()
            .push(r.get_ref().max_spec_version);
        Ok(Response::new(wire::PluginRuntimeDispatch {
            command: (!self.0.finished.load(Ordering::SeqCst)).then(|| self.0.command.clone()),
            supported_spec_version: self.0.supported_spec_version.load(Ordering::SeqCst) as u32,
        }))
    }
    async fn guard_plugin_runtime(
        &self,
        r: Request<wire::PluginRuntimeGuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        self.0.core.peer(&r)?;
        Ok(Response::new(wire::GuardReply {
            allowed: r.get_ref().request_sha256 == self.0.command.request_sha256
                && r.get_ref().lease_json == self.0.command.lease_json,
        }))
    }
    async fn checkpoint_plugin_runtime(
        &self,
        r: Request<wire::PluginRuntimeCheckpointRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.core.peer(&r)?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_plugin_runtime(
        &self,
        r: Request<wire::PluginRuntimeReceipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.core.peer(&r)?;
        self.0.receipts.lock().unwrap().push(r.into_inner());
        self.0.finished.store(true, Ordering::SeqCst);
        if self.0.lose_receipt.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("lost receipt acknowledgment"));
        }
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
struct Executor(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl PluginRuntimeExecutor for Executor {
    async fn execute(
        &self,
        claim: &PluginRuntimeClaim,
        authority: &dyn PluginRuntimeAuthority,
    ) -> Result<PluginRuntimeCompletion> {
        authority.guard(claim).await?;
        self.0.fetch_add(1, Ordering::SeqCst);
        let checkpoint = PluginRuntimeCheckpoint {
            namespace_uid: Some("ns".into()),
            secret_uid: Some("config".into()),
            deployment_uid: Some("deploy".into()),
            service_uid: Some("service".into()),
            network_policy_uid: Some("policy".into()),
            route_uid: Some("route".into()),
        };
        authority.checkpoint(claim, checkpoint.clone()).await?;
        Ok(PluginRuntimeCompletion {
            outcome: PluginRuntimeOutcome::Succeeded,
            checkpoint,
            converged: true,
        })
    }
}
fn core() -> Arc<Core> {
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
fn config(dir: &std::path::Path, path: std::path::PathBuf) -> AgentConfig {
    let kubeconfig = dir.join("unused-kubeconfig");
    std::fs::write(&kubeconfig, b"unused test adapter").unwrap();
    std::fs::set_permissions(&kubeconfig, std::fs::Permissions::from_mode(0o600)).unwrap();
    AgentConfig {
        version: 1,
        installation: "test-install".into(),
        agent_id: "test-agent".into(),
        generation: 1,
        state_directory: dir.into(),
        control: ControlEndpoint::Unix { path },
        runtime: RuntimeConfiguration::Kubernetes {
            kubeconfig,
            context: "unused".into(),
            helm: "/unused".into(),
            chart: "/unused".into(),
            chart_sha256: "a".repeat(64),
            storage_socket: None,
            storage_node: None,
            public_routing: None,
        },
        database_runtime: None,
        mail_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: true,
    }
}
#[tokio::test]
async fn exact_plugin_receipt_replays_after_restart_and_lease_expiration_without_reexecution() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("core.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = config(dir.path(), socket);
    let mut claim = fixture::claim();
    claim.spec.platform_id = config.installation.clone();
    claim.spec.namespace = wire::plugin::PluginRuntimeSpec::namespace_name(
        &claim.spec.platform_id,
        &claim.spec.plugin_id,
    );
    claim.lease.lease_until = timestamp().unwrap() + 2;
    let private_marker = "plugin-private-key-must-not-enter-outbox";
    claim
        .material
        .files
        .insert("runtime.key".into(), private_marker.into());
    claim.spec.material_sha256 = claim.material.fingerprint().unwrap();

    let command = wire::PluginRuntimeCommand {
        spec_json: serde_json::to_vec(&claim.spec).unwrap(),
        lease_json: serde_json::to_vec(&claim.lease).unwrap(),
        request_sha256: claim.spec.fingerprint().unwrap(),
        material_json: serde_json::to_vec(&claim.material).unwrap(),
        checkpoint_json: vec![],
    };
    let state = Arc::new(State {
        core: core(),
        command,
        receipts: Mutex::new(Vec::new()),
        finished: AtomicBool::new(false),
        lose_receipt: AtomicBool::new(true),
        supported_spec_version: AtomicUsize::new(0),
        requested_versions: Mutex::new(Vec::new()),
    });
    let control = state.core.clone();
    let service = state.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(control)))
            .add_service(
                wire::plugin_runtime_control_server::PluginRuntimeControlServer::new(Service(
                    service,
                )),
            )
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let executions = Arc::new(AtomicUsize::new(0));
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled))
            .unwrap()
            .with_plugin_runtime(Arc::new(Executor(executions.clone())));
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.unwrap());
    }
    let outbox = dir.path().join("plugin-runtime-outbox.json");
    let contents = std::fs::read_to_string(&outbox).unwrap();
    assert!(!contents.contains("material_json"));
    let pending = webdock_agent::outbox::PluginRuntimeOutbox::open(&outbox)
        .unwrap()
        .pending()
        .unwrap()
        .unwrap();
    let encoded = prost::Message::encode_to_vec(&pending);
    assert!(
        !encoded
            .windows(private_marker.len())
            .any(|bytes| bytes == private_marker.as_bytes())
    );
    assert!(matches!(
        pending.callback,
        Some(wire::plugin_runtime_delivery::Callback::Receipt(_))
    ));

    while timestamp().unwrap() <= claim.lease.lease_until {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    {
        let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled))
            .unwrap()
            .with_plugin_runtime(Arc::new(Executor(executions.clone())));
        daemon.connect().await.unwrap();
        assert!(!daemon.cycle().await.unwrap());
    }
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    {
        let receipts = state.receipts.lock().unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0], receipts[1]);
    }
    stop.send(()).unwrap();
    server.await.unwrap();
}
struct CompatibilityControl(ControlService, Option<Arc<Mutex<Vec<Option<u64>>>>>);
#[tonic::async_trait]
impl AgentControl for CompatibilityControl {
    async fn register(
        &self,
        r: Request<wire::Hello>,
    ) -> std::result::Result<Response<wire::Welcome>, Status> {
        self.0.register(r).await
    }
    async fn heartbeat(
        &self,
        r: Request<wire::HeartbeatRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        let observation: serde_json::Value = serde_json::from_slice(&r.get_ref().observation_json)
            .map_err(|_| Status::invalid_argument("observation required"))?;
        let version = observation["capabilities"]
            .get("pluginRuntimeVersion")
            .and_then(serde_json::Value::as_u64);
        if let Some(versions) = &self.1 {
            versions.lock().unwrap().push(version);
        }
        if self.1.is_none() && version.is_some() {
            return Err(Status::invalid_argument("Unknown capability"));
        }
        self.0.heartbeat(r).await
    }
    async fn poll(
        &self,
        r: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::Dispatch>, Status> {
        self.0.poll(r).await
    }
    async fn guard(
        &self,
        r: Request<wire::GuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        self.0.guard(r).await
    }
    async fn complete(
        &self,
        r: Request<wire::Receipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.complete(r).await
    }
}
struct ObservedControlled;
#[async_trait::async_trait]
impl RuntimeAdapter for ObservedControlled {
    fn capabilities(&self) -> Capabilities {
        Controlled.capabilities()
    }
    async fn target_observation(&self) -> Result<Option<TargetObservation>> {
        Ok(Some(TargetObservation {
            version: "fixture".into(),
            nodes: vec![],
            runtime_kind: "kubernetes".into(),
            storage: None,
            environment: false,
            registry_version: None,
            service_bindings_version: None,
            public_routing_version: None,
            autoscaling_version: None,
        }))
    }
    async fn reconcile(&self, c: &Command, g: &LeaseGuard) -> Result<ObservedState> {
        Controlled.reconcile(c, g).await
    }
    async fn inspect(&self, s: &RuntimeSpec) -> Result<ObservedState> {
        Controlled.inspect(s).await
    }
    async fn logs(&self, s: &RuntimeSpec, tail: u32) -> Result<String> {
        Controlled.logs(s, tail).await
    }
}
#[tokio::test]
async fn absent_optional_domain_preserves_healthy_shared_agent_connection() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("core.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = config(dir.path(), socket);
    let core = core();
    let control = core.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(CompatibilityControl(
                ControlService(control),
                None,
            )))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let mut daemon = Daemon::with_adapter(config, Arc::new(ObservedControlled))
        .unwrap()
        .with_plugin_runtime(Arc::new(Executor(executions.clone())));
    daemon.connect().await.unwrap();
    assert!(!daemon.cycle().await.unwrap());
    assert!(!daemon.cycle().await.unwrap());
    assert_eq!(core.sequences.lock().unwrap().len(), 2);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    drop(daemon);
    stop.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn plugin_spec_negotiation_resets_after_same_generation_reconnect_to_old_core() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("core.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = config(dir.path(), socket);
    let state = Arc::new(State {
        core: core(),
        command: wire::PluginRuntimeCommand::default(),
        receipts: Mutex::new(Vec::new()),
        finished: AtomicBool::new(true),
        lose_receipt: AtomicBool::new(false),
        supported_spec_version: AtomicUsize::new(2),
        requested_versions: Mutex::new(Vec::new()),
    });
    let versions = Arc::new(Mutex::new(Vec::new()));
    let control = state.core.clone();
    let service = state.clone();
    let observed = versions.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(CompatibilityControl(
                ControlService(control),
                Some(observed),
            )))
            .add_service(
                wire::plugin_runtime_control_server::PluginRuntimeControlServer::new(Service(
                    service,
                )),
            )
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap();
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let mut daemon = Daemon::with_adapter(config, Arc::new(ObservedControlled))
        .unwrap()
        .with_plugin_runtime(Arc::new(Executor(executions)));
    daemon.connect().await.unwrap();
    assert!(!daemon.cycle().await.unwrap());
    assert!(!daemon.cycle().await.unwrap());
    state.supported_spec_version.store(0, Ordering::SeqCst);
    daemon.connect().await.unwrap();
    assert!(!daemon.cycle().await.unwrap());
    assert!(!daemon.cycle().await.unwrap());
    assert_eq!(
        *versions.lock().unwrap(),
        vec![None, Some(2), None, Some(1)]
    );
    assert_eq!(*state.requested_versions.lock().unwrap(), vec![2, 2, 2, 2]);
    stop.send(()).unwrap();
    server.await.unwrap();
}
