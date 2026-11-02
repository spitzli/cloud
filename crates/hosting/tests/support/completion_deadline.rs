//! A live receipt must not settle state after waiting past its persisted deadline.
use super::*;
use sea_orm::TransactionTrait;
use webdock_supervisor::{ObservedState, Phase};

struct AbortCompletion(tokio::task::AbortHandle);
impl Drop for AbortCompletion {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
#[ignore = "owned local restricted-role Hosting fixture"]
async fn mutation_completion_rechecks_deadline_after_application_lock_wait() {
    let fixture = Arc::new(Fixture::new("docker").await);
    let f = fixture.clone();
    let result = tokio::spawn(async move {
        f.configure("docker").await;
        let created = f
            .service
            .execute(
                &f.principal,
                json!({
                    "action":"apps.create", "projectID":f.pid, "name":"deadline-app",
                    "spec":spec(), "subscriptionRevision":1,
                    "idempotencyKey":"completion-deadline-create"
                }),
            )
            .await
            .unwrap();
        let initial = f.service.claim_runtime(&f.agent).await.unwrap().unwrap();
        sql(
            &f.owner,
            "UPDATE webdock_auth.hosting_operation SET lease_until=date_trunc('second',clock_timestamp())+interval '3 seconds' WHERE id=$1",
            vec![initial.operation_id.clone().into()],
        )
        .await;
        let command = f
            .service
            .runtime_command(&f.agent, &initial.operation_id, initial.generation)
            .await
            .unwrap();
        let blocker = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
        let tx = blocker.begin().await.unwrap();
        let pid: i32 = tx
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT pg_backend_pid() AS pid",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "pid")
            .unwrap();
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE",
            [created["appID"].as_str().unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap();
        let host = f.service.clone();
        let agent = f.agent.clone();
        let deadline = command.lease_until;
        let completion = tokio::spawn(async move {
            let observed = ObservedState {
                resource_id: command.spec.resource_id.to_string(),
                revision: command.spec.revision,
                phase: Phase::Running,
                ready: true,
                observed_at: webdock_supervisor::timestamp().unwrap(),
                runtime_id: Some("deadline-owned-runtime".into()),
                conditions: vec![],
                storage: None,
            };
            host.complete_runtime_with_receipt(&agent, &command, &observed, &"a".repeat(64))
                .await
        });
        let _abort_completion = AbortCompletion(completion.abort_handle());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let waiting: bool = f
                    .owner
                    .query_one_raw(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS waiting",
                        [pid.into()],
                    ))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get("", "waiting")
                    .unwrap();
                if waiting {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("completion must reach the held application lock while its lease is live");
        assert!(webdock_supervisor::timestamp().unwrap() < deadline);
        while webdock_supervisor::timestamp().unwrap() <= deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        tx.commit().await.unwrap();
        let completion = completion.await.unwrap();
        let state: Value = f
            .owner
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT jsonb_build_object('operation',o.status,'application',a.status,'observedRevision',a.observed_revision,'reservation',r.status) AS state FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_app a ON a.id=o.app_id JOIN webdock_auth.hosting_reservation r ON r.operation_id=o.id WHERE o.id=$1",
                [initial.operation_id.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "state")
            .unwrap();
        assert!(
            completion.is_err(),
            "expired completion must be rejected before any outcome is committed: {state}"
        );
        assert_eq!(state["observedRevision"], 0);
        assert_eq!(state["reservation"], "reserved");
        assert_ne!(state["application"], "ready");
    })
    .await;
    fixture.cleanup().await;
    result.unwrap();
}

#[derive(Clone, Copy, Debug)]
enum CompletionCase {
    MutationSuccess,
    MutationFailure,
    LegacySuccess,
    LegacyFailure,
    LogsSuccess,
    LogsFailure,
    InspectSuccess,
    InspectFailure,
    MetricsSuccess,
    MetricsFailure,
    OriginalSessionExpiry,
}
impl CompletionCase {
    fn read_action(self) -> Option<&'static str> {
        match self {
            Self::LogsSuccess | Self::LogsFailure => Some("apps.logs"),
            Self::InspectSuccess | Self::InspectFailure => Some("apps.inspect"),
            Self::MetricsSuccess | Self::MetricsFailure => Some("apps.metrics"),
            Self::MutationSuccess
            | Self::MutationFailure
            | Self::LegacySuccess
            | Self::LegacyFailure
            | Self::OriginalSessionExpiry => None,
        }
    }
    fn failure(self) -> bool {
        matches!(
            self,
            Self::MutationFailure
                | Self::LegacyFailure
                | Self::LogsFailure
                | Self::InspectFailure
                | Self::MetricsFailure
        )
    }
    fn audit_barrier(self) -> bool {
        matches!(
            self,
            Self::InspectSuccess
                | Self::InspectFailure
                | Self::MetricsSuccess
                | Self::MetricsFailure
        )
    }
}
#[tokio::test]
#[ignore = "owned local restricted-role Hosting fixture"]
async fn completion_deadlines_rollback_all_live_branches_after_blocking_waits() {
    for case in [
        CompletionCase::MutationFailure,
        CompletionCase::LegacySuccess,
        CompletionCase::LegacyFailure,
        CompletionCase::LogsSuccess,
        CompletionCase::LogsFailure,
        CompletionCase::InspectSuccess,
        CompletionCase::InspectFailure,
        CompletionCase::MetricsSuccess,
        CompletionCase::MetricsFailure,
        CompletionCase::OriginalSessionExpiry,
    ] {
        let fixture = Arc::new(Fixture::new("docker").await);
        let f = fixture.clone();
        let result = tokio::spawn(async move { deadline_scenario(f, case).await }).await;
        fixture
            .owner
            .execute_unprepared(&format!(
                "DROP SCHEMA IF EXISTS {} CASCADE",
                deadline_schema(&fixture)
            ))
            .await
            .unwrap();
        sql(
            &fixture.owner,
            "DELETE FROM webdock_auth.session WHERE id=$1",
            vec![format!("{}-deadline-refresh", fixture.nonce).into()],
        )
        .await;
        fixture.cleanup().await;
        result.unwrap();
    }
}
fn deadline_schema(f: &Fixture) -> String {
    assert!(f.cluster.bytes().all(|b| b.is_ascii_digit()));
    format!("wd_completion_deadline_{}", f.cluster)
}
fn ready_observation(command: &webdock_supervisor::Command) -> ObservedState {
    ObservedState {
        resource_id: command.spec.resource_id.to_string(),
        revision: command.spec.revision,
        phase: Phase::Running,
        ready: true,
        observed_at: webdock_supervisor::timestamp().unwrap(),
        runtime_id: Some("deadline-owned-runtime".into()),
        conditions: vec![],
        storage: None,
    }
}
async fn deadline_scenario(f: Arc<Fixture>, case: CompletionCase) {
    f.configure("docker").await;
    let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"deadline-app","spec":spec(),"subscriptionRevision":1,"idempotencyKey":"completion-deadline-matrix"})).await.unwrap();
    let app = created["appID"].as_str().unwrap().to_owned();
    let initial = f.service.claim_runtime(&f.agent).await.unwrap().unwrap();
    let schema = deadline_schema(&f);
    f.owner
        .execute_unprepared(&format!("CREATE SCHEMA {schema}"))
        .await
        .unwrap();
    let (command, read) = if let Some(action) = case.read_action() {
        f.service
            .complete_runtime(&f.agent, &initial, &ready_observation(&initial))
            .await
            .unwrap();
        let requested = f
            .service
            .execute(
                &f.principal,
                json!({"action":action,"appID":app,"idempotencyKey":"completion-deadline-read"}),
            )
            .await
            .unwrap();
        let id = requested["id"].as_str().unwrap();
        assert!(id.bytes().all(|b| b.is_ascii_digit()));
        // Shorten this owned issued lease in PostgreSQL before the production signer derives it.
        f.owner.execute_unprepared(&format!("CREATE FUNCTION {schema}.short_lease() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN NEW.lease_until=date_trunc('second',clock_timestamp())+interval '3 seconds'; RETURN NEW; END $$; CREATE TRIGGER short_lease_{} BEFORE UPDATE OF status ON webdock_auth.hosting_operation FOR EACH ROW WHEN (NEW.id='{id}' AND OLD.status='queued' AND NEW.status='running') EXECUTE FUNCTION {schema}.short_lease()",f.cluster)).await.unwrap();
        let command = f.service.claim_read(&f.agent).await.unwrap().unwrap();
        f.owner
            .execute_unprepared(&format!(
                "DROP TRIGGER short_lease_{} ON webdock_auth.hosting_operation",
                f.cluster
            ))
            .await
            .unwrap();
        (None, Some(command))
    } else {
        if matches!(case, CompletionCase::OriginalSessionExpiry) {
            sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=date_trunc('second',clock_timestamp())+interval '3 seconds' WHERE id=$1",vec![f.sid.clone().into()]).await;
            sql(&f.owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$1,$2,now()+interval '1 hour',now())",vec![format!("{}-deadline-refresh",f.nonce).into(),f.uid.clone().into()]).await;
        } else {
            sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=date_trunc('second',clock_timestamp())+interval '3 seconds' WHERE id=$1",vec![initial.operation_id.clone().into()]).await;
        }
        (
            Some(
                f.service
                    .runtime_command(&f.agent, &initial.operation_id, initial.generation)
                    .await
                    .unwrap(),
            ),
            None,
        )
    };
    let (operation, generation, mut deadline) = if let Some(command) = &command {
        (
            command.operation_id.clone(),
            command.generation,
            command.lease_until,
        )
    } else {
        let read = read.as_ref().unwrap();
        (read.operation_id.clone(), read.generation, read.lease_until)
    };
    if matches!(case, CompletionCase::OriginalSessionExpiry) {
        deadline=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT extract(epoch FROM \"expiresAt\")::bigint AS until FROM webdock_auth.session WHERE id=$1",[f.sid.clone().into()])).await.unwrap().unwrap().try_get::<i64>("","until").unwrap().try_into().unwrap();
    }
    let before = deadline_state(&f, &operation).await;
    let blocker = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let tx = blocker.begin().await.unwrap();
    let pid: i32 = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    if case.audit_barrier() {
        let key = f.cluster.parse::<i64>().unwrap();
        tx.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [key.into()],
        ))
        .await
        .unwrap();
        assert!(
            f.uid
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        );
        f.owner.execute_unprepared(&format!("CREATE FUNCTION {schema}.pause_audit() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}::bigint); RETURN NEW; END $$; CREATE TRIGGER pause_audit_{} BEFORE INSERT ON webdock_auth.hosting_audit FOR EACH ROW WHEN (NEW.subject='{}' AND NEW.action LIKE 'apps.%.complete') EXECUTE FUNCTION {schema}.pause_audit()",f.cluster,f.uid)).await.unwrap();
    } else {
        tx.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE",
            [app.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    }
    let host = f.service.clone();
    let agent = f.agent.clone();
    let task = tokio::spawn(async move {
        if let Some(read) = read {
            if case.failure() {
                return host
                    .complete_read_failure_with_receipt(
                        &agent,
                        &read,
                        "runtime_failure",
                        &"b".repeat(64),
                    )
                    .await;
            }
            let result = match read.kind {
                webdock_supervisor::ReadKind::Logs => {
                    json!({"logs":"late-output","truncated":false})
                }
                webdock_supervisor::ReadKind::Inspect => {
                    json!({"resourceId":read.spec.resource_id.to_string(),"revision":read.spec.revision,"phase":"running","ready":true,"observedAt":webdock_supervisor::timestamp().unwrap(),"runtimeId":read.expected_runtime_id,"conditions":[]})
                }
                webdock_supervisor::ReadKind::Metrics => {
                    json!({"observedAt":webdock_supervisor::timestamp().unwrap(),"cpuNanoseconds":1,"memoryBytes":1024})
                }
            };
            return host
                .complete_read_with_receipt(&agent, &read, result, &"c".repeat(64))
                .await;
        }
        let command = command.unwrap();
        if matches!(
            case,
            CompletionCase::LegacySuccess | CompletionCase::LegacyFailure
        ) {
            return host.agent_request(&agent,"complete",if case.failure(){json!({"id":command.operation_id,"generation":generation,"outcome":"failed","error":"runtime_failure"})}else{json!({"id":command.operation_id,"generation":generation,"outcome":"succeeded","proof":{"appID":command.spec.resource_id.to_string(),"revision":command.spec.revision,"namespace":format!("wd-{}",command.spec.project_id),"uid":"deadline-owned-runtime","ready":true,"deleted":false,"replicas":1}})}).await;
        }
        if case.failure() {
            return host
                .complete_runtime_failure_with_receipt(
                    &agent,
                    &command,
                    "runtime_failure",
                    &"d".repeat(64),
                )
                .await;
        }
        host.complete_runtime_with_receipt(
            &agent,
            &command,
            &ready_observation(&command),
            &"e".repeat(64),
        )
        .await
    });
    let _abort = AbortCompletion(task.abort_handle());
    wait_completion_blocked(&f, pid).await;
    assert!(
        !database_expired(&f, deadline).await,
        "{case:?}: completion must block while original authority/lease is live"
    );
    while !database_expired(&f, deadline).await {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tx.commit().await.unwrap();
    let completed = task.await.unwrap();
    assert!(completed.is_err(), "{case:?}: delayed completion must fail");
    assert_eq!(
        deadline_state(&f, &operation).await,
        before,
        "{case:?}: all projection/receipt/settlement writes must roll back"
    );
}
async fn wait_completion_blocked(f: &Fixture, pid: i32) {
    tokio::time::timeout(std::time::Duration::from_secs(2),async{loop{
        let waiting:bool=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS waiting",[pid.into()])).await.unwrap().unwrap().try_get("","waiting").unwrap();
        if waiting{return;}tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }}).await.expect("live completion must reach the owned blocking write");
}
async fn database_expired(f: &Fixture, deadline: u64) -> bool {
    f.owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT clock_timestamp()>=to_timestamp($1::double precision) AS expired",
            [i64::try_from(deadline).unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "expired")
        .unwrap()
}
async fn deadline_state(f: &Fixture, operation: &str) -> Value {
    f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT jsonb_build_object('operation',to_jsonb(o),'application',to_jsonb(a),'reservation',(SELECT to_jsonb(r) FROM webdock_auth.hosting_reservation r WHERE r.operation_id=o.id),'audits',(SELECT count(*) FROM webdock_auth.hosting_audit h WHERE h.subject=$2 AND h.action LIKE 'apps.%.complete')) AS state FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_app a ON a.id=o.app_id WHERE o.id=$1",[operation.into(),f.uid.clone().into()])).await.unwrap().unwrap().try_get("","state").unwrap()
}

#[tokio::test]
#[ignore = "owned local restricted-role Hosting fixture and authenticated Unix Core"]
async fn core_retries_late_uncommitted_success_as_custody_without_poisoning_receipt() {
    let fixture = Arc::new(Fixture::new("docker").await);
    let f = fixture.clone();
    let result = tokio::spawn(async move { core_deadline_scenario(f).await }).await;
    fixture
        .owner
        .execute_unprepared(&format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            deadline_schema(&fixture)
        ))
        .await
        .unwrap();
    sql(&fixture.owner,"DELETE FROM webdock_auth.agent_runtime_dispatch WHERE installation=(SELECT runtime_installation FROM webdock_auth.hosting_cluster WHERE id=$1)",vec![fixture.cluster.clone().into()]).await;
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.agent_transport_enrollment WHERE cluster_id=$1",
        vec![fixture.cluster.clone().into()],
    )
    .await;
    fixture.cleanup().await;
    result.unwrap();
}
async fn core_deadline_scenario(f: Arc<Fixture>) {
    use std::os::unix::fs::PermissionsExt;
    use tonic::transport::{Endpoint, Server};
    use webdock_agent_control::{ControlPlane, LocalBinding};
    use webdock_agent_protocol::{self as wire, agent_control_client::AgentControlClient};
    f.configure("docker").await;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let socket = directory.path().join("deadline.sock");
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
    sql(&f.owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'deadline-agent',1,$2,$3,'unix',$4)",vec![installation.clone().into(),f.cluster.clone().into(),hex::encode(Sha256::digest("a".repeat(43))).into(),i64::from(uid).into()]).await;
    let core = ControlPlane::new(
        db("AUTH_DATABASE_URL").await,
        f.service.clone(),
        Some(LocalBinding {
            installation: installation.clone(),
            agent_id: "deadline-agent".into(),
            uid,
        }),
    )
    .await
    .unwrap();
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        Server::builder()
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
    let _abort_server = AbortCompletion(server.abort_handle());
    let mut client = AgentControlClient::new(
        Endpoint::from_shared(format!("unix://{}", socket.display()))
            .unwrap()
            .connect()
            .await
            .unwrap(),
    );
    let identity = wire::AgentIdentity {
        installation,
        agent_id: "deadline-agent".into(),
        generation: 1,
    };
    client
        .register(wire::Hello {
            identity: Some(identity.clone()),
            protocol_version: 1,
            certificate_sha256: String::new(),
            capabilities_json: b"{}".to_vec(),
        })
        .await
        .unwrap();
    let created=f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"deadline-core-app","spec":spec(),"subscriptionRevision":1,"idempotencyKey":"completion-deadline-core"})).await.unwrap();
    let operation = created["id"].as_str().unwrap();
    assert!(operation.bytes().all(|b| b.is_ascii_digit()));
    let schema = deadline_schema(&f);
    f.owner.execute_unprepared(&format!("CREATE SCHEMA {schema}; CREATE FUNCTION {schema}.short_lease() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN NEW.lease_until=date_trunc('second',clock_timestamp())+interval '3 seconds'; RETURN NEW; END $$; CREATE TRIGGER short_lease_{} BEFORE UPDATE OF status ON webdock_auth.hosting_operation FOR EACH ROW WHEN (NEW.id='{operation}' AND OLD.status='queued' AND NEW.status='running') EXECUTE FUNCTION {schema}.short_lease()",f.cluster)).await.unwrap();
    let issued = client
        .poll(wire::PollRequest {
            identity: Some(identity.clone()),
        })
        .await
        .unwrap()
        .into_inner()
        .command
        .unwrap();
    f.owner
        .execute_unprepared(&format!(
            "DROP TRIGGER short_lease_{} ON webdock_auth.hosting_operation",
            f.cluster
        ))
        .await
        .unwrap();
    let stored:Value=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT command FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1 AND generation=$2",[operation.into(),i64::try_from(issued.generation).unwrap().into()])).await.unwrap().unwrap().try_get("","command").unwrap();
    let command: webdock_supervisor::Command = serde_json::from_value(stored).unwrap();
    assert_eq!(command.lease_until, issued.lease_until);
    let receipt = wire::Receipt {
        identity: Some(identity),
        operation_id: operation.into(),
        generation: issued.generation,
        request_sha256: command.fingerprint().unwrap(),
        outcome: wire::Outcome::Succeeded as i32,
        error_code: String::new(),
        result: Some(wire::receipt::Result::ObservedJson(
            serde_json::to_vec(&ready_observation(&command)).unwrap(),
        )),
    };
    let before = deadline_state(&f, operation).await;
    let blocker = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let tx = blocker.begin().await.unwrap();
    let pid: i32 = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    tx.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE",
        [created["appID"].as_str().unwrap().into()],
    ))
    .await
    .unwrap()
    .unwrap();
    let mut sending = client.clone();
    let sent = receipt.clone();
    let completion = tokio::spawn(async move { sending.complete(sent).await });
    let _abort = AbortCompletion(completion.abort_handle());
    wait_completion_blocked(&f, pid).await;
    assert!(!database_expired(&f, issued.lease_until).await);
    while !database_expired(&f, issued.lease_until).await {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tx.commit().await.unwrap();
    assert_eq!(
        completion.await.unwrap().unwrap_err().code(),
        tonic::Code::FailedPrecondition
    );
    assert_eq!(
        deadline_state(&f, operation).await,
        before,
        "late attempt rolls back all business and domain receipt writes"
    );
    let hash:Option<String>=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT receipt_sha256 FROM webdock_auth.agent_runtime_dispatch WHERE operation_id=$1 AND generation=$2",[operation.into(),i64::try_from(issued.generation).unwrap().into()])).await.unwrap().unwrap().try_get("","receipt_sha256").unwrap();
    assert!(
        hash.is_none(),
        "uncommitted late outcome cannot poison transport ACK journal"
    );
    assert!(
        client
            .complete(receipt.clone())
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let retired = deadline_state(&f, operation).await;
    assert_eq!(retired["operation"]["status"], "needs-reconciliation");
    assert_eq!(retired["application"], before["application"]);
    assert_eq!(retired["reservation"], before["reservation"]);
    assert_eq!(retired["audits"], before["audits"]);
    assert!(
        retired["operation"]["desired"]
            .get("nativeHostingAcceptedDeliveries")
            .is_none()
    );
    assert!(
        client
            .complete(receipt)
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    assert_eq!(deadline_state(&f, operation).await, retired);
    let _ = stop.send(());
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "owned local restricted-role Hosting fixture"]
async fn committed_native_success_and_failure_replays_survive_natural_lease_expiry() {
    for case in [
        CompletionCase::MutationSuccess,
        CompletionCase::MutationFailure,
        CompletionCase::LogsSuccess,
        CompletionCase::LogsFailure,
    ] {
        let fixture = Arc::new(Fixture::new("docker").await);
        let f = fixture.clone();
        let result = tokio::spawn(async move { committed_replay_scenario(f, case).await }).await;
        fixture
            .owner
            .execute_unprepared(&format!(
                "DROP SCHEMA IF EXISTS {} CASCADE",
                deadline_schema(&fixture)
            ))
            .await
            .unwrap();
        fixture.cleanup().await;
        result.unwrap();
    }
}
async fn committed_replay_scenario(f: Arc<Fixture>, case: CompletionCase) {
    f.configure("docker").await;
    f.service.execute(&f.principal,json!({"action":"apps.create","projectID":f.pid,"name":"committed-deadline-app","spec":spec(),"subscriptionRevision":1,"idempotencyKey":"committed-deadline-create"})).await.unwrap();
    let initial = f.service.claim_runtime(&f.agent).await.unwrap().unwrap();
    let (mutation, read) = if let Some(action) = case.read_action() {
        f.service
            .complete_runtime(&f.agent, &initial, &ready_observation(&initial))
            .await
            .unwrap();
        let requested=f.service.execute(&f.principal,json!({"action":action,"appID":initial.spec.resource_id.to_string(),"idempotencyKey":"committed-deadline-read"})).await.unwrap();
        let id = requested["id"].as_str().unwrap();
        assert!(id.bytes().all(|b| b.is_ascii_digit()));
        let schema = deadline_schema(&f);
        f.owner.execute_unprepared(&format!("CREATE SCHEMA {schema}; CREATE FUNCTION {schema}.short_lease() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN NEW.lease_until=date_trunc('second',clock_timestamp())+interval '3 seconds'; RETURN NEW; END $$; CREATE TRIGGER short_lease_{} BEFORE UPDATE OF status ON webdock_auth.hosting_operation FOR EACH ROW WHEN (NEW.id='{id}' AND OLD.status='queued' AND NEW.status='running') EXECUTE FUNCTION {schema}.short_lease()",f.cluster)).await.unwrap();
        let read = f.service.claim_read(&f.agent).await.unwrap().unwrap();
        f.owner
            .execute_unprepared(&format!(
                "DROP TRIGGER short_lease_{} ON webdock_auth.hosting_operation",
                f.cluster
            ))
            .await
            .unwrap();
        (None, Some(read))
    } else {
        sql(&f.owner,"UPDATE webdock_auth.hosting_operation SET lease_until=date_trunc('second',clock_timestamp())+interval '3 seconds' WHERE id=$1",vec![initial.operation_id.clone().into()]).await;
        (
            Some(
                f.service
                    .runtime_command(&f.agent, &initial.operation_id, initial.generation)
                    .await
                    .unwrap(),
            ),
            None,
        )
    };
    let (id, deadline) = if let Some(command) = &mutation {
        (command.operation_id.as_str(), command.lease_until)
    } else {
        let read = read.as_ref().unwrap();
        (read.operation_id.as_str(), read.lease_until)
    };
    let observed = mutation.as_ref().map(ready_observation);
    let complete = || async {
        match (&mutation, &read, case.failure()) {
            (Some(command), None, true) => {
                f.service
                    .complete_runtime_failure_with_receipt(
                        &f.agent,
                        command,
                        "runtime_failure",
                        &"f".repeat(64),
                    )
                    .await
            }
            (Some(command), None, false) => {
                f.service
                    .complete_runtime_with_receipt(
                        &f.agent,
                        command,
                        observed.as_ref().unwrap(),
                        &"f".repeat(64),
                    )
                    .await
            }
            (None, Some(command), true) => {
                f.service
                    .complete_read_failure_with_receipt(
                        &f.agent,
                        command,
                        "runtime_failure",
                        &"f".repeat(64),
                    )
                    .await
            }
            (None, Some(command), false) => {
                f.service
                    .complete_read_with_receipt(
                        &f.agent,
                        command,
                        json!({"logs":"committed-output","truncated":false}),
                        &"f".repeat(64),
                    )
                    .await
            }
            _ => panic!("exactly one fixture command is required"),
        }
    };
    let original = complete().await.unwrap();
    let committed = deadline_state(&f, id).await;
    while !database_expired(&f, deadline).await {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        complete().await.unwrap(),
        original,
        "{case:?}: exact committed replay remains ACKable after expiry"
    );
    assert_eq!(
        deadline_state(&f, id).await,
        committed,
        "{case:?}: replay cannot repeat business or audit outcomes"
    );
}
