#[path = "support/jobs_fixture.rs"]
mod fixture;
#[path = "support/jobs_review.rs"]
mod review;
#[path = "support/jobs_rpc.rs"]
mod rpc;
#[tokio::test]
#[ignore = "owns PostgreSQL18 and real saturated native runtime"]
async fn final_core_retries_after_actual_runtime_saturation() {
    rpc::verify_saturation().await;
}
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::sync::Arc;
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{AdmissionInput, FileBindings, HostService};
use webdock_plugin_runtime::PeerIdentity;

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn slow_admission_and_claim_keep_exact_signed_invocation_lifetimes() {
    let f = fixture::JobFixture::start().await;
    f.owner.execute_unprepared("CREATE FUNCTION webdock_auth.fixture_slow_grant() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(1.1); RETURN NEW; END $$; CREATE TRIGGER fixture_slow_grant BEFORE INSERT ON webdock_auth.plugin_grant FOR EACH ROW EXECUTE FUNCTION webdock_auth.fixture_slow_grant()").await.unwrap();
    f.enqueue("slow-parent-admission").await;
    f.owner.execute_unprepared("DROP TRIGGER fixture_slow_grant ON webdock_auth.plugin_grant; DROP FUNCTION webdock_auth.fixture_slow_grant()").await.unwrap();
    let lock = f.owner.begin().await.unwrap();
    lock.execute_unprepared("SELECT id FROM webdock_auth.plugin_installation FOR UPDATE")
        .await
        .unwrap();
    let host = f.host.clone();
    let claim = tokio::spawn(async move { host.claim_job(None).await });
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    lock.commit().await.unwrap();
    let lease = claim.await.unwrap().unwrap().unwrap();
    f.host
        .authenticate(peer(), &lease.invocation().token)
        .await
        .unwrap();
    f.host.complete_job(&lease, Ok(json!(null))).await.unwrap();
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn job_grant_changes_on_promotion_advance_revision_without_reviving_revoked_grants() {
    let f = fixture::JobFixture::start().await;
    f.enqueue("initial-grant").await;
    sql(
        &f.owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('126','Other','123')",
        vec![],
    )
    .await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities,jobs,revision,revoked_at) SELECT plugin_id,customer_id,'126',capabilities,jobs,revision,now() FROM webdock_auth.plugin_grant WHERE project_id='124'",vec![]).await;
    let mut manifest = f.manifest.clone();
    manifest["jobs"][0]["name"] = json!("newwork");
    let raw = manifest.to_string();
    use sha2::{Digest, Sha256};
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_release SET manifest_bytes=$1,manifest_sha256=$2",
        vec![
            raw.clone().into(),
            hex::encode(Sha256::digest(raw.as_bytes())).into(),
        ],
    )
    .await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_installation SET current_generation=2",
        vec![],
    )
    .await;
    assert_eq!(
        scalar(
            &f.owner,
            "SELECT jobs AS value FROM webdock_auth.plugin_grant WHERE project_id='124'"
        )
        .await,
        json!(["newwork"])
    );
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(revision>0) AS value FROM webdock_auth.plugin_grant WHERE project_id='124'").await,json!(true));
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(revoked_at IS NOT NULL AND jobs='[\"work\"]'::jsonb) AS value FROM webdock_auth.plugin_grant WHERE project_id='126'").await,json!(true));
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn legacy_rows_cancel_and_verifier_outage_preserves_pending_work() {
    let f = fixture::JobFixture::start().await;
    f.enqueue("native-pending").await;
    sql(&f.owner,"INSERT INTO webdock_auth.plugin_job_execution(id,plugin_id,customer_id,project_id,name,payload,grant_revision,runtime_generation,available_at) VALUES('legacy','job-fixture','123','124','work','{}',0,1,now()-interval '1 hour')",vec![]).await;
    assert!(f.host.claim_job(None).await.unwrap().is_none());
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(status) AS value FROM webdock_auth.plugin_job_execution WHERE id='legacy'").await,json!("cancelled"));
    f.auth.close().await.unwrap();
    assert_eq!(
        f.host.claim_job(None).await.err().unwrap().code,
        webdock_plugin_protocol::ErrorCode::Unavailable
    );
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_job_execution WHERE status='pending'").await,json!(1));
}

fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn scalar(db: &DatabaseConnection, query: &str) -> Value {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, query))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn leases_retry_exhaustion_cancellation_and_stale_effects_are_fenced() {
    use webdock_plugin_protocol::{ErrorCode, PluginError};
    let f = fixture::JobFixture::start().await;
    let request = f.enqueue("lease-recovery").await;
    let (a, b) = tokio::join!(f.host.claim_job(None), f.host.claim_job(None));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    let old = a.or(b).unwrap();
    let old_handle = f
        .host
        .authenticate(peer(), &old.invocation().token)
        .await
        .unwrap();
    f.host.data_write(&old_handle,json!({"key":"receipt","value":1,"expectedRevision":null,"idempotencyKey":"effect-once"})).await.unwrap();
    sql(&f.owner,"UPDATE webdock_auth.plugin_job_execution SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1",vec![old.id().into()]).await;
    assert!(f.host.data_write(&old_handle,json!({"key":"late-effect","value":1,"expectedRevision":null,"idempotencyKey":"late-effect"})).await.is_err());
    let replacement = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(&old, Ok(json!("stale result")))
        .await
        .unwrap();
    let handle = f
        .host
        .authenticate(peer(), &replacement.invocation().token)
        .await
        .unwrap();
    f.host.data_write(&handle,json!({"key":"receipt","value":1,"expectedRevision":null,"idempotencyKey":"effect-once"})).await.unwrap();
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(revision) AS value FROM webdock_auth.plugin_document WHERE key='receipt'").await,json!(1));
    f.host
        .complete_job(
            &replacement,
            Err(PluginError::new(
                ErrorCode::Unavailable,
                "PRIVATE diagnostic",
            )),
        )
        .await
        .unwrap();
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_job_execution SET available_at=clock_timestamp() WHERE id=$1",
        vec![old.id().into()],
    )
    .await;
    let final_attempt = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(
            &final_attempt,
            Err(PluginError::new(
                ErrorCode::Unavailable,
                "PRIVATE diagnostic",
            )),
        )
        .await
        .unwrap();
    assert!(f.host.claim_job(None).await.unwrap().is_none());
    let read = f.handle("inspect", "read-failed").await;
    let status = f.host.jobs_get(&read, request).await.unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(status["attempts"], 3);
    assert!(!status.to_string().contains("PRIVATE"));
    let cancelled = f.enqueue("cancel-pending").await;
    let cancel = f.handle("cancel", "cancel-pending-action").await;
    f.host
        .jobs_cancel(&cancel, cancelled.clone())
        .await
        .unwrap();
    assert!(f.host.claim_job(None).await.unwrap().is_none());
    assert_eq!(
        f.host.jobs_get(&read, cancelled.clone()).await.unwrap()["state"],
        "cancelled"
    );
    assert_eq!(f.enqueue("cancel-pending").await, cancelled);
    let active = f.enqueue("cancel-running").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let handle = f
        .host
        .authenticate(peer(), &lease.invocation().token)
        .await
        .unwrap();
    f.host.jobs_cancel(&cancel, active.clone()).await.unwrap();
    assert!(f.host.revalidate(&handle).await.is_err());
    f.host
        .complete_job(&lease, Ok(json!("late")))
        .await
        .unwrap();
    assert_eq!(
        f.host.jobs_get(&read, active).await.unwrap()["state"],
        "cancelled"
    );
    let maximum = f
        .host
        .jobs_enqueue(
            &f.handle("start", "five-attempt-parent").await,
            json!({"name":"work","payload":{},"idempotencyKey":"five-attempts","attempts":5}),
        )
        .await
        .unwrap();
    for attempt in 1..=5 {
        sql(&f.owner,"UPDATE webdock_auth.plugin_job_execution SET available_at=clock_timestamp() WHERE id=$1",vec![maximum["id"].as_str().unwrap().into()]).await;
        let lease = f.host.claim_job(None).await.unwrap().unwrap();
        assert_eq!(
            lease.invocation().claims.job.as_ref().unwrap().attempt,
            attempt
        );
        f.host
            .complete_job(
                &lease,
                Err(PluginError::new(ErrorCode::Unavailable, "private")),
            )
            .await
            .unwrap();
    }
    assert!(f.host.claim_job(None).await.unwrap().is_none());
    assert_eq!(
        f.host.jobs_get(&read, maximum).await.unwrap()["attempts"],
        5
    );
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn strict_contract_sql_scope_and_pool_one_concurrent_admission() {
    use webdock_plugin_protocol::ErrorCode;
    let f = fixture::JobFixture::start().await;
    let handle = f.handle("start", "contract-parent").await;
    let good = json!({"name":"work","payload":{},"idempotencyKey":"contract-one"});
    for (key, value) in [
        ("delaySeconds", json!(0)),
        ("runAt", json!("2026-10-24T00:00:00Z")),
        ("attempts", json!(6)),
        ("name", json!("../work")),
        ("customerID", json!("123")),
        ("payload", json!("x".repeat(32769))),
    ] {
        let mut input = good.clone();
        input[key] = value;
        assert_eq!(
            f.host.jobs_enqueue(&handle, input).await.unwrap_err().code,
            ErrorCode::InvalidInput
        );
    }
    let read = f.handle("inspect", "read-only-parent").await;
    assert!(f.host.jobs_enqueue(&read, good.clone()).await.is_err());
    let job = f.host.jobs_enqueue(&handle, good.clone()).await.unwrap();
    let mut different = good;
    different["payload"] = json!(1);
    assert_eq!(
        f.host
            .jobs_enqueue(&handle, different)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let business = f.database.role("webdock_auth_runtime").await;
    assert_eq!(
        scalar(
            &business,
            "SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_job_execution"
        )
        .await,
        json!(0)
    );
    assert_eq!(scalar(&business,"SELECT to_jsonb(webdock_rls.plugin_job_enqueue('job-0000000000000000000000000000000000000000000000000000000000000000','work','forged-one','{}',3,repeat('a',64)) IS NULL) AS value").await,json!(true));
    // All three pools have max_connections=1. This catches worker->auth inversion in admission.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let (enqueued, claim) = tokio::join!(
            async {
                let admission = f.admit("start", "parallel-parent").await;
                let handle = f.host.authenticate(peer(), &admission.token).await.unwrap();
                f.host
                    .jobs_enqueue(
                        &handle,
                        json!({"name":"work","payload":{},"idempotencyKey":"parallel-enqueue"}),
                    )
                    .await
            },
            f.host.claim_job(None)
        );
        enqueued.unwrap();
        let claim = claim.unwrap().unwrap();
        assert!(
            f.host
                .jobs_enqueue(
                    &f.host
                        .authenticate(peer(), &claim.invocation().token)
                        .await
                        .unwrap(),
                    json!({"name":"work","payload":{},"idempotencyKey":"nested-denial"})
                )
                .await
                .is_err()
        );
        f.host.complete_job(&claim, Ok(json!(null))).await.unwrap();
    })
    .await
    .expect("auth/business/worker pool1 admission must make progress");
    assert_eq!(
        f.host.jobs_get(&read, job.clone()).await.unwrap()["state"],
        "completed"
    );
    sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('126','Foreign project','123')",vec![]).await;
    let foreign=f.host.admit_with_authority(&f.actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"job-fixture","projectID":"126","action":"start","input":{},"idempotencyKey":"foreign-parent"})).unwrap(),true,&webdock_browser_session::StoredAuthority::LocalSession{}).await.unwrap();
    let foreign = f.host.authenticate(peer(), &foreign.token).await.unwrap();
    assert_eq!(
        f.host
            .jobs_get(&foreign, job.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        f.host.jobs_cancel(&foreign, job).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    // Cancel and completion linearize on the job row; either terminal winner is retained.
    let queued = f.enqueue("race-cancel-complete").await;
    let earlier = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(&earlier, Ok(json!(null)))
        .await
        .unwrap(); // parallel-enqueue
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let (cancel, complete) = tokio::join!(
        f.host.jobs_cancel(&handle, queued.clone()),
        f.host.complete_job(&lease, Ok(json!(null)))
    );
    complete.unwrap();
    let state = f.host.jobs_get(&read, queued.clone()).await.unwrap();
    if cancel.is_ok() {
        assert_eq!(state["state"], "cancelled");
    } else {
        assert_eq!(state["state"], "completed");
    }
    f.host
        .complete_job(&lease, Ok(json!("stale")))
        .await
        .unwrap();
    assert_eq!(f.host.jobs_get(&read, queued).await.unwrap(), state);
    let boundary = f
        .host
        .jobs_enqueue(
            &handle,
            json!({"name":"work","payload":"x".repeat(32766),"idempotencyKey":"payload-boundary"}),
        )
        .await
        .unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(&lease, Ok(json!("x".repeat(32766))))
        .await
        .unwrap();
    assert_eq!(
        f.host.jobs_get(&read, boundary).await.unwrap()["state"],
        "completed"
    );
    let oversized = f.enqueue("result-overflow").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(&lease, Ok(json!("x".repeat(32767))))
        .await
        .unwrap();
    assert_eq!(
        f.host.jobs_get(&read, oversized).await.unwrap()["state"],
        "failed"
    );
    let deep = f.enqueue("result-nesting").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let mut value = json!(0);
    for _ in 0..70 {
        value = json!([value]);
    }
    f.host.complete_job(&lease, Ok(value)).await.unwrap();
    assert_eq!(
        f.host.jobs_get(&read, deep).await.unwrap()["state"],
        "failed"
    );
    // A valid old actorless worker signature cannot replace the original session scope.
    let active = f.enqueue("worker-scope-denial").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let worker = f.database.role("webdock_worker_runtime").await;
    let tx = worker.begin().await.unwrap();
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"system","purpose":"plugin-job","pluginInvocationID":lease.invocation().claims.id,"authProvider":"better-auth"}}).to_string();
    use hmac::{Hmac, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(b"owned-plugin-jobs-rls-secret-32bytes").unwrap();
    mac.update(raw.as_bytes());
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[raw.into(),hex::encode(mac.finalize().into_bytes()).into()])).await.unwrap();
    let scope: Option<Value> = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.plugin_context('plugin.data.write') AS value",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap();
    assert!(scope.is_none());
    tx.commit().await.unwrap();
    f.host.jobs_cancel(&handle, active).await.unwrap();
}
async fn sql(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
#[tokio::test]
#[ignore = "owns PostgreSQL18 and real private Host/runtime RPCs"]
async fn core_dispatcher_retries_real_host_effect_once_and_cancels_live_rpc() {
    rpc::verify().await;
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn actor_policy_invalidation_cancels_and_pending_jobs_drain() {
    let f = fixture::JobFixture::start().await;
    let pending = f.enqueue("accepted-before-drain").await;
    let handle = f.handle("start", "pre-drain-parent").await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_installation SET state='draining',desired_state='disabled'",
        vec![],
    )
    .await;
    assert!(
        f.host
            .jobs_enqueue(
                &handle,
                json!({"name":"work","payload":{},"idempotencyKey":"deny-in-drain"})
            )
            .await
            .is_err()
    );
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(
            &lease,
            Err(webdock_plugin_protocol::PluginError::new(
                webdock_plugin_protocol::ErrorCode::Unavailable,
                "transient",
            )),
        )
        .await
        .unwrap();
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_job_execution SET available_at=clock_timestamp() WHERE id=$1",
        vec![pending["id"].as_str().unwrap().into()],
    )
    .await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host.complete_job(&lease, Ok(json!(null))).await.unwrap();
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_job_execution WHERE status IN ('pending','running')").await,json!(0));
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_installation SET state='enabled',desired_state='enabled'",
        vec![],
    )
    .await;

    for (key, change, restore) in [
        (
            "session-expiry",
            "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id='session'",
            "UPDATE webdock_auth.session SET \"expiresAt\"=now()+interval '1 hour' WHERE id='session'",
        ),
        (
            "project-inactive",
            "UPDATE webdock_admin.projects SET status='archived' WHERE id='124'",
            "UPDATE webdock_admin.projects SET status='active' WHERE id='124'",
        ),
        (
            "customer-inactive",
            "UPDATE webdock_admin.customers SET status='archived' WHERE id='123'",
            "UPDATE webdock_admin.customers SET status='active' WHERE id='123'",
        ),
        (
            "grant-revoked",
            "UPDATE webdock_auth.plugin_grant SET revoked_at=now()",
            "UPDATE webdock_auth.plugin_grant SET revoked_at=NULL",
        ),
        (
            "config-revision",
            "UPDATE webdock_auth.plugin_configuration SET revision=revision+1",
            "UPDATE webdock_auth.plugin_configuration SET revision=revision-1",
        ),
        (
            "runtime-generation",
            "UPDATE webdock_auth.plugin_installation SET current_generation=2",
            "UPDATE webdock_auth.plugin_installation SET current_generation=1",
        ),
    ] {
        let job = f.enqueue(key).await;
        sql(&f.owner, change, vec![]).await;
        assert!(f.host.claim_job(None).await.unwrap().is_none(), "{key}");
        assert_eq!(scalar(&f.owner,&format!("SELECT to_jsonb(status) AS value FROM webdock_auth.plugin_job_execution WHERE id='{}'",job["id"].as_str().unwrap())).await,json!("cancelled"),"{key}");
        sql(&f.owner, restore, vec![]).await;
    }
    // A customer job retains the parent's feature/resource gate after the action has ended.
    for (key, change, restore) in [
        (
            "feature-loss",
            "UPDATE webdock_auth.plugin_feature_policy SET mode='assigned' WHERE feature_id='run'",
            "UPDATE webdock_auth.plugin_feature_policy SET mode='everyone' WHERE feature_id='run'",
        ),
        (
            "resource-loss",
            "UPDATE webdock_auth.plugin_resource SET project_id='126' WHERE resource_id='resource-one'",
            "UPDATE webdock_auth.plugin_resource SET project_id='124' WHERE resource_id='resource-one'",
        ),
        (
            "membership-loss",
            "DELETE FROM webdock_auth.member WHERE id='member-binding'",
            "INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-binding','125','member','owner',now())",
        ),
    ] {
        if key == "resource-loss" {
            sql(&f.owner,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('126','Other','123')",vec![]).await;
        }
        let parent = f
            .admit_subject(
                "member",
                "member-session",
                "customer-start",
                json!({"resource_id":"resource-one"}),
                key,
            )
            .await;
        let parent_handle = f.host.authenticate(peer(), &parent.token).await.unwrap();
        let job = f
            .host
            .jobs_enqueue(
                &parent_handle,
                json!({"name":"work","payload":{},"idempotencyKey":key}),
            )
            .await
            .unwrap();
        f.host.revoke_issued(&parent.token).await.unwrap();
        sql(&f.owner, change, vec![]).await;
        assert!(f.host.claim_job(None).await.unwrap().is_none(), "{key}");
        assert_eq!(scalar(&f.owner,&format!("SELECT to_jsonb(status) AS value FROM webdock_auth.plugin_job_execution WHERE id='{}'",job["id"].as_str().unwrap())).await,json!("cancelled"));
        sql(&f.owner, restore, vec![]).await;
    }
    sql(&f.owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES('session','operator','123','125','owner')",vec![]).await;
    let parent = f
        .admit_subject(
            "operator",
            "session",
            "customer-start",
            json!({"resource_id":"resource-one"}),
            "acting-parent",
        )
        .await;
    let parent_handle = f.host.authenticate(peer(), &parent.token).await.unwrap();
    let job = f
        .host
        .jobs_enqueue(
            &parent_handle,
            json!({"name":"work","payload":{},"idempotencyKey":"acting-epoch"}),
        )
        .await
        .unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let handle = f
        .host
        .authenticate(peer(), &lease.invocation().token)
        .await
        .unwrap();
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id='session'",
        vec![],
    )
    .await;
    sql(&f.owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES('session','operator','123','125','owner')",vec![]).await;
    assert!(f.host.revalidate(&handle).await.is_err());
    f.host
        .complete_job(&lease, Ok(json!("old scope")))
        .await
        .unwrap();
    assert_eq!(scalar(&f.owner,&format!("SELECT to_jsonb(status) AS value FROM webdock_auth.plugin_job_execution WHERE id='{}'",job["id"].as_str().unwrap())).await,json!("cancelled"));
}
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn durable_job_survives_parent_cleanup_and_uses_original_session() {
    let fixture = fixture::JobFixture::start().await;
    let host = &fixture.host;
    let actor = &fixture.actor;
    let admitted=host.admit_with_authority(actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"job-fixture","projectID":"124","action":"start","input":{},"idempotencyKey":"parent-action"})).unwrap(),true,&webdock_browser_session::StoredAuthority::LocalSession{}).await.unwrap();
    let handle = host
        .authenticate(
            PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
            &admitted.token,
        )
        .await
        .unwrap();
    let input = json!({"name":"work","payload":{"hello":"world"},"idempotencyKey":"request-one"});
    let job = host.jobs_enqueue(&handle, input.clone()).await.unwrap();
    assert_eq!(host.jobs_enqueue(&handle, input).await.unwrap(), job);
    host.revoke_issued(&admitted.token).await.unwrap();
    let lease = host
        .claim_job(None)
        .await
        .unwrap()
        .expect("durable job after parent cleanup");
    assert_eq!(lease.id(), job["id"].as_str().unwrap());
    let job_handle = host
        .authenticate(
            PeerIdentity::Unix {
                uid: rustix::process::geteuid().as_raw(),
            },
            &lease.invocation().token,
        )
        .await
        .unwrap();
    host.data_write(
        &job_handle,
        json!({"key":"receipt","value":1,"expectedRevision":null,"idempotencyKey":"write-once"}),
    )
    .await
    .unwrap();
    host.complete_job(&lease, Ok(json!({"done":true})))
        .await
        .unwrap();
}
