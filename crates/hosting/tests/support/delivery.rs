use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tonic::{
    Request, Response, Status,
    transport::{Endpoint, Server},
};
use webdock_agent::{
    config::{AgentConfig, ControlEndpoint, RuntimeConfiguration},
    management::Daemon,
    outbox::ReceiptOutbox,
};
use webdock_agent_control::{ControlPlane, LocalBinding};
use webdock_agent_protocol::{
    self as wire,
    agent_control_client::AgentControlClient,
    agent_control_server::{AgentControl, AgentControlServer},
};
use webdock_supervisor::{
    Capabilities, Command, LeaseGuard, ObservedState, Phase, RuntimeAdapter, RuntimeSpec,
};

struct AbortServer(tokio::task::AbortHandle);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct Runtime {
    fail: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl RuntimeAdapter for Runtime {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            health: true,
            logs: true,
            ..Default::default()
        }
    }
    async fn reconcile(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> webdock_supervisor::Result<ObservedState> {
        guard.check_for(command).await?;
        if self.fail.load(Ordering::SeqCst) {
            return Err(webdock_supervisor::ExecutionError::Runtime);
        }
        self.inspect(&command.spec).await
    }
    async fn inspect(&self, spec: &RuntimeSpec) -> webdock_supervisor::Result<ObservedState> {
        Ok(ObservedState {
            resource_id: spec.resource_id.to_string(),
            revision: spec.revision,
            phase: Phase::Running,
            ready: true,
            observed_at: webdock_supervisor::timestamp()?,
            runtime_id: Some("custody-runtime".into()),
            conditions: vec![],
            storage: None,
        })
    }
    async fn logs(&self, _spec: &RuntimeSpec, _tail: u32) -> webdock_supervisor::Result<String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(webdock_supervisor::ExecutionError::Runtime);
        }
        Ok("owned-runtime-log".into())
    }
}
struct Switch {
    core: ControlPlane,
    outage_core: ControlPlane,
    outage: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    hold: Arc<AtomicBool>,
    heartbeats: Arc<AtomicUsize>,
    receipts: Arc<std::sync::Mutex<Vec<wire::Receipt>>>,
}
#[tonic::async_trait]
impl AgentControl for Switch {
    async fn register(&self, r: Request<wire::Hello>) -> Result<Response<wire::Welcome>, Status> {
        self.core.register(r).await
    }
    async fn heartbeat(
        &self,
        r: Request<wire::HeartbeatRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        self.heartbeats.fetch_add(1, Ordering::SeqCst);
        self.core.heartbeat(r).await
    }
    async fn poll(
        &self,
        r: Request<wire::PollRequest>,
    ) -> Result<Response<wire::Dispatch>, Status> {
        self.core
            .clone()
            .with_hosting_enabled(self.enabled.load(Ordering::SeqCst))
            .poll(r)
            .await
    }
    async fn guard(
        &self,
        r: Request<wire::GuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        self.core
            .clone()
            .with_hosting_enabled(self.enabled.load(Ordering::SeqCst))
            .guard(r)
            .await
    }
    async fn complete(&self, r: Request<wire::Receipt>) -> Result<Response<wire::Ack>, Status> {
        self.receipts.lock().unwrap().push(r.get_ref().clone());
        if self.hold.load(Ordering::SeqCst) {
            return Err(Status::unavailable("owned fixture holds receipt delivery"));
        }
        let core = if self.outage.load(Ordering::SeqCst) {
            &self.outage_core
        } else {
            &self.core
        };
        core.clone()
            .with_hosting_enabled(self.enabled.load(Ordering::SeqCst))
            .complete(r)
            .await
    }
}
#[derive(Clone, Copy, Debug)]
enum Case {
    Expired,
    RevokedSession,
    Reconciled,
}
#[tokio::test]
#[ignore = "owned local restricted-role Hosting fixture and native Unix control"]
async fn stale_receipt_custody_releases_hosting_outbox_without_business_completion() {
    for case in [Case::Expired, Case::RevokedSession, Case::Reconciled] {
        let fixture = Arc::new(Fixture::new("docker").await);
        let f = fixture.clone();
        let result = tokio::spawn(async move { scenario(f, case).await }).await;
        sql(&fixture.owner,"DELETE FROM webdock_auth.agent_runtime_dispatch WHERE installation=(SELECT runtime_installation FROM webdock_auth.hosting_cluster WHERE id=$1)",vec![fixture.cluster.clone().into()]).await;
        sql(
            &fixture.owner,
            "DELETE FROM webdock_auth.agent_transport_enrollment WHERE cluster_id=$1",
            vec![fixture.cluster.clone().into()],
        )
        .await;
        sql(
            &fixture.owner,
            "DELETE FROM webdock_auth.session WHERE id=$1",
            vec![format!("{}-renewed-session", fixture.nonce).into()],
        )
        .await;
        fixture.cleanup().await;
        result.unwrap();
    }
}
async fn scenario(f: Arc<Fixture>, case: Case) {
    f.configure("docker").await;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let state = directory.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.path().join("control.sock");
    let installation: String = f
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT runtime_installation FROM webdock_auth.hosting_cluster WHERE id=$1",
            [f.cluster.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "runtime_installation")
        .unwrap();
    let uid = rustix::process::geteuid().as_raw();
    sql(&f.owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'custody-agent',1,$2,$3,'unix',$4)",vec![installation.clone().into(),f.cluster.clone().into(),hex::encode(Sha256::digest("a".repeat(43))).into(),i64::from(uid).into()]).await;
    let control = ControlPlane::new(
        db("AUTH_DATABASE_URL").await,
        f.service.clone(),
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "custody-agent".into(),
            uid,
        }),
    )
    .await
    .unwrap();
    let unavailable_pool = db("DATABASE_URL").await;
    let unavailable_hosting = Arc::new(
        HostingService::new(
            unavailable_pool.clone(),
            db("AUTH_DATABASE_URL").await,
            db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            std::env::var("BETTER_AUTH_SECRET").unwrap(),
            Arc::new(webdock_persistence::IdAllocator::new(
                db("AUTH_DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
        )
        .await
        .unwrap(),
    );
    unavailable_pool.close().await.unwrap();
    let outage_core = ControlPlane::new(
        db("AUTH_DATABASE_URL").await,
        unavailable_hosting,
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "custody-agent".into(),
            uid,
        }),
    )
    .await
    .unwrap();
    let outage = Arc::new(AtomicBool::new(false));
    let enabled = Arc::new(AtomicBool::new(true));
    let hold = Arc::new(AtomicBool::new(true));
    let heartbeats = Arc::new(AtomicUsize::new(0));
    let receipts = Arc::new(std::sync::Mutex::new(vec![]));
    let proxy = Switch {
        core: control,
        outage_core,
        outage: outage.clone(),
        enabled: enabled.clone(),
        hold: hold.clone(),
        heartbeats: heartbeats.clone(),
        receipts: receipts.clone(),
    };
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(AgentControlServer::new(proxy))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::UnixListenerStream::new(listener),
                async {
                    let _ = shutdown.await;
                },
            )
            .await
            .unwrap();
    });
    let _abort_server = AbortServer(server.abort_handle());
    let inner = async {
        let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"custody-app","spec":spec(),"subscriptionRevision":1,"idempotencyKey":"custody-app-create-request"})).await.unwrap();
        let operation = created["id"].as_str().unwrap();
        let app = created["appID"].as_str().unwrap();
        let config = AgentConfig {
            database_runtime: None,
            mail_configuration_enabled: false,
            mail_runtime: None,
            recovery_runtime: None,
            version: 1,
            installation: installation.clone(),
            agent_id: "custody-agent".into(),
            generation: 1,
            state_directory: state.clone(),
            control: ControlEndpoint::Unix {
                path: socket.clone(),
            },
            runtime: RuntimeConfiguration::Docker {
                storage_socket: None,
                socket: "/var/run/docker.sock".into(),
                initializer_image: None,
            },
            policy: Default::default(),
            workloads: false,
            plugin_runtime_enabled: false,
        };
        let fail_runtime = Arc::new(AtomicBool::new(false));
        let mut daemon = Daemon::with_adapter(
            config,
            Arc::new(Runtime {
                fail: fail_runtime.clone(),
            }),
        )
        .unwrap();
        daemon.connect().await.unwrap();
        assert!(daemon.cycle().await.is_err());
        let receipt = ReceiptOutbox::open(state.join("receipt-outbox.json"))
            .unwrap()
            .pending()
            .unwrap()
            .expect("actual executor result held in durable outbox");
        assert_eq!(receipt.outcome, wire::Outcome::Succeeded as i32);
        let mut client = AgentControlClient::new(
            Endpoint::from_shared(format!("unix://{}", socket.display()))
                .unwrap()
                .connect()
                .await
                .unwrap(),
        );
        hold.store(false, Ordering::SeqCst);
        outage.store(true, Ordering::SeqCst);
        let before_outage = operation_state(&f, operation).await;
        assert_eq!(
            client.complete(receipt.clone()).await.unwrap_err().code(),
            tonic::Code::Unavailable
        );
        daemon.connect().await.unwrap();
        let heartbeat_before_outage = heartbeats.load(Ordering::SeqCst);
        daemon.cycle().await.unwrap();
        assert!(heartbeats.load(Ordering::SeqCst) > heartbeat_before_outage);
        assert_eq!(
            operation_state(&f, operation).await,
            before_outage,
            "authority database outage must not classify delivery or settle state"
        );
        assert!(
            ReceiptOutbox::open(state.join("receipt-outbox.json"))
                .unwrap()
                .pending()
                .unwrap()
                .is_some()
        );
        outage.store(false, Ordering::SeqCst);
        let mut wrong_target = receipt.clone();
        if let Some(wire::receipt::Result::ObservedJson(bytes)) = &mut wrong_target.result {
            let mut value: Value = serde_json::from_slice(bytes).unwrap();
            value["resourceId"] = json!("999");
            *bytes = serde_json::to_vec(&value).unwrap();
        }
        assert_eq!(
            client.complete(wrong_target).await.unwrap_err().code(),
            tonic::Code::InvalidArgument
        );
        let mut wrong_fingerprint = receipt.clone();
        wrong_fingerprint.request_sha256 = "f".repeat(64);
        assert_eq!(
            client.complete(wrong_fingerprint).await.unwrap_err().code(),
            tonic::Code::PermissionDenied
        );
        let mut wrong_agent_generation = receipt.clone();
        wrong_agent_generation.identity.as_mut().unwrap().generation = 2;
        assert_eq!(
            client
                .complete(wrong_agent_generation)
                .await
                .unwrap_err()
                .code(),
            tonic::Code::PermissionDenied
        );
        match case {
            Case::RevokedSession=>sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id=$1",vec![f.sid.clone().into()]).await,
            Case::Expired|Case::Reconciled=>sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![operation.into()]).await,
        }
        let fresh = fresh_operator(&f).await;
        if matches!(case, Case::Reconciled) {
            f.service.execute(&fresh,json!({"action":"apps.reconcile","appID":app,"idempotencyKey":"custody-early-reconcile"})).await.unwrap();
        }
        hold.store(false, Ordering::SeqCst);
        enabled.store(false, Ordering::SeqCst);
        daemon.connect().await.unwrap();
        let before = heartbeats.load(Ordering::SeqCst);
        daemon.cycle().await.unwrap();
        assert!(heartbeats.load(Ordering::SeqCst) > before);
        assert!(
            ReceiptOutbox::open(state.join("receipt-outbox.json"))
                .unwrap()
                .pending()
                .unwrap()
                .is_some(),
            "disabled Core retains unseen outcomes"
        );
        enabled.store(true, Ordering::SeqCst);
        sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET desired=jsonb_set(desired,'{nativeHostingRetiredDeliveries}','\"corrupt\"'::jsonb) WHERE id=$1",vec![operation.into()]).await;
        assert_eq!(
            client.complete(receipt.clone()).await.unwrap_err().code(),
            tonic::Code::Unavailable,
            "invalid stored custody cannot be silently discarded"
        );
        sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET desired=desired-'nativeHostingRetiredDeliveries' WHERE id=$1",vec![operation.into()]).await;
        daemon.connect().await.unwrap();
        assert!(
            ReceiptOutbox::open(state.join("receipt-outbox.json"))
                .unwrap()
                .pending()
                .unwrap()
                .is_none(),
            "{case:?}: exact stale receipt must be durably acknowledged"
        );
        let status = operation_state(&f, operation).await;
        assert_ne!(status["status"], "succeeded");
        assert_eq!(status["app_observed_revision"], 0);
        assert_ne!(status["app_status"], "ready");
        assert_eq!(status["reservation_status"], "reserved");
        sql(&f.owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL,completed_at=NULL WHERE operation_id=$1 AND generation=$2",vec![operation.into(),(receipt.generation as i64).into()]).await;
        let mut altered = receipt.clone();
        if let Some(wire::receipt::Result::ObservedJson(bytes)) = &mut altered.result {
            let mut value: Value = serde_json::from_slice(bytes).unwrap();
            value["observedAt"] = json!(value["observedAt"].as_u64().unwrap() + 1);
            *bytes = serde_json::to_vec(&value).unwrap();
        }
        assert_eq!(
            client.complete(altered).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition,
            "a lost Core custody-journal write cannot admit a changed delivery"
        );
        assert!(
            client
                .complete(receipt.clone())
                .await
                .unwrap()
                .into_inner()
                .accepted
        );
        let mut changed = receipt.clone();
        changed.error_code = "changed".into();
        assert!(client.complete(changed).await.is_err());
        let mut forged = receipt.clone();
        forged.operation_id = "999999999999999999".into();
        assert!(client.complete(forged).await.is_err());
        enabled.store(false, Ordering::SeqCst);
        assert!(
            client
                .complete(receipt.clone())
                .await
                .unwrap()
                .into_inner()
                .accepted
        );
        enabled.store(true, Ordering::SeqCst);
        if !matches!(case, Case::Reconciled) {
            f.service.execute(&fresh,json!({"action":"apps.reconcile","appID":app,"idempotencyKey":"custody-reconcile-after-ack"})).await.unwrap();
        }
        if matches!(case, Case::Expired) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        assert!(daemon.cycle().await.unwrap());
        let view = f
            .service
            .execute(&fresh, json!({"action":"apps.get","appID":app}))
            .await
            .unwrap();
        assert_eq!(
            view["status"], "ready",
            "fresh authorized generation must progress"
        );
        let live_mutation = last_receipt(&receipts, operation);
        let after = operation_state(&f, operation).await;
        assert!(after["generation"].as_u64().unwrap() > receipt.generation);
        sql(&f.owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL,completed_at=NULL WHERE operation_id=$1 AND generation=$2",vec![operation.into(),(receipt.generation as i64).into()]).await;
        assert!(
            client
                .complete(receipt.clone())
                .await
                .unwrap()
                .into_inner()
                .accepted
        );
        assert_eq!(
            operation_state(&f, operation).await,
            after,
            "old replay cannot overwrite new business state"
        );
        if matches!(case, Case::Expired) {
            let requested=f.service.execute(&fresh,json!({"action":"apps.logs","appID":app,"idempotencyKey":"custody-read-expired-request"})).await.unwrap();
            hold.store(true, Ordering::SeqCst);
            assert!(daemon.cycle().await.is_err());
            let read_receipt = ReceiptOutbox::open(state.join("receipt-outbox.json"))
                .unwrap()
                .pending()
                .unwrap()
                .unwrap();
            let app_before = f
                .service
                .execute(&fresh, json!({"action":"apps.get","appID":app}))
                .await
                .unwrap();
            sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![requested["id"].as_str().unwrap().into()]).await;
            hold.store(false, Ordering::SeqCst);
            daemon.connect().await.unwrap();
            assert!(
                ReceiptOutbox::open(state.join("receipt-outbox.json"))
                    .unwrap()
                    .pending()
                    .unwrap()
                    .is_none()
            );
            let read = serde_json::to_value(
                f.service
                    .get_read(&fresh, requested["id"].as_str().unwrap())
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(read["status"], "failed");
            assert_eq!(
                f.service
                    .execute(&fresh, json!({"action":"apps.get","appID":app}))
                    .await
                    .unwrap(),
                app_before,
                "stale read custody must not change application state"
            );
            assert!(
                client
                    .complete(read_receipt)
                    .await
                    .unwrap()
                    .into_inner()
                    .accepted
            );
            let next=f.service.execute(&fresh,json!({"action":"apps.logs","appID":app,"idempotencyKey":"custody-read-after-expiry"})).await.unwrap();
            assert!(daemon.cycle().await.unwrap());
            let next = serde_json::to_value(
                f.service
                    .get_read(&fresh, next["id"].as_str().unwrap())
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(next["status"], "succeeded");
            let live = last_receipt(&receipts, next["id"].as_str().unwrap());
            assert_live_receipt_exactness(&f, &mut client, live).await;
            assert_live_receipt_exactness(&f, &mut client, live_mutation).await;
            fail_runtime.store(true, Ordering::SeqCst);
            let failed_read=f.service.execute(&fresh,json!({"action":"apps.logs","appID":app,"idempotencyKey":"live-read-failure-replay"})).await.unwrap();
            daemon.cycle().await.unwrap();
            let failed = last_receipt(&receipts, failed_read["id"].as_str().unwrap());
            assert_ne!(failed.outcome, wire::Outcome::Succeeded as i32);
            assert_live_receipt_exactness(&f, &mut client, failed).await;
            let current = f
                .service
                .execute(&fresh, json!({"action":"apps.get","appID":app}))
                .await
                .unwrap();
            let failed_mutation=f.service.execute(&fresh,json!({"action":"apps.restart","appID":app,"revision":current["revision"],"subscriptionRevision":1,"idempotencyKey":"live-mutation-failure-replay"})).await.unwrap();
            daemon.cycle().await.unwrap();
            let failed = last_receipt(&receipts, failed_mutation["id"].as_str().unwrap());
            assert_ne!(failed.outcome, wire::Outcome::Succeeded as i32);
            assert_live_receipt_exactness(&f, &mut client, failed).await;
            fail_runtime.store(false, Ordering::SeqCst);
            f.service.execute(&fresh,json!({"action":"apps.reconcile","appID":app,"idempotencyKey":"requeue-after-live-failure"})).await.unwrap();
            hold.store(true, Ordering::SeqCst);
            assert!(daemon.cycle().await.is_err());
            let renewed = ReceiptOutbox::open(state.join("receipt-outbox.json"))
                .unwrap()
                .pending()
                .unwrap()
                .unwrap();
            assert!(renewed.generation > 1);
            sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",vec![renewed.operation_id.clone().into()]).await;
            hold.store(false, Ordering::SeqCst);
            daemon.connect().await.unwrap();
            assert!(
                ReceiptOutbox::open(state.join("receipt-outbox.json"))
                    .unwrap()
                    .pending()
                    .unwrap()
                    .is_none(),
                "older generation domain receipt must not block newer stale delivery"
            );
            assert_eq!(
                operation_state(&f, &renewed.operation_id).await["status"],
                "needs-reconciliation"
            );
            f.service.execute(&fresh,json!({"action":"apps.reconcile","appID":app,"idempotencyKey":"requeue-after-renewed-expiry"})).await.unwrap();
            assert!(daemon.cycle().await.unwrap());
            assert_eq!(
                f.service
                    .execute(&fresh, json!({"action":"apps.get","appID":app}))
                    .await
                    .unwrap()["status"],
                "ready"
            );
        }
        sql(&f.owner,"UPDATE webdock_auth.agent_transport_enrollment SET revoked=true WHERE installation=$1 AND agent_id='custody-agent'",vec![installation.clone().into()]).await;
        assert_eq!(
            client.complete(receipt).await.unwrap_err().code(),
            tonic::Code::PermissionDenied
        );
        assert!(matches!(
            daemon.connect().await,
            Err(webdock_agent::management::AgentError::Revoked)
        ));
    };
    inner.await;
    let _ = stop.send(());
    server.await.unwrap();
}
async fn fresh_operator(f: &Fixture) -> Principal {
    let session = format!("{}-renewed-session", f.nonce);
    let token = format!("{}-renewed-token", f.nonce);
    sql(&f.owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())",vec![session.into(),token.clone().into(),f.uid.clone().into()]).await;
    let policy = CookiePolicy::new(false, "fixture-hosting-cookie-key-at-least-32-bytes").unwrap();
    let cookie = format!("{}={}", policy.name(), policy.sign(&token).unwrap());
    IdentityService::new(db("AUTH_DATABASE_URL").await, policy)
        .authenticate_cookie(&cookie)
        .await
        .unwrap()
        .unwrap()
}
async fn operation_state(f: &Fixture, operation: &str) -> Value {
    f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT jsonb_build_object('status',o.status,'generation',o.generation,'app_status',a.status,'app_observed_revision',a.observed_revision,'reservation_status',r.status) AS value FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_app a ON a.id=o.app_id LEFT JOIN webdock_auth.hosting_reservation r ON r.operation_id=o.id WHERE o.id=$1",[operation.into()])).await.unwrap().unwrap().try_get("","value").unwrap()
}

fn last_receipt(receipts: &std::sync::Mutex<Vec<wire::Receipt>>, operation: &str) -> wire::Receipt {
    receipts
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|r| r.operation_id == operation)
        .unwrap()
        .clone()
}
async fn assert_live_receipt_exactness(
    f: &Fixture,
    client: &mut AgentControlClient<tonic::transport::Channel>,
    receipt: wire::Receipt,
) {
    for wire_bound in [true, false] {
        if !wire_bound {
            // Owned fixture models a native domain commit from before transport hashes were
            // persisted atomically. Its original semantic domain receipt remains authoritative.
            sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET desired=jsonb_set(desired,'{nativeHostingAcceptedDeliveries}',COALESCE((SELECT jsonb_agg(r) FROM jsonb_array_elements(desired->'nativeHostingAcceptedDeliveries') r WHERE (r->>'operationGeneration')::bigint<>$2),'[]'::jsonb)) WHERE id=$1",vec![receipt.operation_id.clone().into(),(receipt.generation as i64).into()]).await;
        }
        let before = domain_record(f, &receipt.operation_id).await;
        sql(&f.owner,"UPDATE webdock_auth.agent_runtime_dispatch SET receipt_sha256=NULL,completed_at=NULL WHERE operation_id=$1 AND generation=$2",vec![receipt.operation_id.clone().into(),(receipt.generation as i64).into()]).await;
        let mut changed = receipt.clone();
        match &mut changed.result {
            Some(wire::receipt::Result::ObservedJson(bytes)) => {
                let mut value: Value = serde_json::from_slice(bytes).unwrap();
                value["observedAt"] = json!(value["observedAt"].as_u64().unwrap() + 1);
                *bytes = serde_json::to_vec(&value).unwrap();
            }
            Some(wire::receipt::Result::LogsUtf8(bytes)) => {
                bytes.extend_from_slice(b"changed-log-content")
            }
            None => changed.error_code = "different_runtime_error".into(),
            Some(wire::receipt::Result::MetricsJson(_)) => {
                panic!("not used by this lifecycle fixture")
            }
        }
        assert_eq!(
            client.complete(changed).await.unwrap_err().code(),
            tonic::Code::FailedPrecondition,
            "terminal domain receipt must prevent changed replay after transport-journal loss"
        );
        if wire_bound {
            let mut changed = receipt.clone();
            if let Some(wire::receipt::Result::ObservedJson(bytes)) = &mut changed.result {
                bytes.push(b' ');
                assert_eq!(
                    client.complete(changed).await.unwrap_err().code(),
                    tonic::Code::FailedPrecondition,
                    "wire-bound receipt also rejects changed JSON encoding of identical observation"
                );
            }
        }
        assert_eq!(
            domain_record(f, &receipt.operation_id).await,
            before,
            "rejected replay must not poison custody or domain state"
        );
        assert!(
            client
                .complete(receipt.clone())
                .await
                .unwrap()
                .into_inner()
                .accepted
        );
    }
}
async fn domain_record(f: &Fixture, operation: &str) -> Value {
    f.owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1",
            [operation.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}
