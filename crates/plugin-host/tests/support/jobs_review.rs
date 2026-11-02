use super::*;
use webdock_plugin_protocol::ErrorCode;

// Pause an actual claim after discovery/auth but before it acquires the job row lock.
// This uses the existing enrollment boundary rather than adding a production test hook.
struct ClaimCheckpoint {
    file: FileBindings,
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl webdock_plugin_host::RuntimeBindings for ClaimCheckpoint {
    async fn runtime(
        &self,
        plugin: &str,
    ) -> webdock_plugin_protocol::Result<Option<webdock_plugin_host::RuntimeBinding>> {
        let binding = webdock_plugin_host::RuntimeBindings::runtime(&self.file, plugin).await?;
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(binding)
    }
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 with parallel claims and a controlled enrollment checkpoint"]
async fn final_stale_discovery_cannot_claim_exhausted_or_cancel_requested_work() {
    let f = fixture::JobFixture::start().await;
    for cancelled in [false, true] {
        let parent = f
            .handle(
                "start",
                if cancelled {
                    "stale-cancel-parent"
                } else {
                    "stale-exhaust-parent"
                },
            )
            .await;
        let job=f.host.jobs_enqueue(&parent,json!({"name":"work","payload":{},"idempotencyKey":if cancelled {"stale-cancel-job"}else{"stale-exhaust-job"},"attempts":1})).await.unwrap();
        let checkpoint = Arc::new(ClaimCheckpoint {
            file: FileBindings::new(f.directory.path().join("bindings.json")),
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        });
        let stalled = Arc::new(
            HostService::new(
                f.database.role("webdock_auth_runtime").await,
                f.database.role("webdock_worker_runtime").await,
                "owned-plugin-jobs-rls-secret-32bytes".into(),
                Arc::new(webdock_persistence::IdAllocator::new(
                    f.database.role("webdock_auth_runtime").await,
                    webdock_ids::Identity::platform(0, 3).unwrap(),
                )),
                checkpoint.clone(),
                ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
            )
            .await
            .unwrap()
            .with_session_authority(
                f.database.role("webdock_auth_bootstrap").await,
                f.identity.clone(),
                None,
            )
            .await
            .unwrap(),
        );
        let task = tokio::spawn(async move { stalled.claim_job(None).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            checkpoint.entered.notified(),
        )
        .await
        .unwrap();
        let attempts_before = if cancelled {
            sql(
                &f.owner,
                "UPDATE webdock_auth.plugin_job_execution SET cancel_requested=true WHERE id=$1",
                vec![job["id"].as_str().unwrap().into()],
            )
            .await;
            0
        } else {
            let winner = f.host.claim_job(None).await.unwrap().unwrap();
            assert_eq!(winner.id(), job["id"].as_str().unwrap());
            assert_eq!(winner.invocation().claims.job.as_ref().unwrap().attempt, 1);
            // Simulate the winning worker crashing and its fixed lease expiring; no completion.
            sql(&f.owner,"UPDATE webdock_auth.plugin_job_execution SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1",vec![winner.id().into()]).await;
            1
        };
        checkpoint.resume.notify_one();
        let candidate = tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            candidate.is_none(),
            "fresh exhausted/cancel-requested work must not receive another claim; observed attempt {:?}",
            candidate.as_ref().and_then(|lease| lease
                .invocation()
                .claims
                .job
                .as_ref()
                .map(|job| job.attempt))
        );
        let attempts=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(attempts) AS value FROM webdock_auth.plugin_job_execution WHERE id=$1",[job["id"].as_str().unwrap().into()])).await.unwrap().unwrap().try_get::<Value>("","value").unwrap();
        assert_eq!(attempts, json!(attempts_before));
        assert!(
            f.host.claim_job(None).await.unwrap().is_none(),
            "next pass terminalizes independently of runtime execution"
        );
        assert_eq!(
            state(&f, job["id"].as_str().unwrap()).await,
            if cancelled { "cancelled" } else { "failed" }
        );
        let invoked=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_invocation WHERE job_execution_id=$1",[job["id"].as_str().unwrap().into()])).await.unwrap().unwrap().try_get::<Value>("","value").unwrap();
        assert_eq!(invoked, json!(attempts_before));
    }
}

async fn state(f: &fixture::JobFixture, id: &str) -> Value {
    f.owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(status) AS value FROM webdock_auth.plugin_job_execution WHERE id=$1",
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}
#[tokio::test]
#[ignore = "owns PostgreSQL18 and private enrollment fixture"]
async fn final_removed_binding_cancels_head_and_later_work_progresses() {
    let f = fixture::JobFixture::start().await;
    let first = f.enqueue("removed-first").await;
    let later = f.enqueue("removed-later").await;
    let path = f.directory.path().join("bindings.json");
    let enrolled = std::fs::read(&path).unwrap();
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_installation SET state='draining',desired_state='disabled'",
        vec![],
    )
    .await;
    std::fs::write(&path, b"{\"runtimes\":{}}").unwrap();
    assert!(f.host.claim_job(None).await.unwrap().is_none());
    assert_eq!(state(&f, first["id"].as_str().unwrap()).await, "cancelled");
    std::fs::write(&path, enrolled).unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    assert_eq!(lease.id(), later["id"].as_str().unwrap());
    f.host.complete_job(&lease, Ok(json!(null))).await.unwrap();
    assert_eq!(scalar(&f.owner,"SELECT to_jsonb(count(*)) AS value FROM webdock_auth.plugin_job_execution WHERE status IN ('pending','running')").await,0);
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 and private enrollment fixture"]
async fn final_exhausted_cleanup_ignores_unreadable_runtime_and_preserves_transient_work() {
    let f = fixture::JobFixture::start().await;
    let parent = f.handle("start", "exhausted-parent").await;
    let exhausted = f
        .host
        .jobs_enqueue(
            &parent,
            json!({"name":"work","payload":{},"idempotencyKey":"exhausted-job","attempts":1}),
        )
        .await
        .unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let later = f.enqueue("transient-later").await;
    sql(&f.owner,"UPDATE webdock_auth.plugin_job_execution SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1",vec![lease.id().into()]).await;
    let path = f.directory.path().join("bindings.json");
    let enrolled = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{").unwrap();
    assert!(f.host.claim_job(None).await.unwrap().is_none());
    assert_eq!(state(&f, exhausted["id"].as_str().unwrap()).await, "failed");
    assert_eq!(
        f.host.claim_job(None).await.err().unwrap().code,
        ErrorCode::Unavailable
    );
    assert_eq!(state(&f, later["id"].as_str().unwrap()).await, "pending");
    std::fs::write(&path, enrolled).unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    assert_eq!(lease.id(), later["id"].as_str().unwrap());
    f.host.complete_job(&lease, Ok(json!(null))).await.unwrap();
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 with lossless Rust JSON values"]
async fn final_payload_fingerprint_distinguishes_integers_and_sorts_nested_keys() {
    let f = fixture::JobFixture::start().await;
    let parent = f.handle("start", "lossless-parent").await;
    let input = json!({"name":"work","payload":{"n":9007199254740992_u64,"nested":{"z":1,"a":2}},"idempotencyKey":"lossless-job"});
    let job = f.host.jobs_enqueue(&parent, input.clone()).await.unwrap();
    let mut changed = input.clone();
    changed["payload"]["n"] = json!(9007199254740993_u64);
    assert_eq!(
        f.host
            .jobs_enqueue(&parent, changed.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let reordered:Value=serde_json::from_str(r#"{"attempts":3,"payload":{"nested":{"a":2,"z":1},"n":9007199254740992},"idempotencyKey":"lossless-job","name":"work"}"#).unwrap();
    assert_eq!(f.host.jobs_enqueue(&parent, reordered).await.unwrap(), job);
    changed["idempotencyKey"] = json!("lossless-next");
    let next = f.host.jobs_enqueue(&parent, changed).await.unwrap();
    let first = f.host.claim_job(None).await.unwrap().unwrap();
    f.host.complete_job(&first, Ok(json!(null))).await.unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    assert_eq!(lease.id(), next["id"].as_str().unwrap());
    assert_eq!(
        lease.invocation().input["n"].as_u64(),
        Some(9007199254740993)
    );
    let handle = f
        .host
        .authenticate(peer(), &lease.invocation().token)
        .await
        .unwrap();
    let mut altered = lease.invocation().input.clone();
    altered["n"] = json!(9007199254740992_u64);
    assert!(
        f.host
            .verify_request(&handle, &altered, &lease.invocation().configuration)
            .await
            .is_err()
    );
    f.host
        .verify_request(
            &handle,
            &lease.invocation().input,
            &lease.invocation().configuration,
        )
        .await
        .unwrap();
    f.host.complete_job(&lease, Ok(json!(null))).await.unwrap();
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 exponent-expanded JSON persistence"]
async fn final_exponent_payload_preserves_compact_bound_through_claim_and_completion() {
    let f = fixture::JobFixture::start().await;
    let parent = f.handle("start", "exponent-payload-parent").await;
    let payload = json!(vec![1e308_f64; 220]);
    assert!(serde_json::to_vec(&payload).unwrap().len() < 32768);
    let job = f
        .host
        .jobs_enqueue(
            &parent,
            json!({"name":"work","payload":payload,"idempotencyKey":"exponent-payload"}),
        )
        .await
        .unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    assert_eq!(lease.invocation().input, payload);
    f.host
        .complete_job(&lease, Ok(json!("done")))
        .await
        .unwrap();
    let read = f.handle("inspect", "exponent-payload-read").await;
    assert_eq!(
        f.host.jobs_get(&read, job).await.unwrap()["state"],
        "completed"
    );
    assert_eq!(f.host.jobs_enqueue(&parent,json!({"name":"work","payload":"x".repeat(32767),"idempotencyKey":"exponent-oversize"})).await.unwrap_err().code,ErrorCode::InvalidInput);
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 exponent-expanded JSON persistence"]
async fn final_exponent_result_completes_with_exact_compact_bound() {
    let f = fixture::JobFixture::start().await;
    let job = f.enqueue("exponent-result").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    let result = json!(vec![1e308_f64; 220]);
    f.host
        .complete_job(&lease, Ok(result.clone()))
        .await
        .unwrap();
    let read = f.handle("inspect", "exponent-result-read").await;
    let snapshot = f.host.jobs_get(&read, job).await.unwrap();
    assert_eq!(snapshot["state"], "completed");
    assert_eq!(snapshot["result"], result);
    let oversized = f.enqueue("result-oversized-review").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(&lease, Ok(json!("x".repeat(32767))))
        .await
        .unwrap();
    assert_eq!(
        f.host.jobs_get(&read, oversized).await.unwrap()["state"],
        "failed"
    );
}

async fn configuration_case(configuration: Value, schema: Value) {
    let f = fixture::JobFixture::start().await;
    assert!(serde_json::to_vec(&configuration).unwrap().len() <= 65536);
    let mut descriptor = f.manifest.clone();
    descriptor["configuration"] = schema;
    manifest(&f, &descriptor).await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_configuration SET configuration=$1",
        vec![configuration.clone().into()],
    )
    .await;
    let job = f.enqueue("large-config-job").await;
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    assert_eq!(lease.invocation().configuration, configuration);
    let handle = f
        .host
        .authenticate(peer(), &lease.invocation().token)
        .await
        .unwrap();
    f.host
        .verify_request(
            &handle,
            &lease.invocation().input,
            &lease.invocation().configuration,
        )
        .await
        .unwrap();
    f.host
        .complete_job(&lease, Ok(json!("done")))
        .await
        .unwrap();
    let read = f.handle("inspect", "large-config-read").await;
    assert_eq!(
        f.host.jobs_get(&read, job).await.unwrap()["state"],
        "completed"
    );
    assert!(f.owner.execute_unprepared("UPDATE webdock_auth.plugin_invocation SET session_context=session_context||jsonb_build_object('extraAuthority',repeat('x',16384))").await.is_err(),"authority/context overhead must retain its original bound");
    if configuration["data"].is_string() {
        sql(&f.owner,"UPDATE webdock_auth.plugin_configuration SET configuration=jsonb_build_object('data',repeat('x',65536))",vec![]).await;
        let attempted=f.host.admit_with_authority(&f.actor,serde_json::from_value::<AdmissionInput>(json!({"pluginID":"job-fixture","projectID":"124","action":"start","input":{},"idempotencyKey":"oversize-config"})).unwrap(),true,&webdock_browser_session::StoredAuthority::LocalSession{}).await;
        assert_eq!(attempted.err().unwrap().code, ErrorCode::InvalidInput);
    }
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 with valid 40KiB public configuration"]
async fn final_source_provenance_supports_40k_configuration() {
    configuration_case(json!({"data":"x".repeat(40960)}),json!({"type":"object","properties":{"data":{"type":"string"}},"additionalProperties":false})).await;
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 with exponent-expanded 64KiB configuration"]
async fn final_source_provenance_supports_expanded_configuration() {
    configuration_case(json!({"data":vec![1e308_f64;9000]}),json!({"type":"object","properties":{"data":{"type":"array","items":{"type":"number"}}},"additionalProperties":false})).await;
}

async fn manifest(f: &fixture::JobFixture, value: &Value) {
    use sha2::{Digest, Sha256};
    let raw = value.to_string();
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_release SET manifest_bytes=$1,manifest_sha256=$2",
        vec![
            raw.clone().into(),
            hex::encode(Sha256::digest(raw.as_bytes())).into(),
        ],
    )
    .await;
}
async fn session_scope(f: &fixture::JobFixture, id: &str) -> sea_orm::DatabaseTransaction {
    use hmac::{Hmac, Mac};
    let business = f.database.role("webdock_auth_runtime").await;
    let tx = business.begin().await.unwrap();
    let raw=json!({"v":1,"aud":"webdock_auth_runtime","exp":chrono::Utc::now().timestamp()+60,"scope":{"kind":"session","authProvider":"better-auth","subject":"operator","sessionID":"session","pluginInvocationID":id}}).to_string();
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(b"owned-plugin-jobs-rls-secret-32bytes").unwrap();
    mac.update(raw.as_bytes());
    tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[raw.into(),hex::encode(mac.finalize().into_bytes()).into()])).await.unwrap();
    tx
}

#[tokio::test]
#[ignore = "owns PostgreSQL18 with restricted signed invocation scope"]
async fn final_null_capability_cannot_read_jobs_from_data_only_invocation() {
    let f = fixture::JobFixture::start().await;
    let mut descriptor = f.manifest.clone();
    descriptor["actions"].as_array_mut().unwrap().push(json!({"name":"data-only","intent":"read","audience":"operator","capabilities":["plugin.data.read"]}));
    manifest(&f, &descriptor).await;
    let job = f.enqueue("null-cap-job").await;
    let admitted = f.admit("data-only", "data-only-parent").await;
    let tx = session_scope(&f, &admitted.claims.id).await;
    let value: Option<Value> = tx
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT webdock_rls.plugin_job_get($1,NULL) AS value",
            [job["id"].as_str().unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap();
    assert!(
        value.is_none(),
        "NULL must never disable the job-read capability gate"
    );
    tx.commit().await.unwrap();
}
