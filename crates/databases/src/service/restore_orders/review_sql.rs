//! Explicit SQL primitives only. All tests are ignored until Root opens the
//! copied disposable-PG window. No source/Ready/session/backup rows are created.
use super::*;
use sea_orm::{ConnectOptions, Database, DbErr};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    worker_url: String,
    business_url: String,
    owner_url: String,
    rls_secret: String,
    baseline: bool,
}
fn input() -> Input {
    let path = std::env::var("WEBDOCK_PG_REVIEW_COMPONENT_INPUT")
        .expect("owned SQL component input required");
    assert!(path.contains("/.runtime-test/pg-restore-review1-"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    serde_json::from_slice(&std::fs::read(path).unwrap())
        .unwrap_or_else(|_| panic!("private component shape"))
}
async fn connect(raw: &str) -> DatabaseConnection {
    let url = url::Url::parse(raw).unwrap_or_else(|_| panic!("component URL"));
    assert!(["postgres", "postgresql"].contains(&url.scheme()));
    assert!([Some("127.0.0.1"), Some("localhost")].contains(&url.host_str()));
    assert_eq!(url.path(), "/webdock_rls_test");
    let mut options = ConnectOptions::new(raw);
    options
        .max_connections(1)
        .min_connections(1)
        .sqlx_logging(false);
    Database::connect(options)
        .await
        .unwrap_or_else(|_| panic!("component connection"))
}
async fn value(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) -> Value {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .unwrap_or_else(|_| panic!("component query"))
        .expect("component row");
    decoded_json(row.try_get::<Option<Value>>("", "value"))
}
fn decoded_json(decoded: std::result::Result<Option<Value>, DbErr>) -> Value {
    decoded.expect("component value").unwrap_or(Value::Null)
}
#[test]
fn json_null_and_nonnull_decode_boundary() {
    assert_eq!(decoded_json(Ok(None)), Value::Null);
    for value in [Value::Null, json!(false), json!({"field":"fixture-value"})] {
        assert_eq!(decoded_json(Ok(Some(value.clone()))), value);
    }
}
#[test]
fn json_decode_errors_still_reject() {
    assert!(
        std::panic::catch_unwind(|| decoded_json(Err(DbErr::Type(
            "fixture incompatible column type".into()
        ))))
        .is_err()
    );
}
async fn run(db: &impl ConnectionTrait, sql: &str) {
    assert!(
        db.execute_unprepared(sql).await.is_ok(),
        "component statement"
    );
}
fn error_code(error: &DbErr) -> Option<String> {
    match error {
        DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) => e
            .as_database_error()
            .and_then(|e| e.code())
            .map(|s| s.into_owned()),
        _ => None,
    }
}
fn error_message(error: &DbErr, expected: &str) -> bool {
    match error {
        DbErr::Query(sea_orm::RuntimeErr::SqlxError(e))
        | DbErr::Exec(sea_orm::RuntimeErr::SqlxError(e)) => e
            .as_database_error()
            .is_some_and(|e| e.message() == expected),
        _ => false,
    }
}
async fn denied(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
    state: &str,
    message: Option<&str>,
) {
    run(tx, "SAVEPOINT error_probe").await;
    let result = tx
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await;
    assert!(result.is_err(), "negative primitive must deny");
    let error = result.err().unwrap();
    assert_eq!(error_code(&error).as_deref(), Some(state));
    if let Some(message) = message {
        if !error_message(&error, message) {
            let category = [
                "Actual early database epoch witness required",
                "Current native operator required",
                "Fresh unissued direct Restore required",
                "Exact admitted database restore scope required",
            ]
            .into_iter()
            .find(|s| error_message(&error, s))
            .unwrap_or("other bounded SQL error");
            eprintln!("observed negative SQL boundary: {category}");
        }
        assert!(error_message(&error, message), "exact primitive boundary");
    }
    run(tx, "ROLLBACK TO SAVEPOINT error_probe").await;
    run(tx, "RELEASE SAVEPOINT error_probe").await;
}
fn scope(epoch: &Value) -> Value {
    // Deliberately invalid actor/reference: every Source/audit call is NEGATIVE.
    // No Principal/session/grant is synthesized to get a domain success.
    json!({"kind":"system","purpose":"database-order","clusterID":"991","customerID":"992","projectID":"993",
        "subject":"absent-negative-component-actor","sessionID":"absent-negative-component-session","operator":true,"canWrite":true,"permission":"org:databases:manage",
        "restoreOfferID":"994","restoreIntentHash":"a".repeat(64),"nativeEpoch":epoch,"nativeAuthority":{"kind":"first-party-browser"}})
}
fn signed(secret: &str, scope: &Value) -> (String, String) {
    let raw=json!({"v":1,"aud":"webdock_worker_runtime","exp":chrono::Utc::now().timestamp()+110,"scope":scope}).to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(raw.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    (raw, sig)
}
async fn install(tx: &DatabaseTransaction, raw: &str, sig: &str) {
    assert!(tx.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",[raw.into(),sig.into()])).await.is_ok());
}
async fn source_denied(tx: &DatabaseTransaction, message: &str) {
    denied(
        tx,
        "SELECT webdock_rls.database_restore_source($1,$2)",
        vec!["994".into(), "a".repeat(64).into()],
        "42501",
        Some(message),
    )
    .await;
}
#[tokio::test]
#[ignore = "Root-coordinated copied SQL primitive window only"]
async fn component_timestamp_roundtrip() {
    let i = input();
    let db = connect(&i.worker_url).await;
    retained_proof_precision_cases(&db, i.baseline).await;
    let nano = chrono::DateTime::parse_from_rfc3339("2026-10-25T12:00:00.123456789Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let expiry = if i.baseline {
        nano + chrono::Duration::minutes(30)
    } else {
        review_expiry(nano)
    };
    // Real SeaORM/SQLx typed timestamp binding, not PostgreSQL text-cast rounding.
    let stored = value(
        &db,
        "SELECT to_jsonb($1::timestamptz) AS value",
        vec![expiry.into()],
    )
    .await;
    let stored = chrono::DateTime::parse_from_rfc3339(stored.as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert_eq!(
        stored, expiry,
        "reviewed expiry must survive actual PostgreSQL typed storage precision"
    );
    db.close().await.unwrap();
}
async fn retained_proof_precision_cases(db: &DatabaseConnection, baseline: bool) {
    let expression = if baseline {
        "(backup.proof->>'completedAt')::timestamptz"
    } else {
        retained_proof_time_expression()
    };
    let query = format!(
        "WITH backup(proof) AS(VALUES($1::jsonb)) SELECT jsonb_build_object('matches',({expression})=$2::timestamptz,'mismatch',({expression}) IS DISTINCT FROM ($2::timestamptz+interval '1 microsecond'),'proof',proof) AS value FROM backup"
    );
    let cases = [
        "2026-10-25T12:00:00.123456789Z",
        "2026-10-25T12:00:00.999999999Z",
        "2026-10-25T12:00:00.000000999Z",
        "2026-10-25T14:00:00.123456789+02:00",
        "2026-10-25T14:00:00.999999999+02:00",
        "2026-10-25T14:00:00.000000999+02:00",
        "2026-10-25T12:00:00.123456Z",
        "2026-10-25T14:00:00.123456+02:00",
        "2026-10-25T12:00:00Z",
        "2026-10-25T14:00:00+02:00",
    ];
    for text in cases {
        let original = chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let proof = json!({"completedAt":text});
        let observed = value(db, &query, vec![proof.clone().into(), original.into()]).await;
        let expect_exact = !baseline || original.timestamp_subsec_nanos() % 1000 == 0;
        assert_eq!(
            observed["matches"], expect_exact,
            "retained proof time must match typed persistence after truncation only"
        );
        assert_eq!(
            observed["mismatch"], expect_exact,
            "one-microsecond foreign point must not pass exact comparison"
        );
        assert_eq!(
            observed["proof"], proof,
            "projection must not rewrite retained JSON time"
        );
    }
    eprintln!(
        "precision checkpoint: ten UTC/offset/edge proof projections and foreign-point comparisons verified; retained JSON unchanged"
    );
}
pub(super) fn retained_proof_time_expression() -> &'static str {
    let sql = include_str!(
        "../../../../../docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql"
    );
    let suffix = " IS DISTINCT FROM backup.completed_at";
    let lines = sql
        .lines()
        .filter(|line| line.contains("backup.proof->>'completedAt'") && line.ends_with(suffix))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "exact retained-proof comparison must stay unique"
    );
    lines[0]
        .trim()
        .strip_prefix("OR ")
        .unwrap()
        .strip_suffix(suffix)
        .unwrap()
}
const CTE: &str = "WITH component_order(id,database_id,offer_id,customer_id,project_id) AS(VALUES('301'::text,'302'::text,'2001'::text,'100'::text,'101'::text)), component_database(id,order_id,operation_id,customer_id,project_id) AS(VALUES('302'::text,'900'::text,'901'::text,'100'::text,'101'::text)), component_job(operation_id,database_id,action) AS(VALUES('303'::text,'302'::text,'restore'::text)), component_operation(id,desired,created_at) AS(VALUES('303'::text,'{\"restoreOrderID\":\"301\",\"restoreOfferID\":\"2001\",\"restoreIntentHash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}'::jsonb,now())) ";
const LEGACY_REPLAY: &str = "SELECT jsonb_build_object('orderID',o.id,'databaseID',d.id,'operationID',op.id) AS value FROM webdock_auth.hosting_database_order o LEFT JOIN webdock_auth.hosting_database d ON d.id=o.database_id AND d.order_id=o.id LEFT JOIN webdock_auth.hosting_database_job j ON j.database_id=d.id AND j.action='restore' LEFT JOIN webdock_auth.hosting_operation op ON op.id=j.operation_id AND op.desired->>'restoreOfferID'=o.offer_id AND op.desired->>'restoreIntentHash'=$2 WHERE o.offer_id=$1 ORDER BY op.created_at,op.id LIMIT 2";
#[tokio::test]
#[ignore = "Root-coordinated copied SQL primitive window only"]
async fn component_original_order_replay() {
    let i = input();
    let db = connect(&i.worker_url).await;
    let original = if i.baseline {
        LEGACY_REPLAY
    } else {
        RESTORE_RECEIPT_QUERY
    };
    let query = original
        .replace("webdock_auth.hosting_database_order", "component_order")
        .replace("webdock_auth.hosting_database_job", "component_job")
        .replace("webdock_auth.hosting_database d", "component_database d")
        .replace("webdock_auth.hosting_operation", "component_operation");
    let query =
        format!("{CTE} SELECT coalesce(jsonb_agg(x.value),'[]'::jsonb) AS value FROM ({query}) x");
    let rows = value(&db, &query, vec!["2001".into(), "a".repeat(64).into()]).await;
    assert_eq!(
        rows,
        json!([{"orderID":"301","databaseID":"302","operationID":"303"}]),
        "original receipt survives later current pointers"
    );
    db.close().await.unwrap();
}
#[tokio::test]
#[ignore = "Root-coordinated copied SQL primitive window only"]
async fn component_null_required_links() {
    let i = input();
    let db = connect(&i.worker_url).await;
    let operator = if i.baseline { "<>" } else { "IS DISTINCT FROM" };
    // Isolated SQL NULL semantics for every required nullable FK/JSON-link class;
    // these are component values, not a successful fake database/domain record.
    for field in [
        "accepted.database_id",
        "op.target_revision",
        "target.operation_id",
        "op.subject",
        "op.actor_session",
        "op.customer_id",
        "op.project_id",
        "op.cluster_id",
        "j.source_backend_id",
        "j.revision",
        "desired.restoreOfferID",
        "desired.restoreOrderID",
        "desired.restoreIntentHash",
        "desired.nativeEpoch",
        "desired.nativeAuthority",
        "desired.restorePriceAccepted",
        "desired.restorePrice",
        "j.recovery",
        "target.pendingOrderBinding",
    ] {
        for missing in [json!({}), json!({"field":null})] {
            let sql = format!(
                "SELECT to_jsonb((($1::jsonb->>'field') {operator} ($2::jsonb->>'field')) IS TRUE) AS value"
            );
            assert_eq!(
                value(
                    &db,
                    &sql,
                    vec![
                        missing.into(),
                        json!({"field":"required-exact-value"}).into()
                    ]
                )
                .await,
                true,
                "missing/null required link {field}"
            );
        }
    }
    db.close().await.unwrap();
}
#[tokio::test]
#[ignore = "Root-coordinated copied SQL primitive window only"]
async fn component_fence_owner_savepoints_and_role_privileges() {
    let i = input();
    let owner = connect(&i.owner_url).await;
    let worker = connect(&i.worker_url).await;
    let business = connect(&i.business_url).await;
    webdock_persistence::connection::verify_runtime_role(&worker, "webdock_worker_runtime")
        .await
        .unwrap();
    webdock_persistence::connection::verify_runtime_role(&business, "webdock_auth_runtime")
        .await
        .unwrap();
    assert_eq!(value(&owner,"SELECT to_jsonb(has_database_privilege('webdock_platform_owner',current_database(),'TEMP')) AS value",vec![]).await,true,"no silent TEMP grant");
    for db in [&worker, &business] {
        for table in [
            "hosting_database_backend",
            "hosting_database_order",
            "hosting_database",
            "hosting_database_port",
            "hosting_database_job",
            "hosting_operation",
        ] {
            let privileges=value(db,"SELECT jsonb_build_object('select',has_table_privilege(current_user,$1,'SELECT'),'insert',has_table_privilege(current_user,$1,'INSERT'),'xmin',has_column_privilege(current_user,$1,'xmin','SELECT'),'tableoid',has_column_privilege(current_user,$1,'tableoid','SELECT'),'ctid',has_column_privilege(current_user,$1,'ctid','SELECT')) AS value",vec![format!("webdock_auth.{table}").into()]).await;
            assert_eq!(
                privileges,
                json!({"select":true,"insert":true,"xmin":true,"tableoid":true,"ctid":true})
            );
            // Executes an actual zero-row INSERT RETURNING, checking privileges
            // and RLS statement semantics without inventing authorized root rows.
            run(db,&format!("INSERT INTO webdock_auth.{table} SELECT * FROM webdock_auth.{table} WHERE false RETURNING xmin,tableoid,ctid")).await;
        }
    }
    let epoch = value(
        &owner,
        "SELECT to_jsonb(epoch) AS value FROM webdock_auth.native_recovery_epoch WHERE singleton",
        vec![],
    )
    .await;
    let raw_scope = scope(&epoch);
    let (raw, sig) = signed(&i.rls_secret, &raw_scope);
    let tx = worker.begin().await.unwrap();
    install(&tx, &raw, &sig).await;
    value(
        &tx,
        "SELECT to_jsonb(set_config('webdock.database_restore_epoch',$1,true)) AS value",
        vec![epoch.as_str().unwrap().into()],
    )
    .await;
    source_denied(&tx, "Actual early database epoch witness required").await; // baseline must RED here.
    tx.rollback().await.unwrap();
    let tx = worker.begin().await.unwrap();
    run(&tx,"CREATE TEMP TABLE pg_temp.webdock_restore_epoch_witness(backend_pid integer NOT NULL,top_xid xid8 NOT NULL,epoch uuid NOT NULL) ON COMMIT DROP").await;
    install(&tx, &raw, &sig).await;
    source_denied(&tx, "Actual early database epoch witness required").await;
    denied(
        &tx,
        "SELECT webdock_rls.database_restore_epoch(NULL)",
        vec![],
        "42501",
        None,
    )
    .await;
    tx.rollback().await.unwrap();
    let tx = worker.begin().await.unwrap();
    value(
        &tx,
        "SELECT to_jsonb(webdock_rls.database_restore_epoch(NULL)) AS value",
        vec![],
    )
    .await;
    install(&tx, &raw, &sig).await;
    source_denied(&tx, "Current native operator required").await;
    for sql in [
        "SELECT * FROM pg_temp.webdock_restore_epoch_witness",
        "INSERT INTO pg_temp.webdock_restore_epoch_witness SELECT pg_backend_pid(),pg_current_xact_id(),gen_random_uuid()",
        "UPDATE pg_temp.webdock_restore_epoch_witness SET epoch=epoch",
        "TRUNCATE pg_temp.webdock_restore_epoch_witness",
        "ALTER TABLE pg_temp.webdock_restore_epoch_witness ADD COLUMN forged integer",
        "ALTER TABLE pg_temp.webdock_restore_epoch_witness OWNER TO webdock_worker_runtime",
    ] {
        denied(&tx, sql, vec![], "42501", None).await;
    }
    run(&tx, "SAVEPOINT retained_witness").await;
    run(&tx, "ROLLBACK TO SAVEPOINT retained_witness").await;
    run(&tx, "RELEASE SAVEPOINT retained_witness").await;
    source_denied(&tx, "Current native operator required").await;
    tx.commit().await.unwrap();
    // Same still-unexpired signed scope, new top TX: ON COMMIT DROP denies.
    let tx = worker.begin().await.unwrap();
    install(&tx, &raw, &sig).await;
    source_denied(&tx, "Actual early database epoch witness required").await;
    tx.rollback().await.unwrap();
    let tx = worker.begin().await.unwrap();
    run(&tx, "SAVEPOINT before_epoch").await;
    value(
        &tx,
        "SELECT to_jsonb(webdock_rls.database_restore_epoch(NULL)) AS value",
        vec![],
    )
    .await;
    run(&tx, "ROLLBACK TO SAVEPOINT before_epoch").await;
    install(&tx, &raw, &sig).await;
    source_denied(&tx, "Actual early database epoch witness required").await;
    tx.rollback().await.unwrap();
    let tx = worker.begin().await.unwrap();
    run(&tx, "SAVEPOINT release_epoch").await;
    value(
        &tx,
        "SELECT to_jsonb(webdock_rls.database_restore_epoch(NULL)) AS value",
        vec![],
    )
    .await;
    run(&tx, "RELEASE SAVEPOINT release_epoch").await;
    install(&tx, &raw, &sig).await;
    source_denied(&tx, "Current native operator required").await;
    denied(
        &tx,
        "SELECT webdock_rls.record_database_restore_admission('missing-component-op')",
        vec![],
        "42501",
        Some("Fresh unissued direct Restore required"),
    )
    .await;
    run(&tx, "SAVEPOINT drop_probe").await;
    let dropped = tx
        .execute_unprepared("DROP TABLE pg_temp.webdock_restore_epoch_witness")
        .await;
    if dropped.is_ok() {
        source_denied(&tx, "Actual early database epoch witness required").await;
    } else {
        assert_eq!(
            error_code(&dropped.err().unwrap()).as_deref(),
            Some("42501")
        );
    }
    run(&tx, "ROLLBACK TO SAVEPOINT drop_probe").await;
    run(&tx, "RELEASE SAVEPOINT drop_probe").await;
    source_denied(&tx, "Current native operator required").await;
    // DISCARD TEMP may delete privileged temp objects; absence must always deny.
    let discarded = tx.execute_unprepared("DISCARD TEMP").await;
    if discarded.is_ok() {
        source_denied(&tx, "Actual early database epoch witness required").await;
    } else {
        assert_eq!(
            error_code(&discarded.err().unwrap()).as_deref(),
            Some("25001")
        );
    }
    tx.rollback().await.unwrap();
    let held = owner.begin().await.unwrap();
    run(
        &held,
        "SELECT singleton FROM webdock_auth.native_recovery_epoch WHERE singleton FOR UPDATE",
    )
    .await;
    let tx = worker.begin().await.unwrap();
    let Err(miss) = tx
        .execute_unprepared("SELECT webdock_rls.database_restore_epoch(NULL)")
        .await
    else {
        panic!("NOWAIT must fail")
    };
    assert_eq!(error_code(&miss).as_deref(), Some("55P03"));
    tx.rollback().await.unwrap();
    held.rollback().await.unwrap();
    let tx = worker.begin().await.unwrap();
    denied(
        &tx,
        "SELECT webdock_rls.database_restore_epoch('00000000-0000-0000-0000-000000000001')",
        vec![],
        "55000",
        None,
    )
    .await;
    tx.rollback().await.unwrap();
    reverse_source_lock_cases(&owner, &i.owner_url).await;
    owner.close().await.unwrap();
    worker.close().await.unwrap();
    business.close().await.unwrap();
}
// Only disposable scratch rows and two owner connections model reverse lock
// order. This proves fail-fast/full-TX release primitives, not actor authorization
// or the complete application/Main recovery lock graph.
async fn reverse_source_lock_cases(owner: &DatabaseConnection, owner_url: &str) {
    run(owner,"CREATE TABLE public.pg_review_reverse_locks(id text PRIMARY KEY,value integer NOT NULL DEFAULT 0)").await;
    run(
        owner,
        "INSERT INTO public.pg_review_reverse_locks(id) VALUES('backend'),('policy'),('epoch')",
    )
    .await;
    let peer = connect(owner_url).await;
    for policy_case in [true, false] {
        let holder = owner.begin().await.unwrap();
        run(
            &holder,
            "SELECT id FROM public.pg_review_reverse_locks WHERE id='backend' FOR UPDATE",
        )
        .await;
        let holder_pid = value(
            &holder,
            "SELECT to_jsonb(pg_backend_pid()) AS value",
            vec![],
        )
        .await
        .as_i64()
        .unwrap();
        let attempt = peer.begin().await.unwrap();
        run(
            &attempt,
            "SELECT id FROM public.pg_review_reverse_locks WHERE id='epoch' FOR SHARE",
        )
        .await;
        let prior = if policy_case {
            "SELECT id FROM public.pg_review_reverse_locks WHERE id='policy' FOR SHARE"
        } else {
            "SELECT pg_advisory_xact_lock(hashtextextended('pg-review2-scratch-cluster',0))"
        };
        run(&attempt, prior).await;
        let waiting = tokio::spawn(async move {
            let requested = if policy_case {
                "SELECT id FROM public.pg_review_reverse_locks WHERE id='policy' FOR UPDATE"
            } else {
                "SELECT pg_advisory_xact_lock(hashtextextended('pg-review2-scratch-cluster',0))"
            };
            let result = holder.execute_unprepared(requested).await;
            (holder, result)
        });
        let mut saw_wait = false;
        // Real pg_locks observation, no time-based sleep or inferred scheduling.
        for _ in 0..128 {
            saw_wait=value(&attempt,"SELECT to_jsonb(EXISTS(SELECT 1 FROM pg_catalog.pg_locks WHERE pid=$1 AND NOT granted)) AS value",vec![(holder_pid as i32).into()]).await==true;
            if saw_wait {
                break;
            }
        }
        assert!(
            saw_wait,
            "reverse predecessor lock request must actually be waiting"
        );
        let state = RestoreAttemptState::default();
        let failed=state.finish(restore_one(&attempt,"SELECT jsonb_build_object('id',id) AS value FROM public.pg_review_reverse_locks WHERE id='backend' FOR SHARE NOWAIT",vec![]).await).unwrap_err();
        assert!(matches!(
            failed,
            RestoreAttemptFailure::Contention { sqlstate: "55P03" }
        ));
        assert!(matches!(state.check(), Err(RestoreAttemptFailure::Retired)));
        let Err(poisoned) = attempt.execute_unprepared("SELECT 1").await else {
            panic!("no inner savepoint recovery allowed")
        };
        assert_eq!(error_code(&poisoned).as_deref(), Some("25P02"));
        // Own the TX and explicitly release ALL its locks. No borrowed helper
        // pretends to consume this transaction; the entrypoint must do it.
        attempt.rollback().await.unwrap();
        let (holder, acquired) = tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
            .await
            .expect("full rollback must unblock predecessor")
            .unwrap();
        assert!(
            acquired.is_ok(),
            "predecessor must progress after whole attempt rollback"
        );
        run(
            &holder,
            "SELECT id FROM public.pg_review_reverse_locks WHERE id='epoch' FOR UPDATE NOWAIT",
        )
        .await;
        holder.rollback().await.unwrap();
        eprintln!(
            "lock checkpoint: reverse predecessor observed; backend55P03 propagated; entire attempt rollback released predecessor and other held lock"
        );
    }
    peer.close().await.unwrap();
    run(owner, "DROP TABLE public.pg_review_reverse_locks").await;
}
async fn tuple_insert(tx: &DatabaseTransaction, id: i32) -> Value {
    let mut tuple=value(tx,"INSERT INTO pg_temp.component_tuples(id,value) VALUES($1,0) RETURNING jsonb_build_object('id',id,'tableOid',tableoid::oid::text,'xmin',xmin::text,'ctid',ctid::text) AS value",vec![id.into()]).await;
    let top = value(
        tx,
        "SELECT to_jsonb(pg_current_xact_id()::text) AS value",
        vec![],
    )
    .await
    .as_str()
    .unwrap()
    .parse::<u64>()
    .unwrap();
    tuple["_originXid8"] = json!(
        expand_insert_xid(top, tuple["xmin"].as_str().unwrap().parse().unwrap())
            .unwrap()
            .to_string()
    );
    tuple
}
async fn tuple_scope(tx: &DatabaseTransaction, secret: &str, tuple: &Value) -> (String, String) {
    let top = value(
        tx,
        "SELECT to_jsonb(pg_current_xact_id()::text) AS value",
        vec![],
    )
    .await;
    let mut header = tuple.clone();
    let origin = header
        .as_object_mut()
        .unwrap()
        .remove("_originXid8")
        .unwrap();
    let facts = json!({"topXid8":top,"insertXid8":origin,"tuple":header});
    let (raw, sig) = signed(
        secret,
        &json!({"kind":"system","purpose":"database-order","componentTuple":facts}),
    );
    install(tx, &raw, &sig).await;
    (raw, sig)
}
async fn tuple_valid(tx: &DatabaseTransaction) -> Value {
    value(tx,"WITH p AS(SELECT webdock_rls.request_scope()->'componentTuple' f) SELECT to_jsonb(coalesce((SELECT f->>'topXid8'=pg_current_xact_id()::text AND (f->>'insertXid8')::xid8>=pg_current_xact_id() AND pg_xact_status((f->>'insertXid8')::xid8)='in progress' AND f->'tuple'=jsonb_build_object('id',t.id,'tableOid',t.tableoid::oid::text,'xmin',t.xmin::text,'ctid',t.ctid::text) FROM p JOIN pg_temp.component_tuples t ON t.id=(f#>>'{tuple,id}')::integer),false)) AS value",vec![]).await
}
#[tokio::test]
#[ignore = "Root-coordinated copied SQL primitive window only"]
async fn component_inserting_subxid_mac_final_tuple_and_replacement() {
    let i = input();
    let worker = connect(&i.worker_url).await;
    run(&worker,"CREATE TEMP TABLE pg_temp.component_tuples(id integer PRIMARY KEY,value integer NOT NULL,created_at timestamptz NOT NULL DEFAULT now()) ON COMMIT PRESERVE ROWS").await;
    let seed = worker.begin().await.unwrap();
    let old = tuple_insert(&seed, 1).await;
    seed.commit().await.unwrap();
    let tx = worker.begin().await.unwrap();
    let mut updated=value(&tx,"UPDATE pg_temp.component_tuples SET value=1,created_at=transaction_timestamp() WHERE id=1 RETURNING jsonb_build_object('id',id,'tableOid',tableoid::oid::text,'xmin',xmin::text,'ctid',ctid::text) AS value",vec![]).await;
    updated["_originXid8"] = old["_originXid8"].clone();
    if i.baseline {
        let falsely_fresh=value(&tx,"SELECT to_jsonb(xmin::text=(pg_current_xact_id()::text::numeric%4294967296)::text AND created_at>=transaction_timestamp()) AS value FROM pg_temp.component_tuples WHERE id=1",vec![]).await;
        assert_eq!(
            falsely_fresh, false,
            "UPDATE and mutable timestamp do not authenticate INSERT provenance"
        );
    }
    // A genuinely prior INSERT is committed and cannot become fresh by UPDATE.
    tuple_scope(&tx, &i.rls_secret, &updated).await;
    assert_eq!(tuple_valid(&tx).await, false);
    eprintln!("tuple checkpoint: prior committed insertion denied");
    run(&tx, "SAVEPOINT insert_released").await;
    let released = tuple_insert(&tx, 2).await;
    let proof = tuple_scope(&tx, &i.rls_secret, &released).await;
    run(&tx, "RELEASE SAVEPOINT insert_released").await;
    assert_eq!(
        value(
            &tx,
            "SELECT to_jsonb(pg_xact_status($1::xid8)) AS value",
            vec![released["_originXid8"].as_str().unwrap().into()]
        )
        .await,
        "in progress"
    );
    assert_eq!(
        tuple_valid(&tx).await,
        true,
        "released sub-XID remains in progress with retained tuple"
    );
    eprintln!("tuple checkpoint: released sub-XID in progress and tuple retained");
    let (mut changed, sig) = proof.clone();
    changed = changed.replace("\"id\":2", "\"id\":3");
    install(&tx, &changed, &sig).await;
    assert_eq!(
        value(&tx, "SELECT webdock_rls.request_scope() AS value", vec![]).await,
        Value::Null,
        "cannot mutate facts without existing private MAC"
    );
    eprintln!("tuple checkpoint: tampered MAC denied");
    install(&tx, &proof.0, &proof.1).await;
    run(&tx, "SAVEPOINT insert_aborted").await;
    let aborted = tuple_insert(&tx, 3).await;
    let copied = tuple_scope(&tx, &i.rls_secret, &aborted).await;
    run(&tx, "ROLLBACK TO SAVEPOINT insert_aborted").await;
    assert_eq!(
        value(
            &tx,
            "SELECT to_jsonb(pg_xact_status($1::xid8)) AS value",
            vec![aborted["_originXid8"].as_str().unwrap().into()]
        )
        .await,
        "aborted"
    );
    install(&tx, &copied.0, &copied.1).await;
    assert_eq!(tuple_valid(&tx).await, false);
    eprintln!("tuple checkpoint: aborted sub-XID and copied proof denied");
    let _replacement = tuple_insert(&tx, 3).await;
    assert_eq!(
        tuple_valid(&tx).await,
        false,
        "copied MAC cannot revive aborted inserting sub-XID or reused slot"
    );
    eprintln!("tuple checkpoint: rollback slot replacement denied");
    install(&tx, &proof.0, &proof.1).await;
    run(&tx, "DELETE FROM pg_temp.component_tuples WHERE id=2").await;
    let _replacement = tuple_insert(&tx, 2).await;
    assert_eq!(
        tuple_valid(&tx).await,
        false,
        "live insert sub-XID alone cannot authorize replacement final tuple"
    );
    eprintln!("tuple checkpoint: final tuple replacement denied");
    tx.commit().await.unwrap();
    let tx = worker.begin().await.unwrap();
    install(&tx, &proof.0, &proof.1).await;
    assert_eq!(
        tuple_valid(&tx).await,
        false,
        "copied MAC is bound to its original top transaction"
    );
    eprintln!("tuple checkpoint: cross-top-transaction replay denied");
    tx.rollback().await.unwrap();
    eprintln!("tuple checkpoint: rollback completed");
    worker.close().await.unwrap();
}
