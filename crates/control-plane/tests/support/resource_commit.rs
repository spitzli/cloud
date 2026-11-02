//! A deferred audit trigger pauses real COMMIT after the final domain authority check.
//! Only unique fixture resources and actor-filtered triggers are changed.
use super::resources::*;
use axum::http::StatusCode;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::{io::Write, sync::Arc, time::Duration};
use webdock_control_plane::resource_auth::ResourceClientConfig;

#[derive(Clone, Copy, Debug)]
enum Domain {
    Registry,
    Hosting,
}
#[derive(Clone, Copy, Debug)]
enum Revocation {
    Resource,
    Issuer,
    Introspector,
}

#[tokio::test]
#[ignore = "requires explicit owned local retained-schema PostgreSQL fixture"]
async fn resource_and_both_links_remain_locked_through_actual_domain_commit() {
    let mut failures = Vec::new();
    for domain in [Domain::Registry, Domain::Hosting] {
        for revocation in [
            Revocation::Resource,
            Revocation::Issuer,
            Revocation::Introspector,
        ] {
            let fixture = Arc::new(Fixture::isolated_resource().await);
            let f = fixture.clone();
            let result = tokio::spawn(async move { scenario(f, domain, revocation).await }).await;
            let schema = barrier_schema(&fixture);
            fixture
                .owner
                .execute_unprepared(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
                .await
                .unwrap();
            fixture.cleanup().await;
            match result {
                Ok(None) => (),
                Ok(Some(failure)) => failures.push(failure),
                Err(error) => failures.push(format!("{domain:?}/{revocation:?}: {error}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn barrier_schema(fixture: &Fixture) -> String {
    let suffix = fixture
        .nonce
        .strip_prefix("rust-resources-")
        .unwrap()
        .replace('-', "");
    assert_eq!(suffix.len(), 32);
    assert!(suffix.bytes().all(|byte| byte.is_ascii_hexdigit()));
    format!("wd_resource_barrier_{suffix}")
}

async fn scenario(f: Arc<Fixture>, domain: Domain, revocation: Revocation) -> Option<String> {
    let schema = barrier_schema(&f);
    let suffix = schema.strip_prefix("wd_resource_barrier_").unwrap();
    let trigger = format!("resource_commit_{suffix}");
    let lock_key =
        u32::from_be_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap()) & 0x7fff_ffff;
    let (table, column, actor, issuer, introspector, token, path, body, success) = match domain {
        Domain::Registry => (
            "webdock_admin.audit_events",
            "actor_id",
            &f.mapped,
            &f.registry_client,
            &f.hosting_client,
            f.registry_client.write.clone(),
            "/api/registry/customers",
            json!({"name":f.nonce}),
            StatusCode::CREATED,
        ),
        Domain::Hosting => (
            "webdock_auth.hosting_audit",
            "subject",
            &f.uid,
            &f.hosting_client,
            &f.registry_client,
            f.hosting_client.write.clone(),
            "/api/hosting/clusters",
            json!({"name":f.nonce,"provider":"owned-fixture","country":"DE","region":"local","locationEvidence":"isolated commit barrier","runtimeKind":"docker","idempotencyKey":format!("{}-cluster",f.nonce)}),
            StatusCode::OK,
        ),
    };
    // These interpolated identifiers and the predicate are generated locally, never request data.
    assert!(
        actor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    );
    f.owner.execute_unprepared(&format!("CREATE SCHEMA {schema}; CREATE FUNCTION {schema}.pause_commit() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog AS $$ BEGIN PERFORM pg_advisory_xact_lock({lock_key}::bigint); RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER {trigger} AFTER INSERT ON {table} DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.{column}='{actor}') EXECUTE FUNCTION {schema}.pause_commit()")).await.unwrap();
    let barrier_connection = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let barrier = barrier_connection.begin().await.unwrap();
    barrier
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [i64::from(lock_key).into()],
        ))
        .await
        .unwrap();
    let mut secret_file = tempfile::NamedTempFile::new().unwrap();
    secret_file
        .write_all(introspector.secret.as_bytes())
        .unwrap();
    let app = f
        .app_with_client(
            None,
            Some(ResourceClientConfig {
                client_id: introspector.id.clone(),
                secret: secret_file.path().to_path_buf(),
            }),
        )
        .await;
    let request_app = app.clone();
    let request_token = token.clone();
    let request_body = body.clone();
    let mutation = tokio::spawn(async move {
        request(
            &request_app,
            "POST",
            path,
            Some(&request_token),
            Some(request_body),
        )
        .await
    });
    let entered = wait_for_commit(&f.owner, lock_key, &mutation).await;
    let revoker = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
    let pid: i32 = revoker
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    let resource = f.resource.clone();
    let client = match revocation {
        Revocation::Resource => None,
        Revocation::Issuer => Some(issuer.id.clone()),
        Revocation::Introspector => Some(introspector.id.clone()),
    };
    let revoke = tokio::spawn(async move {
        match client {
            None=>revoker.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"oauthResource\" SET disabled=true WHERE identifier=$1",[resource.into()])).await,
            Some(client)=>revoker.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM webdock_auth.\"oauthClientResource\" WHERE \"resourceId\"=$1 AND \"clientId\"=$2",[resource.into(),client.into()])).await,
        }
    });
    let blocked = wait_for_revocation_lock(&f.owner, pid, &revoke).await;
    // This is the actual domain COMMIT boundary, not a delay before final verification.
    barrier.commit().await.unwrap();
    let (status, result) = mutation.await.unwrap();
    let affected = revoke.await.unwrap().unwrap().rows_affected();
    let (denied, _) = request(&app, "POST", path, Some(&token), Some(body)).await;
    f.owner
        .execute_unprepared(&format!(
            "DROP TRIGGER {trigger} ON {table}; DROP SCHEMA {schema} CASCADE"
        ))
        .await
        .unwrap();
    if entered
        && blocked
        && status == success
        && affected == 1
        && denied == StatusCode::UNAUTHORIZED
    {
        return None;
    }
    Some(format!(
        "{domain:?}/{revocation:?}: at_commit={entered}, revocation_blocked={blocked}, mutation={status}, changed={affected}, future={denied}, result={result}"
    ))
}
async fn wait_for_commit(
    owner: &DatabaseConnection,
    key: u32,
    mutation: &tokio::task::JoinHandle<(StatusCode, Value)>,
) -> bool {
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {
            if mutation.is_finished() { return false; }
            let waiting=count(owner,"SELECT count(*) AS n FROM pg_catalog.pg_locks l JOIN pg_catalog.pg_stat_activity a ON a.pid=l.pid WHERE l.locktype='advisory' AND l.classid=0 AND l.objid=$1::oid AND l.objsubid=1 AND NOT l.granted AND upper(a.query)='COMMIT'",vec![i64::from(key).into()]).await;
            if waiting==1 { return true; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or(false)
}
async fn wait_for_revocation_lock(
    owner: &DatabaseConnection,
    pid: i32,
    revoke: &tokio::task::JoinHandle<Result<sea_orm::ExecResult, sea_orm::DbErr>>,
) -> bool {
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            if revoke.is_finished() { return false; }
            if count(owner,"SELECT count(*) AS n FROM pg_catalog.pg_stat_activity WHERE pid=$1 AND wait_event_type='Lock'",vec![pid.into()]).await==1 { return true; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap_or(false)
}
