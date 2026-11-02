//! Real restricted authority with controlled runtimes; no Docker/Kubernetes mutations.
#[path = "support/database_common.rs"]
mod support;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use support::{connection, exec, identity, row, url};
use webdock_agent::{
    config::{AgentConfig, ControlEndpoint, RuntimeConfiguration},
    database::{DatabaseAuthority, DatabaseRuntime, DatabaseStep},
    management::Daemon,
};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::{self as wire, database::*};
use webdock_databases::{DatabaseService, StoredAuthority};
use webdock_supervisor::{
    Capabilities, Command, ExecutionError, LeaseGuard, ObservedState, Phase, RuntimeAdapter,
    RuntimeSpec,
};
#[derive(Clone, Copy, Debug)]
enum Failure {
    RevokedSession,
    Expired,
    MissingCapability,
    Replaced,
    ReplacedRevision,
    ExpiredCheckpoint,
    ExpiredSuccess,
}
struct Interrupted {
    owner: DatabaseConnection,
    actor_session: String,
    mode: Failure,
    claims: Mutex<Vec<(DatabaseJobSpec, DatabaseLease)>>,
    executions: AtomicUsize,
}
#[async_trait::async_trait]
impl DatabaseRuntime for Interrupted {
    async fn execute(
        &self,
        claim: &DatabaseClaim,
        authority: &dyn DatabaseAuthority,
    ) -> webdock_supervisor::Result<DatabaseStep> {
        authority.guard(claim).await?;
        self.executions.fetch_add(1, Ordering::SeqCst);
        self.claims
            .lock()
            .unwrap()
            .push((claim.spec.clone(), claim.lease.clone()));
        match self.mode {
            Failure::ExpiredCheckpoint|Failure::ExpiredSuccess=>panic!("stale deliveries must never execute"),
            Failure::RevokedSession=>exec(&self.owner,"DELETE FROM webdock_auth.session WHERE id=$1",vec![self.actor_session.clone().into()]).await,
            Failure::Expired=>exec(&self.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await,
            Failure::MissingCapability=>exec(&self.owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,databaseVersion}','0'::jsonb) WHERE cluster_id=$1",vec![claim.spec.cluster_id.clone().into()]).await,
            Failure::ReplacedRevision=>exec(&self.owner,"UPDATE webdock_auth.hosting_database SET revision=revision+1 WHERE id=$1",vec![claim.spec.database.as_ref().unwrap().id.clone().into()]).await,
            Failure::Replaced=>{
                exec(&self.owner,"UPDATE webdock_auth.hosting_operation SET generation=generation+1,status='queued',lease_until=NULL WHERE id=$1",vec![claim.spec.operation_id.clone().into()]).await;
                exec(&self.owner,"UPDATE webdock_auth.hosting_database_job SET lease_token_hash=NULL,checkpoint=NULL WHERE operation_id=$1",vec![claim.spec.operation_id.clone().into()]).await;
            }
        }
        assert!(authority.guard(claim).await.is_err());
        Err(ExecutionError::Denied)
    }
}
struct Apps(AtomicUsize);
#[async_trait::async_trait]
impl RuntimeAdapter for Apps {
    async fn logs(&self, _: &RuntimeSpec, _: u32) -> webdock_supervisor::Result<String> {
        Err(ExecutionError::Unsupported("fixture logs"))
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            health: true,
            upgrades: true,
            ..Default::default()
        }
    }
    async fn reconcile(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> webdock_supervisor::Result<ObservedState> {
        guard.check_for(command).await?;
        self.0.fetch_add(1, Ordering::SeqCst);
        self.inspect(&command.spec).await
    }
    async fn inspect(&self, spec: &RuntimeSpec) -> webdock_supervisor::Result<ObservedState> {
        Ok(ObservedState {
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Running,
            ready: true,
            observed_at: webdock_supervisor::timestamp()?,
            runtime_id: Some("owned-controlled-app".into()),
            conditions: vec![],
            storage: None,
        })
    }
}
type DbClient = wire::database_control_client::DatabaseControlClient<tonic::transport::Channel>;
async fn deliver(
    client: &mut DbClient,
    delivery: &wire::DatabaseDelivery,
) -> std::result::Result<wire::Ack, tonic::Status> {
    match delivery.callback.as_ref().unwrap() {
        wire::database_delivery::Callback::Checkpoint(value) => client
            .checkpoint_database(value.clone())
            .await
            .map(tonic::Response::into_inner),
        wire::database_delivery::Callback::Receipt(value) => client
            .complete_database(value.clone())
            .await
            .map(tonic::Response::into_inner),
    }
}
fn checkpoint(
    spec: &DatabaseJobSpec,
    lease: &DatabaseLease,
    identity: wire::AgentIdentity,
    phase: DatabaseCheckpointPhase,
) -> wire::DatabaseDelivery {
    wire::DatabaseDelivery {
        callback: Some(wire::database_delivery::Callback::Checkpoint(
            wire::DatabaseCheckpointRequest {
                identity: Some(identity),
                spec_json: serde_json::to_vec(spec).unwrap(),
                request_sha256: spec.fingerprint().unwrap(),
                checkpoint_json: serde_json::to_vec(&DatabaseCheckpoint {
                    lease: lease.clone(),
                    phase,
                    cnpg_uid: "owned-callback-instance".into(),
                    ca_certificate: None,
                    previous_pod_uid: None,
                    migration_job_uid: None,
                    copy_sha256: None,
                })
                .unwrap(),
            },
        )),
    }
}
async fn poll(
    client: &mut DbClient,
    identity: wire::AgentIdentity,
) -> (DatabaseJobSpec, DatabaseLease) {
    let raw = client
        .poll_database(wire::PollRequest {
            identity: Some(identity),
        })
        .await
        .unwrap()
        .into_inner()
        .command
        .unwrap();
    let claim = webdock_agent::management::database_claim(&raw, 1).unwrap();
    (claim.spec, claim.lease)
}
fn store(config: &AgentConfig, delivery: &wire::DatabaseDelivery) {
    let _journal =
        webdock_supervisor::Journal::open(config.state_directory.join("commands.json")).unwrap();
    webdock_agent::outbox::DatabaseOutbox::open(
        config.state_directory.join("database-outbox.json"),
    )
    .unwrap()
    .put(delivery)
    .unwrap();
}
fn changed(mut delivery: wire::DatabaseDelivery, fingerprint: bool) -> wire::DatabaseDelivery {
    match delivery.callback.as_mut().unwrap() {
        wire::database_delivery::Callback::Checkpoint(value) => {
            if fingerprint {
                value.request_sha256 = "f".repeat(64);
            } else {
                let mut data: DatabaseCheckpoint =
                    serde_json::from_slice(&value.checkpoint_json).unwrap();
                data.cnpg_uid = "changed-receipt".into();
                value.checkpoint_json = serde_json::to_vec(&data).unwrap();
            }
        }
        wire::database_delivery::Callback::Receipt(value) => {
            if fingerprint {
                value.request_sha256 = "f".repeat(64);
            } else {
                let mut data: DatabaseCompletion =
                    serde_json::from_slice(&value.completion_json).unwrap();
                data.proof = Some(DatabaseCompletionProof {
                    installation_id: "123".into(),
                    database_id: None,
                    cnpg_uid: "changed-receipt".into(),
                    revision: 1,
                    state: DatabaseProofState::Ready,
                    endpoint_ready: true,
                    copy_sha256: None,
                    backup: None,
                });
                value.completion_json = serde_json::to_vec(&data).unwrap();
            }
        }
    }
    delivery
}

async fn case(name: String, owner: DatabaseConnection, mode: Failure) {
    eprintln!("database receipt case: {mode:?}");
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &url("AUTH_DATABASE_URL", &name),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let auth = connection(&name, "AUTH_DATABASE_URL").await;
    let business = connection(&name, "DATABASE_URL").await;
    let worker = connection(&name, "WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await;
    let (identity, actor) = identity(auth.clone(), ids.clone()).await;
    let (_, app_actor) = support::identity(auth.clone(), ids.clone()).await;
    let cluster = ids.allocate().await.unwrap().to_string();
    let customer = ids.allocate().await.unwrap().to_string();
    let project = ids.allocate().await.unwrap().to_string();
    let app_project = ids.allocate().await.unwrap().to_string();
    let installation = format!("delivery-{}", &cluster[cluster.len() - 12..]);
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let rls = std::env::var("DATABASE_RLS_SECRET").unwrap();
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            business.clone(),
            auth.clone(),
            worker.clone(),
            rls.clone(),
            secret.clone(),
            ids.clone(),
        )
        .await
        .unwrap(),
    );
    let databases = Arc::new(
        DatabaseService::new(
            business,
            auth.clone(),
            worker,
            identity,
            None,
            ids.clone(),
            rls,
            &secret,
            "http://127.0.0.1:3150",
        )
        .await
        .unwrap()
        .with_management(webdock_databases::ManagementConfig {
            cluster_id: Some(cluster.clone()),
            endpoint_domain: Some("fixture.test".into()),
            public_ordering: false,
            protected_database_id: None,
            lease_secret: Some(secret),
        })
        .unwrap(),
    );
    exec(&owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled','fixture'),('databases','builtin','enabled','enabled','fixture')",vec![]).await;
    exec(&owner,"WITH allocation AS (SELECT set_config('webdock.organization_allocation',$2,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Delivery fixture' FROM allocation",vec![customer.clone().into(),json!({"customer_id":customer,"organization_id":ids.allocate().await.unwrap().to_string()}).to_string().into()]).await;
    exec(
        &owner,
        "INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Delivery fixture')",
        vec![project.clone().into(), customer.clone().into()],
    )
    .await;
    exec(&owner,"INSERT INTO webdock_admin.projects(id,customer_id,name) VALUES($1,$2,'Unrelated hosting fixture')",vec![app_project.clone().into(),customer.clone().into()]).await;
    exec(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Delivery app operator',$2,'operator',$3)",vec![ids.allocate().await.unwrap().to_string().into(),format!("app-{}@example.invalid",app_actor.user_id).into(),app_actor.user_id.clone().into()]).await;
    let cap = json!({"apps":10,"cpuMillicores":16000,"memoryBytes":68719476736_u64,"volumeBytes":214748364800_u64,"ephemeralBytes":17179869184_u64,"replicasPerApp":1,"concurrentDeployments":10});
    exec(&owner,"INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Fixture','Fixture',$2,'{}',1,$3)",vec![customer.clone().into(),json!({"hosting":cap}).into(),app_actor.user_id.clone().into()]).await;
    exec(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,capacity,runtime_kind,runtime_installation) VALUES($1,'Delivery fixture','fixture','local','owned controlled runtime fixture',true,$2,'docker',$3)",vec![cluster.clone().into(),cap.clone().into(),installation.clone().into()]).await;
    exec(
        &owner,
        "UPDATE webdock_auth.hosting_cluster SET dedicated_customer_id=$2 WHERE id=$1",
        vec![cluster.clone().into(), customer.clone().into()],
    )
    .await;
    let pool = "00000000-0000-4000-8000-000000000002";
    let mut observation = json!({"capacity":cap,"nodes":[{"id":"fixture-node","cpuMillicores":16000,"memoryBytes":68719476736_u64}],"runtimeKind":"docker","capabilities":{"networkIsolation":true,"restrictedPods":true,"storage":false,"databaseVersion":1,"databaseNodePorts":[],"databasePools":[{"node":"fixture-node","poolID":pool,"ready":true,"capacityBytes":107374182400_u64,"availableBytes":107374182400_u64,"allocatedBytes":0,"requestedCpuMillicores":0,"requestedMemoryBytes":0,"allocations":[],"observedDatabases":[]}]}});
    observation["capacity"]["volumeBytes"] = json!(107374182400_u64);
    let hash = hex::encode(Sha256::digest("owned-delivery-fixture-credential"));
    let uid = rustix::process::geteuid().as_raw();
    exec(&owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_sequence,last_seen,observation) VALUES($1,$2,1,0,now(),$3)",vec![cluster.clone().into(),hash.clone().into(),observation.into()]).await;
    exec(&owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'delivery-agent',1,$2,$3,'unix',$4)",vec![installation.clone().into(),cluster.clone().into(),hash.into(),i64::from(uid).into()]).await;
    let authority = StoredAuthority::LocalSession {};
    databases.execute_managed(&actor,&authority,json!({"action":"databases.pools.register","clusterID":cluster,"node":"fixture-node","poolID":pool,"revision":1,"idempotencyKey":"delivery-pool-register"})).await.unwrap();
    let plan=databases.execute_managed(&actor,&authority,json!({"action":"databases.plans.create","plan":{"kind":"dedicated","postgresMajor":18,"cpuMillicores":500,"memoryBytes":536870912,"volumeBytes":536870912,"logicalStorageAllowanceBytes":null,"name":"Owned plan","monthlyPriceCents":1200,"currency":"EUR","connectionLimit":10,"terms":"Owned disposable integration fixture database service terms."},"idempotencyKey":"delivery-plan-create"})).await.unwrap();
    databases.execute_managed(&actor,&authority,json!({"action":"databases.plans.setAvailability","planRevisionID":plan["id"],"availability":"pilot","revision":1,"idempotencyKey":"delivery-plan-available"})).await.unwrap();
    let offer=databases.execute_managed(&actor,&authority,json!({"action":"databases.offers.create","projectID":project,"planRevisionID":plan["id"],"name":"Owned database","idempotencyKey":"delivery-offer-create"})).await.unwrap();
    let accepted=databases.execute_managed(&actor,&authority,json!({"action":"databases.acceptOrder","offerID":offer["id"],"projectID":project,"idempotencyKey":"delivery-order-accept"})).await.unwrap();
    hosting.execute(&app_actor,json!({"action":"projects.create","projectID":app_project,"provider":"docker","clusterID":cluster,"mode":"selfservice","ownImages":true,"idempotencyKey":"delivery-hosting-configure"})).await.unwrap();
    let core = ControlPlane::new(
        auth,
        hosting.clone(),
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "delivery-agent".into(),
            uid,
        }),
    )
    .await
    .unwrap()
    .with_databases(databases);
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = directory.path().join("control.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(core.clone().database_service())
            .add_service(core.service())
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(listener),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let config = AgentConfig {
        version: 1,
        installation: installation.clone(),
        agent_id: "delivery-agent".into(),
        generation: 1,
        state_directory: directory.path().into(),
        control: ControlEndpoint::Unix {
            path: socket.clone(),
        },
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
    let runtime = Arc::new(Interrupted {
        owner: owner.clone(),
        actor_session: actor.session_id,
        mode,
        claims: Mutex::new(vec![]),
        executions: AtomicUsize::new(0),
    });
    let apps = Arc::new(Apps(AtomicUsize::new(0)));
    let channel = tonic::transport::Endpoint::from_shared(format!("unix://{}", socket.display()))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = DbClient::new(channel);
    let manual = matches!(mode, Failure::ExpiredCheckpoint | Failure::ExpiredSuccess);
    let initial = if manual {
        let (mut spec, mut lease) = poll(&mut client, config.identity()).await;
        if matches!(mode, Failure::ExpiredSuccess) {
            assert!(
                deliver(
                    &mut client,
                    &checkpoint(
                        &spec,
                        &lease,
                        config.identity(),
                        DatabaseCheckpointPhase::Applied
                    )
                )
                .await
                .unwrap()
                .accepted
            );
            (spec, lease) = poll(&mut client, config.identity()).await;
            for phase in [
                DatabaseCheckpointPhase::Configuring,
                DatabaseCheckpointPhase::Observing,
            ] {
                assert!(
                    deliver(
                        &mut client,
                        &checkpoint(&spec, &lease, config.identity(), phase)
                    )
                    .await
                    .unwrap()
                    .accepted
                );
            }
            (spec, lease) = poll(&mut client, config.identity()).await;
        }
        let delivery = if matches!(mode, Failure::ExpiredCheckpoint) {
            checkpoint(
                &spec,
                &lease,
                config.identity(),
                DatabaseCheckpointPhase::Applied,
            )
        } else {
            wire::DatabaseDelivery {
                callback: Some(wire::database_delivery::Callback::Receipt(
                    wire::DatabaseReceipt {
                        identity: Some(config.identity()),
                        spec_json: serde_json::to_vec(&spec).unwrap(),
                        request_sha256: spec.fingerprint().unwrap(),
                        completion_json: serde_json::to_vec(&DatabaseCompletion {
                            lease: lease.clone(),
                            outcome: DatabaseCompletionOutcome::Succeeded,
                            proof: Some(DatabaseCompletionProof {
                                installation_id: spec.backend.id.clone(),
                                database_id: spec.database.as_ref().map(|d| d.id.clone()),
                                cnpg_uid: "owned-callback-instance".into(),
                                revision: spec.revision,
                                state: DatabaseProofState::Ready,
                                endpoint_ready: true,
                                copy_sha256: None,
                                backup: None,
                            }),
                        })
                        .unwrap(),
                    },
                )),
            }
        };
        exec(&owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![spec.operation_id.clone().into()]).await;
        runtime.claims.lock().unwrap().push((spec, lease));
        store(&config, &delivery);
        delivery
    } else {
        let mut daemon = Daemon::with_adapter(config.clone(), apps.clone())
            .unwrap()
            .with_database_runtime(runtime.clone());
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.is_err());
        drop(daemon);
        webdock_agent::outbox::DatabaseOutbox::open(
            config.state_directory.join("database-outbox.json"),
        )
        .unwrap()
        .pending()
        .unwrap()
        .unwrap()
    };
    let mut daemon = Daemon::with_adapter(config.clone(), apps.clone())
        .unwrap()
        .with_database_runtime(runtime.clone());
    daemon
        .connect()
        .await
        .expect("issued failure delivery must be classified without tenant execution authority");
    let id = accepted["operationID"].as_str().unwrap();
    let operation = row(
        &owner,
        "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1",
        id,
    )
    .await;
    assert_eq!(
        operation["status"],
        if matches!(mode, Failure::Replaced) {
            "queued"
        } else {
            "needs-reconciliation"
        }
    );
    let database = row(
        &owner,
        "SELECT to_jsonb(d) AS value FROM webdock_auth.hosting_database d WHERE id=$1",
        accepted["databaseID"].as_str().unwrap(),
    )
    .await;
    assert_eq!(
        database["state"],
        if matches!(mode, Failure::Replaced | Failure::ReplacedRevision) {
            "provisioning"
        } else {
            "needs-reconciliation"
        }
    );
    assert!(database["confirmed_credentials_encrypted"].is_null());
    let (spec, lease) = runtime.claims.lock().unwrap()[0].clone();
    assert!(deliver(&mut client, &initial).await.unwrap().accepted);
    assert_eq!(
        operation["desired"]["nativeDatabaseRetiredDeliveries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        operation,
        row(
            &owner,
            "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1",
            id
        )
        .await
    );
    assert!(
        client
            .guard_database(wire::DatabaseGuardRequest {
                identity: Some(config.identity()),
                spec_json: serde_json::to_vec(&spec).unwrap(),
                lease_json: serde_json::to_vec(&lease).unwrap(),
                request_sha256: spec.fingerprint().unwrap()
            })
            .await
            .is_err()
    );
    assert!(
        deliver(&mut client, &changed(initial.clone(), false))
            .await
            .is_err()
    );
    let forged = changed(initial, true);
    assert!(deliver(&mut client, &forged).await.is_err());
    if manual {
        let backend = row(
            &owner,
            "SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1",
            &spec.backend.id,
        )
        .await;
        assert_ne!(backend["state"], "ready");
        assert_eq!(backend["endpoint_ready"], false);
        assert_eq!(
            backend["cnpg_uid"],
            if matches!(mode, Failure::ExpiredCheckpoint) {
                serde_json::Value::Null
            } else {
                json!("owned-callback-instance")
            }
        );
        let order = row(
            &owner,
            "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_database_order o WHERE id=$1",
            accepted["orderID"].as_str().unwrap(),
        )
        .await;
        assert!(order["billing_started_at"].is_null());
        assert!(
            operation["lease_until"]
                .as_str()
                .map(|s| chrono::DateTime::parse_from_rfc3339(s).unwrap() < chrono::Utc::now())
                .unwrap()
        );
    }
    drop(daemon);
    store(&config, &forged);
    let mut daemon = Daemon::with_adapter(config.clone(), apps.clone())
        .unwrap()
        .with_database_runtime(runtime.clone());
    daemon.connect().await.expect(
        "an unclassifiable DB envelope must remain private without blocking unrelated domains",
    );
    exec(&owner,"UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,databaseVersion}','0'::jsonb) WHERE cluster_id=$1",vec![cluster.clone().into()]).await;
    let app=hosting.execute(&app_actor,json!({"action":"apps.create","projectID":app_project,"name":"unrelated-app","spec":{"template":"custom","image":format!("docker.io/library/busybox@sha256:{}","a".repeat(64)),"args":[],"port":8080,"healthPath":"/","cpuMillicores":100,"memoryBytes":16777216,"ephemeralBytes":1048576,"replicas":1,"volumeBytes":0},"subscriptionRevision":1,"idempotencyKey":"delivery-unrelated-app"})).await.unwrap();
    assert!(daemon.cycle().await.unwrap());
    assert_eq!(apps.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        hosting
            .execute(
                &app_actor,
                json!({"action":"apps.get","appID":app["appID"]})
            )
            .await
            .unwrap()["status"],
        "ready"
    );
    for _ in 0..2 {
        assert!(!daemon.cycle().await.unwrap());
    }
    let enrollment=row(&owner,"SELECT to_jsonb(e) AS value FROM webdock_auth.agent_transport_enrollment e WHERE cluster_id=$1",&cluster).await;
    assert!(enrollment["last_sequence"].as_i64().unwrap() >= 2);
    assert_eq!(runtime.claims.lock().unwrap().len(), 1);
    assert_eq!(
        runtime.executions.load(Ordering::SeqCst),
        usize::from(!manual)
    );
    let retained: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("database-outbox.json")).unwrap(),
    )
    .unwrap();
    assert!(
        !retained["pending"].is_null(),
        "unknown envelope must not be discarded"
    );
    drop(daemon);
    let _ = stop.send(());
    server.await.unwrap();
}
#[tokio::test]
#[ignore = "creates only fresh uniquely named databases in the verified local restricted-role fixture"]
async fn stale_database_delivery_cannot_poison_unrelated_hosting_and_heartbeat() {
    let source =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(source.host_str(), Some("127.0.0.1"));
    assert_eq!(source.path(), "/webdock_rls_test");
    let control = Database::connect(source.as_str()).await.unwrap();
    for mode in [
        Failure::RevokedSession,
        Failure::Expired,
        Failure::MissingCapability,
        Failure::Replaced,
        Failure::ReplacedRevision,
        Failure::ExpiredCheckpoint,
        Failure::ExpiredSuccess,
    ] {
        let name = format!("webdock_native_db_{}_test", uuid::Uuid::new_v4().simple());
        control
            .execute_unprepared(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let test_name = name.clone();
        let result = tokio::spawn(async move {
            let owner = connection(&test_name, "WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
            webdock_persistence::migrations::bootstrap(
                &owner,
                &test_name,
                &std::env::var("DATABASE_RLS_SECRET").unwrap(),
            )
            .await
            .unwrap();
            case(test_name, owner, mode).await;
        })
        .await;
        control
            .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .await
            .unwrap();
        result.unwrap();
    }
}
