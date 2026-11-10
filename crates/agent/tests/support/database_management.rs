use super::*;
use spitzli_agent::database::{DatabaseAuthority, DatabaseRuntime, DatabaseStep};
use wire::database::*;
use wire::database_control_server::{DatabaseControl, DatabaseControlServer};
struct DatabaseCore {
    peer: Arc<Core>,
    command: Mutex<Option<wire::DatabaseCommand>>,
    checkpoints: Mutex<Vec<wire::DatabaseCheckpointRequest>>,
    receipts: Mutex<Vec<wire::DatabaseReceipt>>,
    lose_checkpoint: AtomicBool,
    lose_receipt: AtomicBool,
    guard_until: u64,
}
struct DatabaseService(Arc<DatabaseCore>);
#[tonic::async_trait]
impl DatabaseControl for DatabaseService {
    async fn poll_database(
        &self,
        r: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::DatabaseDispatch>, Status> {
        self.0.peer.peer(&r)?;
        Ok(Response::new(wire::DatabaseDispatch {
            command: self.0.command.lock().unwrap().take(),
        }))
    }
    async fn guard_database(
        &self,
        r: Request<wire::DatabaseGuardRequest>,
    ) -> std::result::Result<Response<wire::GuardReply>, Status> {
        self.0.peer.peer(&r)?;
        if self.0.guard_until != 0 {
            while timestamp().unwrap() <= self.0.guard_until {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
        Ok(Response::new(wire::GuardReply {
            allowed: self.0.peer.allowed.load(Ordering::SeqCst),
        }))
    }
    async fn checkpoint_database(
        &self,
        r: Request<wire::DatabaseCheckpointRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.peer.peer(&r)?;
        self.0.checkpoints.lock().unwrap().push(r.into_inner());
        if self.0.lose_checkpoint.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("lost checkpoint acknowledgment"));
        }
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_database(
        &self,
        r: Request<wire::DatabaseReceipt>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        self.0.peer.peer(&r)?;
        self.0.receipts.lock().unwrap().push(r.into_inner());
        if self.0.lose_receipt.swap(false, Ordering::SeqCst) {
            return Err(Status::unavailable("lost completion acknowledgment"));
        }
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn database_observation_config(
        &self,
        _: Request<wire::PollRequest>,
    ) -> std::result::Result<Response<wire::DatabaseObservationPlan>, Status> {
        Err(Status::unimplemented("fixture"))
    }
    async fn report_database_observations(
        &self,
        _: Request<wire::DatabaseObservationRequest>,
    ) -> std::result::Result<Response<wire::Ack>, Status> {
        Err(Status::unimplemented("fixture"))
    }
}
struct DatabaseFixture(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl DatabaseRuntime for DatabaseFixture {
    async fn execute(
        &self,
        c: &DatabaseClaim,
        a: &dyn DatabaseAuthority,
    ) -> spitzli_supervisor::Result<DatabaseStep> {
        a.guard(c).await?;
        self.0.fetch_add(1, Ordering::SeqCst);
        if c.spec.phase == DatabaseExecutionPhase::Apply {
            a.checkpoint(
                c,
                DatabaseCheckpoint {
                    lease: c.lease.clone(),
                    phase: DatabaseCheckpointPhase::Applied,
                    cnpg_uid: "fixture-cnpg".into(),
                    ca_certificate: None,
                    previous_pod_uid: None,
                    migration_job_uid: None,
                    copy_sha256: None,
                },
            )
            .await?;
            Ok(DatabaseStep::Pending)
        } else {
            Ok(DatabaseStep::Complete(DatabaseCompletionProof {
                installation_id: c.spec.backend.id.clone(),
                database_id: c.spec.database.as_ref().map(|d| d.id.clone()),
                cnpg_uid: "fixture-cnpg".into(),
                revision: 1,
                state: DatabaseProofState::Ready,
                endpoint_ready: true,
                copy_sha256: None,
                backup: None,
            }))
        }
    }
}
fn database_command(phase: DatabaseExecutionPhase) -> wire::DatabaseCommand {
    let mut spec:DatabaseJobSpec=serde_json::from_value(serde_json::json!({"version":1,"operationID":"101","agentGeneration":1,"operationGeneration":1,"clusterID":"789","action":"create","phase":"apply","revision":1,"previousPodUID":null,"backend":{"id":"456","kind":"dedicated","namespace":"wd-db-456","node":"fixture-node","poolID":"123e4567-e89b-42d3-a456-426614174000","revision":1,"cnpgUID":null,"resources":{"cpuMillicores":500,"memoryBytes":536870912,"volumeBytes":268435456},"host":"db-456.example.test","directPort":32000,"pooledPort":32001},"database":{"id":"123","kind":"dedicated","name":"db_123","revision":1,"credentialRevision":1,"connectionLimit":10},"members":[{"id":"123","projectID":"321","databaseName":"db_123","enabled":true}],"migration":null})).unwrap();
    spec.phase = phase;
    wire::DatabaseCommand {
        request_sha256: spec.fingerprint().unwrap(),
        spec_json: serde_json::to_vec(&spec).unwrap(),
        lease_json: serde_json::to_vec(&DatabaseLease {
            operation_id: "101".into(),
            generation: 1,
            lease_token: "a".repeat(64),
        })
        .unwrap(),
        lease_until: timestamp().unwrap() + 60,
        material_json: serde_json::to_vec(&DatabaseMaterial {
            credentials: Some(DatabaseCredentials {
                application_password: format!(
                    "secretFixturePasswordNeverPersisted{}",
                    "A".repeat(8)
                ),
                migration_password: "b".repeat(43),
            }),
            transfer_password: None,
            object_store: None,
        })
        .unwrap(),
    }
}
#[tokio::test]
async fn database_checkpoint_and_completion_replay_exactly_after_daemon_restart() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let peer = Arc::new(Core {
        commands: Mutex::new(VecDeque::new()),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: Default::default(),
    });
    let core = Arc::new(DatabaseCore {
        peer: peer.clone(),
        command: Mutex::new(Some(database_command(DatabaseExecutionPhase::Apply))),
        checkpoints: Mutex::new(vec![]),
        receipts: Mutex::new(vec![]),
        lose_checkpoint: AtomicBool::new(true),
        lose_receipt: AtomicBool::new(true),
        guard_until: 0,
    });
    let service = core.clone();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(peer)))
            .add_service(DatabaseControlServer::new(DatabaseService(service)))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                let _ = shutdown.await;
            })
            .await
            .unwrap()
    });
    let config = AgentConfig {
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
        database_runtime: None,
        recovery_runtime: None,
        mail_configuration_enabled: false,
        mail_runtime: None,
        policy: Default::default(),
        workloads: false,
        plugin_runtime_enabled: false,
    };
    let runtime = Arc::new(DatabaseFixture(std::sync::atomic::AtomicUsize::new(0)));
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled))
            .unwrap()
            .with_database_runtime(runtime.clone());
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
        assert_eq!(core.checkpoints.lock().unwrap().len(), 1);
    }
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled))
            .unwrap()
            .with_database_runtime(runtime.clone());
        daemon.connect().await.unwrap();
        {
            let checkpoints = core.checkpoints.lock().unwrap();
            assert_eq!(checkpoints.len(), 2);
            assert_eq!(checkpoints[0], checkpoints[1]);
        }
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
        *core.command.lock().unwrap() = Some(database_command(DatabaseExecutionPhase::Observe));
        assert!(daemon.cycle().await.unwrap());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 2);
    }
    {
        let mut daemon = Daemon::with_adapter(config.clone(), Arc::new(Controlled))
            .unwrap()
            .with_database_runtime(runtime.clone());
        daemon.connect().await.unwrap();
        {
            let receipts = core.receipts.lock().unwrap();
            assert_eq!(receipts.len(), 2);
            assert_eq!(receipts[0], receipts[1]);
        }
        assert_eq!(runtime.0.load(Ordering::SeqCst), 2);
        core.peer.allowed.store(false, Ordering::SeqCst);
        *core.command.lock().unwrap() = Some(database_command(DatabaseExecutionPhase::Observe));
        assert!(daemon.cycle().await.is_err());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 2);
    }
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|v| v == "json") {
            assert!(
                !std::fs::read_to_string(path)
                    .unwrap()
                    .contains("secretFixturePasswordNeverPersisted")
            );
        }
    }
    let _ = stop.send(());
    server.await.unwrap();
}

struct DatabaseEntry(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl DatabaseRuntime for DatabaseEntry {
    async fn execute(
        &self,
        _: &DatabaseClaim,
        _: &dyn DatabaseAuthority,
    ) -> spitzli_supervisor::Result<DatabaseStep> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ExecutionError::Runtime)
    }
}

#[tokio::test]
async fn delayed_database_guard_cannot_enter_runtime_after_lease_expiry() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = dir.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let peer = Arc::new(Core {
        commands: Mutex::new(VecDeque::new()),
        receipts: Mutex::new(vec![]),
        sequences: Mutex::new(vec![]),
        allowed: AtomicBool::new(true),
        certificate: Mutex::new(None),
        revoked: AtomicBool::new(false),
        fail_complete: AtomicBool::new(false),
        terminal_guard: AtomicBool::new(false),
        slow_read_guard_calls: Default::default(),
    });
    let mut command = database_command(DatabaseExecutionPhase::Apply);
    command.lease_until = timestamp().unwrap() + 2;
    let core = Arc::new(DatabaseCore {
        peer: peer.clone(),
        guard_until: command.lease_until,
        command: Mutex::new(Some(command)),
        checkpoints: Mutex::new(vec![]),
        receipts: Mutex::new(vec![]),
        lose_checkpoint: AtomicBool::new(false),
        lose_receipt: AtomicBool::new(false),
    });
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(AgentControlServer::new(ControlService(peer)))
            .add_service(DatabaseControlServer::new(DatabaseService(core)))
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
        state_directory: dir.path().into(),
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
    let runtime = Arc::new(DatabaseEntry(std::sync::atomic::AtomicUsize::new(0)));
    let mut daemon = Daemon::with_adapter(config, Arc::new(Controlled))
        .unwrap()
        .with_database_runtime(runtime.clone());
    daemon.connect().await.unwrap();
    let result = daemon.cycle().await;
    drop(daemon);
    stop.send(()).unwrap();
    server.await.unwrap();
    assert_eq!(
        runtime.0.load(Ordering::SeqCst),
        0,
        "expired dispatch entered runtime"
    );
    assert!(matches!(
        result,
        Err(spitzli_agent::management::AgentError::Execution(
            ExecutionError::Expired
        ))
    ));
}
