//! Resource count admission against an exclusively owned, disposable PostgreSQL database.
#[allow(
    dead_code,
    reason = "Shared owned fixture also serves the broader jobs suite"
)]
#[path = "support/jobs_fixture.rs"]
mod fixture;
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use spitzli_identity::{CookiePolicy, IdentityService};
use spitzli_plugin_host::{AdmissionInput, FileBindings, HostService};
use spitzli_plugin_runtime::PeerIdentity;
use std::sync::Arc;

fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
async fn product() -> fixture::JobFixture {
    let mut f = fixture::JobFixture::start_with_connections(4).await;
    f.manifest["offerings"] =
        json!([{"id":"unit","label":"Bucket","description":"An isolated bucket."}]);
    f.manifest["selfService"]["features"][0]["offering"] = json!("unit");
    f.manifest["actions"].as_array_mut().unwrap().push(json!({
        "name":"create","intent":"write","audience":"customer","feature":"run",
        "resource":"create","offering":"unit","capabilities":["plugin.jobs.enqueue"]
    }));
    f.manifest["actions"].as_array_mut().unwrap().push(json!({
        "name":"delete","intent":"write","audience":"customer","feature":"run",
        "resource":"delete","offering":"unit","capabilities":["plugin.jobs.enqueue"],
        "input":{"type":"object","properties":{"resource_id":{"type":"string","maxLength":128}},"required":["resource_id"],"additionalProperties":false}
    }));
    let raw = f.manifest.to_string();
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
    sql(&f.owner, "DELETE FROM webdock_auth.plugin_resource", vec![]).await;
    sql(&f.owner, "INSERT INTO webdock_auth.customer_plan(customer_id,overrides,updated_by) VALUES('123','{\"job-fixture.unit\":1}','operator')", vec![]).await;
    f
}
async fn create(
    f: &fixture::JobFixture,
    project: &str,
    key: &str,
) -> spitzli_plugin_protocol::Result<spitzli_plugin_host::AdmittedInvocation> {
    f.host.admit_with_authority(&f.actor, serde_json::from_value::<AdmissionInput>(json!({
        "pluginID":"job-fixture","projectID":project,"action":"create","input":{},"idempotencyKey":key
    })).unwrap(), true, &spitzli_browser_session::StoredAuthority::local_session(&f.actor)).await
}
async fn allocated(f: &fixture::JobFixture) -> Value {
    f.owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_auth.allocated_of('123') AS value",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}
async fn resource_count(f: &fixture::JobFixture) -> i64 {
    f.owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS value FROM webdock_auth.plugin_resource",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
}

async fn postgres_fixture(f: &fixture::JobFixture) {
    use sha2::{Digest, Sha256};
    let manifest: Value =
        serde_json::from_str(include_str!("../../../plugins/postgres/manifest.json")).unwrap();
    let raw = manifest.to_string();
    sql(&f.owner, "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES('postgres','external','enabled','enabled','0.1.0','ready',1)", vec![]).await;
    sql(&f.owner, "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('postgres','0.1.0',$1,$2,$3,'https://registry.invalid','fixture')", vec![raw.clone().into(),hex::encode(Sha256::digest(raw.as_bytes())).into(),manifest["image"].as_str().unwrap().into()]).await;
    sql(&f.owner, "INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('postgres',$1,'{}')", vec![manifest["capabilities"].clone().into()]).await;
    sql(&f.owner, "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('postgres',1,'0.1.0',$1,'ready',0,$2,'fixtures','fixture-plugin')", vec![manifest["image"].as_str().unwrap().into(),"a".repeat(64).into()]).await;
    for feature in ["create-shared", "create-dedicated"] {
        sql(&f.owner, "INSERT INTO webdock_auth.plugin_feature_policy(plugin_id,feature_id,mode,updated_by) VALUES('postgres',$1,'selected','operator')", vec![feature.into()]).await;
        sql(&f.owner, "INSERT INTO webdock_auth.plugin_feature_customer(plugin_id,feature_id,customer_id,added_by) VALUES('postgres',$1,'123','operator')", vec![feature.into()]).await;
    }
    let path = f.directory.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"]["postgres"] = bindings["runtimes"]["job-fixture"].clone();
    std::fs::write(path, bindings.to_string()).unwrap();
}

#[tokio::test]
#[ignore = "owns disposable PostgreSQL18; local-process switch or Docker; no provider I/O"]
async fn retired_postgres_stale_grants_cannot_create_but_retained_work_and_maintenance_survive() {
    let f = fixture::JobFixture::start_with_connections(4).await;
    postgres_fixture(&f).await;
    let enabled = HostService::new(
        f.database
            .role_with_connections("webdock_auth_runtime", 4)
            .await,
        f.database
            .role_with_connections("webdock_worker_runtime", 4)
            .await,
        "owned-plugin-jobs-rls-secret-32bytes".into(),
        Arc::new(
            spitzli_persistence::IdAllocator::connect(
                f.database.url.as_str(),
                spitzli_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        ),
        Arc::new(FileBindings::new(f.directory.path().join("bindings.json"))),
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
    )
    .await
    .unwrap()
    .with_new_database_orders_enabled(true)
    .with_session_authority(f.auth.clone(), f.identity.clone(), None)
    .await
    .unwrap();
    let tx = f.auth.begin().await.unwrap();
    let owner = f
        .identity
        .authenticate_session_in(&tx, "member", "member-session")
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let proof = spitzli_browser_session::StoredAuthority::local_session(&owner);
    let command = |action: &str, input: Value, key: &str| {
        serde_json::from_value::<AdmissionInput>(json!({"pluginID":"postgres","projectID":"124","action":action,"input":input,"idempotencyKey":key})).unwrap()
    };
    for (action, resource, input) in [
        (
            "databases.create",
            "retained-shared",
            json!({"name":"Shared"}),
        ),
        (
            "instances.create",
            "retained-dedicated",
            json!({"name":"Dedicated","version":"18"}),
        ),
    ] {
        let accepted = enabled
            .admit_with_authority(
                &owner,
                command(action, input.clone(), resource),
                true,
                &proof,
            )
            .await
            .unwrap();
        let count = || async {
            f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT count(*) AS count FROM webdock_auth.plugin_invocation WHERE plugin_id='postgres'")).await.unwrap().unwrap().try_get::<i64>("", "count").unwrap()
        };
        let before = count().await;
        for actor in [&owner, &f.actor] {
            let denied = f
                .host
                .admit_with_authority(
                    actor,
                    command(action, input.clone(), "fresh-after-retirement"),
                    true,
                    &spitzli_browser_session::StoredAuthority::local_session(actor),
                )
                .await;
            assert_eq!(
                denied.err().unwrap().code,
                spitzli_plugin_protocol::ErrorCode::Conflict
            );
        }
        assert_eq!(
            count().await,
            before,
            "refused fresh requests leave no invocation or resource reservation"
        );
        assert!(
            f.host
                .admit_with_authority(
                    &owner,
                    command(action, input.clone(), resource),
                    true,
                    &proof
                )
                .await
                .is_err(),
            "uncertain pending work must not be redispatched"
        );
        let handle = f.host.authenticate(peer(), &accepted.token).await.unwrap();
        let output = json!({"resource":{"id":resource,"name":input["name"]}});
        f.host.record_resource(&handle, &output).await.unwrap();
        let replay = f
            .host
            .admit_with_authority(
                &owner,
                command(action, input.clone(), resource),
                true,
                &proof,
            )
            .await
            .unwrap();
        assert_eq!(replay.replay_result, Some(output));
        let mut changed = input.clone();
        changed["name"] = json!("Changed");
        assert!(
            f.host
                .admit_with_authority(&owner, command(action, changed, resource), true, &proof)
                .await
                .is_err(),
            "retirement cannot relax existing idempotency validation"
        );
    }
    for (action, resource) in [
        ("databases.view", "retained-shared"),
        ("databases.credentials", "retained-shared"),
        ("databases.rotate", "retained-shared"),
        ("databases.delete", "retained-shared"),
        ("instances.delete", "retained-dedicated"),
    ] {
        assert!(
            f.host
                .admit_with_authority(
                    &owner,
                    command(action, json!({"resource_id":resource}), action),
                    true,
                    &proof
                )
                .await
                .is_ok(),
            "retirement denied maintenance {action}"
        );
        assert!(
            f.host
                .admit_with_authority(
                    &owner,
                    command(action, json!({"resource_id":"foreign-resource"}), "foreign"),
                    true,
                    &proof
                )
                .await
                .is_err(),
            "maintenance still requires resource ownership"
        );
    }
    let views = f
        .host
        .project_plugins_with_authority(&owner, "124", &proof)
        .await
        .unwrap();
    let view = views
        .iter()
        .find(|view| view.manifest["name"] == "postgres")
        .unwrap();
    let projected = serde_json::to_value(view).unwrap();
    assert_eq!(
        projected["blockedCreateActions"],
        json!(["databases.create", "instances.create"])
    );
    assert_eq!(view.features, ["create-shared", "create-dedicated"]);
    assert_eq!(view.resources.len(), 2);
    let grants = f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT count(*) AS count FROM webdock_auth.plugin_feature_customer WHERE plugin_id='postgres' AND customer_id='123'")).await.unwrap().unwrap().try_get::<i64>("", "count").unwrap();
    assert_eq!(grants, 2, "retirement must not delete retained grants");
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn concurrent_last_slot_creates_count_before_either_plugin_returns() {
    let f = product().await;
    sql(
        &f.owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('126','Second','123')",
        vec![],
    )
    .await;
    // Both requests finish admission before either has a plugin response or resource index row.
    let (a, b) = tokio::join!(
        create(&f, "124", "last-slot-a"),
        create(&f, "126", "last-slot-b")
    );
    assert_eq!(
        a.as_ref().err().or(b.as_ref().err()).unwrap().code,
        spitzli_plugin_protocol::ErrorCode::Conflict
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "exactly one last-slot create may reach the plugin; errors: {:?}, {:?}",
        a.as_ref().err(),
        b.as_ref().err()
    );
    let admitted = a.or(b).unwrap();
    let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
    f.host
        .record_resource(
            &handle,
            &json!({"resource":{"id":"bucket","name":"Bucket"}}),
        )
        .await
        .unwrap();
    assert!(create(&f, "124", "third-slot").await.is_err());
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn idempotent_create_reuses_one_slot_and_can_only_index_one_exact_resource() {
    let f = product().await;
    let (first, retry) = tokio::join!(
        create(&f, "124", "same-create"),
        create(&f, "124", "same-create")
    );
    assert_eq!(
        usize::from(first.is_ok()) + usize::from(retry.is_ok()),
        1,
        "a pending retry must not receive another dispatch token"
    );
    let first = first.or(retry).unwrap();
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
    let a = f.host.authenticate(peer(), &first.token).await.unwrap();
    let output = json!({"resource":{"id":"bucket","name":"Bucket"}});
    f.host.record_resource(&a, &output).await.unwrap();
    let retry = create(&f, "124", "same-create").await.unwrap();
    assert_eq!(retry.replay_result, Some(output.clone()));
    assert_eq!(
        allocated(&f).await["job-fixture.unit"],
        1,
        "index replaces the reservation atomically"
    );
    assert!(
        f.host
            .record_resource(&a, &json!({"resource":{"id":"second","name":"Other"}}))
            .await
            .is_err()
    );
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.plugin_resource WHERE resource_id='bucket'",
        vec![],
    )
    .await;
    assert!(
        f.host.record_resource(&a, &output).await.is_err(),
        "a late replay must not resurrect a deleted resource"
    );
    assert!(
        create(&f, "124", "same-create").await.is_err(),
        "a settled and deleted create cannot reach the runtime again"
    );
    assert!(allocated(&f).await["job-fixture.unit"].is_null());
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn uncertain_create_survives_revocation_expiry_and_runtime_promotion() {
    let f = product().await;
    let first = create(&f, "124", "uncertain-create").await.unwrap();
    let handle = f.host.authenticate(peer(), &first.token).await.unwrap();
    assert!(f.host.record_resource(&handle, &json!({})).await.is_err());
    f.host.revoke_issued(&first.token).await.unwrap();
    assert!(
        f.host
            .record_resource(
                &handle,
                &json!({"resource":{"id":"revoked","name":"Revoked"}})
            )
            .await
            .is_err()
    );
    sql(&f.owner, "UPDATE webdock_auth.plugin_invocation SET created_at=now()-interval '2 minutes',expires_at=now()-interval '1 minute'", vec![]).await;
    sql(&f.owner, "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) SELECT plugin_id,2,version,image,status,configuration_revision,credential_hash,namespace,service_name FROM webdock_auth.plugin_runtime WHERE generation=1", vec![]).await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_installation SET current_generation=2",
        vec![],
    )
    .await;
    let path = f.directory.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bindings["runtimes"]["job-fixture"]["generation"] = json!(2);
    std::fs::write(path, bindings.to_string()).unwrap();
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
    assert!(create(&f, "124", "new-create").await.is_err());
    assert!(
        create(&f, "124", "uncertain-create").await.is_err(),
        "runtime promotion cannot make an uncertain create safe to repeat"
    );
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
    // The same key in another project is a different operation, under the same customer budget.
    sql(
        &f.owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('126','Second','123')",
        vec![],
    )
    .await;
    assert!(create(&f, "126", "uncertain-create").await.is_err());
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.session WHERE id='session'",
        vec![],
    )
    .await;
    assert_eq!(
        create(&f, "124", "uncertain-create")
            .await
            .err()
            .unwrap()
            .code,
        spitzli_plugin_protocol::ErrorCode::PermissionDenied
    );
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn create_job_owns_the_slot_and_completion_indexes_before_releasing_it() {
    let f = product().await;
    let admitted = create(&f, "124", "job-create").await.unwrap();
    let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
    let job = f
        .host
        .jobs_enqueue(
            &handle,
            json!({"name":"work","payload":{},"idempotencyKey":"create-job-one"}),
        )
        .await
        .unwrap();
    assert!(
        f.host
            .jobs_enqueue(
                &handle,
                json!({"name":"work","payload":{},"idempotencyKey":"create-job-two"})
            )
            .await
            .is_err(),
        "a create cannot fan out one reserved slot to multiple jobs"
    );
    f.host.record_resource(&handle, &job).await.unwrap();
    f.host.revoke_issued(&admitted.token).await.unwrap();
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host
        .complete_job(
            &lease,
            Ok(json!({"resource":{"id":"job-bucket","name":"Job bucket"}})),
        )
        .await
        .unwrap();
    let resource: Value = f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,
        "SELECT to_jsonb(r) AS value FROM webdock_auth.plugin_resource r WHERE resource_id='job-bucket'"))
        .await.unwrap().expect("successful job must enter the Core resource index").try_get("", "value").unwrap();
    assert_eq!(resource["customer_id"], "123");
    assert_eq!(resource["project_id"], "124");
    assert_eq!(resource["offering_id"], "unit");
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
    assert!(create(&f, "124", "job-took-last-slot").await.is_err());
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn terminal_job_errors_and_invalid_results_do_not_prove_no_external_resource() {
    use spitzli_plugin_protocol::{ErrorCode, PluginError};
    for result in [
        Err(PluginError::new(ErrorCode::Unavailable, "Lost response")),
        Ok(json!({})),
    ] {
        let f = product().await;
        let admitted = create(&f, "124", "unknown-job-create").await.unwrap();
        let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
        f.host
            .jobs_enqueue(
                &handle,
                json!({"name":"work","payload":{},"idempotencyKey":"unknown-job-one","attempts":1}),
            )
            .await
            .unwrap();
        let lease = f.host.claim_job(None).await.unwrap().unwrap();
        f.host.complete_job(&lease, result).await.unwrap();
        assert_eq!(resource_count(&f).await, 0);
        assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
        assert!(create(&f, "124", "after-unknown-job").await.is_err());
        assert!(
            f.host.claim_job(None).await.unwrap().is_none(),
            "the job is terminal, but its external effect is uncertain"
        );
    }
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn expired_create_job_is_not_reclaimed_and_stale_completion_cannot_free_capacity() {
    let f = product().await;
    let admitted = create(&f, "124", "lease-job-create").await.unwrap();
    let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
    f.host
        .jobs_enqueue(
            &handle,
            json!({"name":"work","payload":{},"idempotencyKey":"lease-job-one"}),
        )
        .await
        .unwrap();
    let stale = f.host.claim_job(None).await.unwrap().unwrap();
    sql(&f.owner, "UPDATE webdock_auth.plugin_job_execution SET lease_until=clock_timestamp()-interval '1 second'", vec![]).await;
    assert!(
        f.host.claim_job(None).await.unwrap().is_none(),
        "an expired create lease is an uncertain external outcome, not permission to retry"
    );
    f.host
        .complete_job(
            &stale,
            Ok(json!({"resource":{"id":"stale","name":"Stale"}})),
        )
        .await
        .unwrap();
    assert_eq!(resource_count(&f).await, 0);
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn existing_resource_ids_cannot_settle_another_scopes_reservation() {
    let f = product().await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.customer_plan SET overrides='{\"job-fixture.unit\":2}'",
        vec![],
    )
    .await;
    sql(
        &f.owner,
        "INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('126','Second','123')",
        vec![],
    )
    .await;
    sql(&f.owner, "INSERT INTO webdock_auth.plugin_resource(plugin_id,resource_id,offering_id,customer_id,project_id,name,created_via,created_by) VALUES('job-fixture','existing','unit','123','126','Existing','operator','operator')", vec![]).await;
    let admitted = create(&f, "124", "scoped-create").await.unwrap();
    let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
    assert!(
        f.host
            .record_resource(
                &handle,
                &json!({"resource":{"id":"existing","name":"Stolen"}})
            )
            .await
            .is_err()
    );
    assert_eq!(
        allocated(&f).await["job-fixture.unit"],
        2,
        "a conflict must retain the uncertain slot"
    );
    f.host.record_resource(&handle, &json!({"resource":{"id":"mine","name":"Mine"},"customer_id":"other","project_id":"126","offering":"other"})).await.unwrap();
    assert_eq!(allocated(&f).await["job-fixture.unit"], 2);
    assert_eq!(resource_count(&f).await, 2);
}

#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn queued_delete_keeps_the_resource_count_until_the_job_proves_deletion() {
    let f = product().await;
    let admitted = create(&f, "124", "delete-target").await.unwrap();
    let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
    f.host
        .record_resource(
            &handle,
            &json!({"resource":{"id":"bucket","name":"Bucket"}}),
        )
        .await
        .unwrap();
    let deleting = f.host.admit_with_authority(&f.actor, serde_json::from_value::<AdmissionInput>(json!({
        "pluginID":"job-fixture","projectID":"124","action":"delete","input":{"resource_id":"bucket"},"idempotencyKey":"delete-bucket"
    })).unwrap(), true, &spitzli_browser_session::StoredAuthority::local_session(&f.actor)).await.unwrap();
    let handle = f.host.authenticate(peer(), &deleting.token).await.unwrap();
    let concurrent_handle = f.host.authenticate(peer(), &deleting.token).await.unwrap();
    let budget = f.owner.begin().await.unwrap();
    budget
        .execute_unprepared("SELECT pg_advisory_xact_lock(hashtext('webdock-limits:123'))")
        .await
        .unwrap();
    let pid: i32 = budget
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    let host = f.host.clone();
    let completing =
        tokio::spawn(async move { host.record_resource(&concurrent_handle, &json!({})).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))) AS blocked", [pid.into()])).await.unwrap().unwrap().try_get("", "blocked").unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("synchronous delete is waiting at the customer budget lock");
    let job = f
        .host
        .jobs_enqueue(
            &handle,
            json!({"name":"work","payload":{},"idempotencyKey":"delete-job-one"}),
        )
        .await
        .unwrap();
    budget.commit().await.unwrap();
    assert!(
        completing.await.unwrap().is_err(),
        "settlement must see a delegation committed while waiting for its budget lock"
    );
    f.host.record_resource(&handle, &job).await.unwrap();
    assert_eq!(
        resource_count(&f).await,
        1,
        "an enqueue acknowledgement is not a completed delete"
    );
    assert!(
        f.host.record_resource(&handle, &json!({})).await.is_err(),
        "a delegated delete cannot also settle synchronously"
    );
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
    assert!(create(&f, "124", "before-delete-completes").await.is_err());
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host.complete_job(&lease, Ok(json!({}))).await.unwrap();
    assert_eq!(resource_count(&f).await, 0);
    assert!(allocated(&f).await["job-fixture.unit"].is_null());
    create(&f, "124", "after-delete-completes").await.unwrap();
}

// This handler deliberately allocates a fresh UUID on every call, like PostgreSQL create_shared.
// Its external ledger is independent of Core's resource index; index rejection cannot undo it.
#[derive(Clone, Copy)]
enum CreateMode {
    Sync,
    Lost,
    Job,
    JobLost,
}
struct NonIdempotentCreate {
    effects: sea_orm::DatabaseConnection,
    host: spitzli_plugin_runtime::HostClient,
    mode: CreateMode,
}
impl NonIdempotentCreate {
    async fn effect(&self) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        sql(
            &self.effects,
            "INSERT INTO external_effects(id) VALUES($1)",
            vec![id.clone().into()],
        )
        .await;
        json!({"resource":{"id":id,"name":"Fresh resource"},"credential":"PRIVATE result must not be cached"})
    }
}
#[tonic::async_trait]
impl spitzli_plugin_runtime::ActionHandler for NonIdempotentCreate {
    async fn invoke(
        &self,
        _: &spitzli_plugin_protocol::InvocationClaims,
        token: &str,
        _: Value,
        _: Value,
    ) -> spitzli_plugin_protocol::Result<Value> {
        if matches!(self.mode, CreateMode::Job | CreateMode::JobLost) {
            return self
                .host
                .call(
                    token,
                    "jobs",
                    "enqueue",
                    json!({"name":"work","payload":{},"idempotencyKey":"non-idempotent-job"}),
                )
                .await;
        }
        let output = self.effect().await;
        if matches!(self.mode, CreateMode::Lost) {
            return Err(spitzli_plugin_protocol::PluginError::new(
                spitzli_plugin_protocol::ErrorCode::Unavailable,
                "Lost external response",
            ));
        }
        Ok(output)
    }
}
#[tonic::async_trait]
impl spitzli_plugin_runtime::JobHandler for NonIdempotentCreate {
    async fn run(
        &self,
        _: &spitzli_plugin_protocol::InvocationClaims,
        _: &str,
        _: Value,
        _: Value,
    ) -> spitzli_plugin_protocol::Result<Value> {
        let output = self.effect().await;
        if matches!(self.mode, CreateMode::JobLost) {
            return Err(spitzli_plugin_protocol::PluginError::new(
                spitzli_plugin_protocol::ErrorCode::Unavailable,
                "Lost external response",
            ));
        }
        Ok(output)
    }
}
struct RunningCreate {
    endpoints: spitzli_control_plane::plugins::PluginEndpoints,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for RunningCreate {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
async fn runtime(f: &fixture::JobFixture, mode: CreateMode) -> RunningCreate {
    use spitzli_plugin_runtime::{CorePeer, HostClient, RemoteAuthority, Runtime, UnixSocket};
    sql(
        &f.owner,
        "CREATE TABLE external_effects(id text PRIMARY KEY)",
        vec![],
    )
    .await;
    let host_path = f.directory.path().join("host.sock");
    let mut socket = UnixSocket::bind(&host_path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = f.host.clone().rpc();
    let host_task = tokio::spawn(async move {
        let _socket = socket;
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let host = HostClient::connect_unix(&host_path).await.unwrap();
    let handler = Arc::new(NonIdempotentCreate {
        effects: f.owner.clone(),
        host: host.clone(),
        mode,
    });
    let path = f.directory.path().join("runtime.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let runtime = Runtime::new(
        spitzli_plugin_protocol::Manifest::parse(&serde_json::to_vec(&f.manifest).unwrap())
            .unwrap(),
        1,
        ed25519_dalek::SigningKey::from_bytes(&[19; 32]).verifying_key(),
        Arc::new(
            RemoteAuthority::new(
                host,
                "job-fixture".into(),
                1,
                vec![CorePeer::Unix {
                    uid: rustix::process::geteuid().as_raw(),
                }],
            )
            .unwrap(),
        ),
        handler.clone(),
    )
    .unwrap()
    .with_jobs(handler);
    let runtime_task = tokio::spawn(async move {
        runtime
            .serve_unix(socket, std::future::pending())
            .await
            .unwrap();
    });
    let connections = f.directory.path().join("connections.json");
    std::fs::write(&connections,json!({"runtimes":{"job-fixture":{"generation":1,"transport":{"kind":"unix","path":path}}}}).to_string()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&connections, std::fs::Permissions::from_mode(0o600)).unwrap();
    RunningCreate {
        endpoints: spitzli_control_plane::plugins::PluginEndpoints::new(
            f.host.clone(),
            spitzli_control_plane::plugin_config::RuntimeConnections::new(connections),
            None,
        ),
        tasks: vec![host_task, runtime_task],
    }
}
async fn invoke_create(
    f: &fixture::JobFixture,
    r: &RunningCreate,
) -> spitzli_plugin_protocol::Result<Value> {
    let tx = f.auth.begin().await.unwrap();
    let principal = f
        .identity
        .authenticate_session_in(&tx, "operator", "session")
        .await
        .unwrap()
        .ok_or_else(spitzli_plugin_protocol::PluginError::denied)?;
    tx.commit().await.unwrap();
    r.endpoints.invoke(&spitzli_control_plane::auth::AuthenticatedActor { principal,authority:spitzli_browser_session::StoredAuthority::local_session(&f.actor) },
        serde_json::from_value(json!({"pluginID":"job-fixture","projectID":"124","action":"create","input":{},"idempotencyKey":"external-create"})).unwrap()).await
}
async fn external_count(f: &fixture::JobFixture) -> i64 {
    f.owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM external_effects",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
#[tokio::test]
#[ignore = "owns PostgreSQL18 and native Host/runtime RPC"]
async fn lost_completed_response_replays_public_metadata_without_a_second_external_create() {
    let f = product().await;
    let r = runtime(&f, CreateMode::Sync).await;
    let first = invoke_create(&f, &r).await.unwrap();
    let replay = invoke_create(&f, &r).await;
    assert_eq!(
        external_count(&f).await,
        1,
        "retry must never execute a non-idempotent create twice"
    );
    assert_eq!(replay.unwrap(), json!({"resource":first["resource"]}));
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_grant SET revoked_at=now()",
        vec![],
    )
    .await;
    assert!(
        invoke_create(&f, &r).await.is_err(),
        "cached results require a current grant"
    );
    assert_eq!(external_count(&f).await, 1);
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_grant SET revoked_at=NULL",
        vec![],
    )
    .await;
    sql(
        &f.owner,
        "DELETE FROM webdock_auth.session WHERE id='session'",
        vec![],
    )
    .await;
    assert!(
        create(&f, "124", "external-create").await.is_err(),
        "even a retained principal must revalidate the session before receiving cached metadata"
    );
}
#[tokio::test]
#[ignore = "owns PostgreSQL18 and native Host/runtime RPC"]
async fn lost_external_response_conflicts_without_redispatch_or_freeing_capacity() {
    let f = product().await;
    let r = runtime(&f, CreateMode::Lost).await;
    assert!(invoke_create(&f, &r).await.is_err());
    assert!(invoke_create(&f, &r).await.is_err());
    assert_eq!(external_count(&f).await, 1);
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
}
#[tokio::test]
#[ignore = "owns PostgreSQL18 and native Host/runtime RPC"]
async fn uncertain_create_job_is_never_redispatched() {
    let f = product().await;
    let r = runtime(&f, CreateMode::JobLost).await;
    let ack = invoke_create(&f, &r).await.unwrap();
    assert!(r.endpoints.advance_job().await.unwrap());
    sql(
        &f.owner,
        "UPDATE webdock_auth.plugin_job_execution SET available_at=clock_timestamp()",
        vec![],
    )
    .await;
    r.endpoints.advance_job().await.unwrap();
    assert_eq!(
        external_count(&f).await,
        1,
        "a lost create-job response must not trigger another external create"
    );
    assert_eq!(invoke_create(&f, &r).await.unwrap(), ack);
    assert_eq!(allocated(&f).await["job-fixture.unit"], 1);
}

async fn delete(f: &fixture::JobFixture) -> spitzli_plugin_host::AdmittedInvocation {
    f.host.admit_with_authority(&f.actor,serde_json::from_value(json!({"pluginID":"job-fixture","projectID":"124","action":"delete","input":{"resource_id":"bucket"},"idempotencyKey":"delete-bucket"})).unwrap(),true,&spitzli_browser_session::StoredAuthority::local_session(&f.actor)).await.unwrap()
}
#[tokio::test]
#[ignore = "owns a disposable PostgreSQL18 container"]
async fn regression_delete_fanout_and_new_invocation_cannot_delegate_a_second_job() {
    let f = product().await;
    let target = create(&f, "124", "fanout-delete-target").await.unwrap();
    let handle = f.host.authenticate(peer(), &target.token).await.unwrap();
    f.host
        .record_resource(
            &handle,
            &json!({"resource":{"id":"bucket","name":"Bucket"}}),
        )
        .await
        .unwrap();
    let deleting = delete(&f).await;
    let handle = f.host.authenticate(peer(), &deleting.token).await.unwrap();
    let first =
        json!({"name":"work","payload":{"bookkeeping":true},"idempotencyKey":"delete-bookkeeping"});
    let ack = f.host.jobs_enqueue(&handle, first.clone()).await.unwrap();
    let second = json!({"name":"work","payload":{"actualDelete":true},"idempotencyKey":"delete-actual-work"});
    assert!(
        f.host.jobs_enqueue(&handle, second.clone()).await.is_err(),
        "reject delete fan-out before a bookkeeping job can release capacity"
    );
    assert!(
        f.host.claim_job(None).await.unwrap().is_none(),
        "a rejected fan-out must not leave its first bookkeeping job free to settle deletion"
    );
    assert_eq!(resource_count(&f).await, 1);
    f.host.record_resource(&handle, &ack).await.unwrap();
    let retry = delete(&f).await;
    let retry = f.host.authenticate(peer(), &retry.token).await.unwrap();
    assert_eq!(f.host.jobs_enqueue(&retry, first).await.unwrap(), ack);
    assert!(
        f.host.jobs_enqueue(&retry, second).await.is_err(),
        "one operation cannot acquire another deletion job via a new invocation"
    );
    assert_eq!(resource_count(&f).await, 1);
    let stale = f.host.claim_job(None).await.unwrap().unwrap();
    sql(&f.owner,"UPDATE webdock_auth.plugin_job_execution SET lease_until=clock_timestamp()-interval '1 second'",vec![]).await;
    let current = f.host.claim_job(None).await.unwrap().unwrap();
    f.host.complete_job(&stale, Ok(json!({}))).await.unwrap();
    assert_eq!(
        resource_count(&f).await,
        1,
        "stale deletion attempt cannot free count"
    );
    f.host.complete_job(&current, Ok(json!({}))).await.unwrap();
    assert_eq!(resource_count(&f).await, 0);
}
#[tokio::test]
#[ignore = "owns PostgreSQL18 and native Host/runtime RPC"]
async fn regression_completed_create_job_reenqueues_exact_ack_without_dispatch() {
    let f = product().await;
    let admitted = create(&f, "124", "completed-job-ack").await.unwrap();
    let handle = f.host.authenticate(peer(), &admitted.token).await.unwrap();
    let request = json!({"name":"work","payload":{},"idempotencyKey":"completed-job-ack"});
    let ack = f.host.jobs_enqueue(&handle, request.clone()).await.unwrap();
    let lease = f.host.claim_job(None).await.unwrap().unwrap();
    f.host.complete_job(&lease,Ok(json!({"resource":{"id":"completed","name":"Completed"},"credential":"PRIVATE job result"}))).await.unwrap();
    assert_eq!(
        f.host.jobs_enqueue(&handle, request.clone()).await.unwrap(),
        ack,
        "an indexed reservation still returns its exact existing job"
    );
    let mut changed = request;
    changed["payload"] = json!({"different":true});
    assert!(f.host.jobs_enqueue(&handle, changed).await.is_err());
    let r = runtime(&f, CreateMode::Job).await;
    // Use a different operation for a complete real dispatcher -> runtime -> job -> replay round.
    sql(
        &f.owner,
        "UPDATE webdock_auth.customer_plan SET overrides='{\"job-fixture.unit\":2}'",
        vec![],
    )
    .await;
    let real_ack = invoke_create(&f, &r).await.unwrap();
    assert!(r.endpoints.advance_job().await.unwrap());
    assert_eq!(invoke_create(&f, &r).await.unwrap(), real_ack);
    assert_eq!(external_count(&f).await, 1);
    let results: Value = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT jsonb_agg(result) AS value FROM webdock_auth.plugin_job_execution",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap();
    assert!(
        !results.to_string().contains("PRIVATE"),
        "create results retain only public resource metadata"
    );
}

#[tokio::test]
#[ignore = "owns PostgreSQL18, OAuth and native Host/runtime RPC"]
async fn mcp_completed_create_replays_once_and_revoked_access_denies_cached_metadata() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use spitzli_oauth::{
        AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientAuthentication,
        ClientPurpose, ClientRegistration, CodeExchange, OAuthConfig, OAuthService,
    };
    let mut f = product().await;
    let ids = Arc::new(
        spitzli_persistence::IdAllocator::connect(
            f.database.url.as_str(),
            spitzli_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let keys = spitzli_identity::SecretKeys::new(
        std::collections::BTreeMap::from([(0, "owned-mcp-resource-replay-key-32bytes".into())]),
        0,
        None,
    )
    .unwrap();
    let oauth = Arc::new(OAuthService::new(
        f.auth.clone(),
        f.identity.clone(),
        ids.clone(),
        Arc::new(keys),
        OAuthConfig::new(
            "http://localhost:3121/api/auth",
            "http://localhost:3120/api/mcp",
            None,
        )
        .unwrap(),
    ));
    oauth.provision_signing_key(&f.owner).await.unwrap();
    let client = oauth
        .provision_client(
            &f.owner,
            ClientRegistration {
                name: "Owned resource replay".into(),
                redirect_uris: vec!["https://client.invalid/callback".into()],
                post_logout_redirect_uris: vec![],
                scopes: vec![
                    "openid".into(),
                    "webdock:read".into(),
                    "webdock:write".into(),
                ],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: false,
                purpose: ClientPurpose::McpOperator,
            },
        )
        .await
        .unwrap();
    let credentials = ClientAuthentication {
        client_id: client.client_id.clone(),
        client_secret: client.client_secret,
        method: ClientAuthMethod::ClientSecretPost,
    };
    let policy = CookiePolicy::new(false, "owned-plugin-jobs-cookie-key-32bytes").unwrap();
    let cookie = format!("{}={}", policy.name(), policy.sign("owned-token").unwrap());
    let verifier = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-._~";
    let mut request = AuthorizationRequest::code(
        client.client_id,
        "https://client.invalid/callback".into(),
        "openid webdock:read webdock:write".into(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
    );
    request.resources = vec!["http://localhost:3120/api/mcp".into()];
    let authorization = oauth.authorize(&cookie, request).await.unwrap();
    let authorization = match authorization {
        AuthorizationOutcome::ConsentRequired(consent) => oauth
            .consent(&cookie, &consent.request_id, true)
            .await
            .unwrap(),
        value => value,
    };
    let AuthorizationOutcome::Redirect(location) = authorization else {
        panic!("owned MCP authorization must redirect")
    };
    let code = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    let token = oauth
        .exchange_code(CodeExchange {
            client: ClientAuthentication {
                client_id: credentials.client_id.clone(),
                client_secret: credentials.client_secret.clone(),
                method: ClientAuthMethod::ClientSecretPost,
            },
            code,
            redirect_uri: "https://client.invalid/callback".into(),
            code_verifier: verifier.into(),
        })
        .await
        .unwrap();
    let actor = oauth
        .authorize_mcp_reference(&credentials, &token.access_token, "webdock:write")
        .await
        .unwrap();
    f.host = Arc::new(
        HostService::new(
            f.database
                .role_with_connections("webdock_auth_runtime", 4)
                .await,
            f.database
                .role_with_connections("webdock_worker_runtime", 4)
                .await,
            "owned-plugin-jobs-rls-secret-32bytes".into(),
            ids,
            Arc::new(FileBindings::new(f.directory.path().join("bindings.json"))),
            ed25519_dalek::SigningKey::from_bytes(&[19; 32]),
        )
        .await
        .unwrap()
        .with_session_authority(f.auth.clone(), f.identity.clone(), None)
        .await
        .unwrap()
        .with_mcp_authority(oauth.clone())
        .unwrap(),
    );
    let r = runtime(&f, CreateMode::Sync).await;
    let input = || {
        serde_json::from_value(json!({"pluginID":"job-fixture","projectID":"124","action":"create","input":{},"idempotencyKey":"mcp-resource-create"})).unwrap()
    };
    let first = r.endpoints.invoke_mcp(&actor, input()).await.unwrap();
    let replay = r.endpoints.invoke_mcp(&actor, input()).await.unwrap();
    assert_eq!(external_count(&f).await, 1);
    assert_eq!(replay, json!({"resource":first["resource"]}));
    oauth
        .revoke(&credentials, &token.access_token)
        .await
        .unwrap();
    assert!(
        r.endpoints.invoke_mcp(&actor, input()).await.is_err(),
        "stale MCP authentication cannot return retained metadata"
    );
    assert_eq!(external_count(&f).await, 1);
}
