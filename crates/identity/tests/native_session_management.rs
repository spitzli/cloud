//! Owned local PostgreSQL only; no incoming DSN or provider network calls.
#[path = "support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{sync::Arc, time::Duration};
use webdock_identity::{CookiePolicy, IdentityError, IdentityService, SecretKeys};
use webdock_persistence::postgres::{Limits, Pool};
async fn sql(db: &impl ConnectionTrait, text: &str) {
    db.execute_unprepared(text).await.unwrap();
}
fn cookie(token: &str) -> String {
    CookiePolicy::new(false, AUTH_SECRET)
        .unwrap()
        .header(token, 3600)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}
async fn runtime(f: &fixture::Fixture) -> (Arc<IdentityService>, Pool) {
    let pool = Pool::connect(
        &f.bootstrap_url,
        "webdock_auth_bootstrap",
        false,
        Limits {
            max_connections: 1,
            min_connections: 0,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let service = IdentityService::new(
        Database::connect(&f.bootstrap_url).await.unwrap(),
        CookiePolicy::new(false, AUTH_SECRET).unwrap(),
    )
    .with_native_session_management(pool.clone())
    .with_secret_keys(
        SecretKeys::new(
            std::collections::BTreeMap::from([(1, AUTH_SECRET.into())]),
            1,
            Some(AUTH_SECRET.into()),
        )
        .unwrap(),
    );
    (Arc::new(service), pool)
}
async fn seed(f: &fixture::Fixture) {
    sql(&f.owner,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('a','A','a@example.test',true,'user',false,false),('b','B','b@example.test',true,'user',false,false);
INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('sa','owned-a-token','a',now()+interval '1 hour',now()),('sa2','owned-a2-token','a',now()+interval '1 hour',now()),('expired','owned-expired','a',now()-interval '1 hour',now()),('sb','owned-b-token','b',now()+interval '1 hour',now());
INSERT INTO webdock_auth.identity_mfa_challenge(token_hash,user_id,credential_digest,expires_at) VALUES(repeat('a',64),'a',repeat('c',64),now()+interval '5 minutes'),(repeat('b',64),'b',repeat('c',64),now()+interval '5 minutes')"#).await;
}
async fn provider(f: &fixture::Fixture) {
    sql(&f.owner,r#"CREATE SCHEMA neon_auth;
CREATE TABLE neon_auth."user"(id uuid PRIMARY KEY,banned boolean);
CREATE TABLE neon_auth.session(id uuid PRIMARY KEY,"userId" uuid,"expiresAt" timestamptz,"impersonatedBy" uuid);
CREATE TABLE neon_auth.account("userId" uuid,"providerId" text,password text);
GRANT USAGE ON SCHEMA neon_auth TO webdock_platform_owner;GRANT SELECT ON ALL TABLES IN SCHEMA neon_auth TO webdock_platform_owner;
INSERT INTO neon_auth."user" VALUES('00000000-0000-4000-8000-000000000001',false);
INSERT INTO neon_auth.session VALUES('00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001',now()+interval '1 hour',null);
INSERT INTO neon_auth.account VALUES('00000000-0000-4000-8000-000000000001','credential','owned-hash');
INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES('a','https://provider.example.test/auth','00000000-0000-4000-8000-000000000001','a');
INSERT INTO webdock_auth.neon_session(session_id,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,mfa_verified) VALUES('sa','a','00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001',1,encode(sha256(convert_to('owned-hash','UTF8')),'hex'),'owned-placeholder',false)"#).await;
}
async fn count(f: &fixture::Fixture, text: &str) -> i64 {
    f.owner
        .query_one_raw(Statement::from_string(DbBackend::Postgres, text))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
async fn blocked(f: &fixture::Fixture, pattern: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let found=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE $1) AS blocked",[pattern.into()])).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
        if found {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "owned canonical query must be waiting for its row lock"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native management ownership and setup parity"]
async fn native_management_preserves_session_ownership_and_setup_access() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let (native, _) = runtime(&f).await;
    let legacy = IdentityService::new(
        Database::connect(&f.bootstrap_url).await.unwrap(),
        CookiePolicy::new(false, AUTH_SECRET).unwrap(),
    );
    assert_eq!(
        serde_json::to_value(
            legacy
                .list_sessions(&cookie("owned-a-token"))
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(
            native
                .list_sessions(&cookie("owned-a-token"))
                .await
                .unwrap()
        )
        .unwrap()
    );
    assert!(
        !native
            .revoke_owned_session(&cookie("owned-a-token"), "sb")
            .await
            .unwrap()
    );
    assert!(
        !native
            .revoke_session_by_token(&cookie("owned-a-token"), "owned-b-token")
            .await
            .unwrap()
    );
    sql(&f.owner,r#"UPDATE webdock_auth."user" SET "emailVerified"=false,"mustChangePassword"=true WHERE id='a'"#).await;
    assert_eq!(
        native
            .list_sessions(&cookie("owned-a-token"))
            .await
            .unwrap()
            .len(),
        2,
        "setup users retain only self-session management"
    );
    assert!(
        native
            .revoke_owned_session(&cookie("owned-a-token"), "sa2")
            .await
            .unwrap()
    );
    assert_eq!(
        native
            .revoke_other_sessions(&cookie("owned-a-token"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.identity_mfa_challenge WHERE user_id='a'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.identity_mfa_challenge WHERE user_id='b'"
        )
        .await,
        1
    );
    assert!(
        native
            .authenticate_cookie(&cookie("owned-b-token"))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        native
            .revoke_session_by_token(&cookie("owned-a-token"), "owned-a-token")
            .await
            .unwrap()
    );
    assert!(
        native
            .authenticate_cookie(&cookie("owned-a-token"))
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native renewal throttling and one-connection reuse"]
async fn native_refresh_preserves_token_and_throttle_without_pool_deadlock() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let (native, pool) = runtime(&f).await;
    let a = native
        .refresh_session(&cookie("owned-a-token"))
        .await
        .unwrap();
    assert_eq!(a.principal.session_id, "sa");
    assert_eq!(
        a.set_cookie.split(';').next().unwrap(),
        cookie("owned-a-token")
    );
    let before = a.principal.expires_at;
    let b = native
        .refresh_session(&cookie("owned-a-token"))
        .await
        .unwrap();
    assert_eq!(b.principal.expires_at, before);
    sql(
        &f.owner,
        "UPDATE webdock_auth.session SET \"updatedAt\"=now()-interval '16 minutes' WHERE id='sa'",
    )
    .await;
    let c = native
        .refresh_session(&cookie("owned-a-token"))
        .await
        .unwrap();
    assert!(c.principal.expires_at > chrono::Utc::now() + chrono::Duration::hours(7));
    assert!(c.principal.expires_at < chrono::Utc::now() + chrono::Duration::hours(9));
    assert_eq!(
        c.set_cookie.split(';').next().unwrap(),
        cookie("owned-a-token")
    );
    assert_eq!(pool.status().size, 1);
    assert!(matches!(
        native.refresh_session(&cookie("owned-expired")).await,
        Err(IdentityError::Unauthorized)
    ));
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; fresh provider authority after canonical lock"]
async fn native_mutation_rechecks_provider_after_waiting_for_user_lock() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    provider(&f).await;
    sql(
        &f.owner,
        "ALTER ROLE webdock_auth_bootstrap SET default_transaction_isolation='repeatable read'",
    )
    .await;
    let (native, _) = runtime(&f).await;
    let gate = f.owner.begin().await.unwrap();
    sql(
        &gate,
        "SELECT 1 FROM webdock_auth.\"user\" WHERE id='a' FOR UPDATE",
    )
    .await;
    let task =
        tokio::spawn(async move { native.revoke_other_sessions(&cookie("owned-a-token")).await });
    blocked(&f, "%FOR UPDATE OF u,s%").await;
    sql(
        &f.owner,
        "UPDATE neon_auth.session SET \"expiresAt\"=now()-interval '1 second'",
    )
    .await;
    gate.commit().await.unwrap();
    assert!(
        matches!(task.await.unwrap(), Err(IdentityError::Unauthorized)),
        "provider revocation committed while waiting must precede mutation"
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.session WHERE \"userId\"='a'"
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.identity_mfa_challenge WHERE user_id='a'"
        )
        .await,
        1
    );
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native caller-owned session row fence"]
async fn native_caller_revalidation_retains_canonical_share_locks() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let (native, pool) = runtime(&f).await;
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, principal) = native
        .authenticate_session_native_in(tx, "a", "sa")
        .await
        .unwrap();
    assert!(principal.unwrap().can_access());
    let owner = f.owner.clone();
    let deletion = tokio::spawn(async move {
        owner
            .execute_unprepared("DELETE FROM webdock_auth.session WHERE id='sa'")
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !deletion.is_finished(),
        "canonical SHARE fence must block concurrent revocation until caller completion"
    );
    tx.commit().await.unwrap();
    drop(connection);
    deletion.await.unwrap();
    let mut connection = pool.acquire().await.unwrap();
    let tx = connection.transaction().await.unwrap();
    let (tx, principal) = native
        .authenticate_session_native_in(tx, "a", "sa")
        .await
        .unwrap();
    assert!(principal.is_none());
    tx.rollback().await.unwrap();
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native logout revokes even after provider/session denial"]
async fn native_logout_revokes_local_session_without_provider_grant() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    provider(&f).await;
    let (native, _) = runtime(&f).await;
    let keys = SecretKeys::new(
        std::collections::BTreeMap::from([(1, AUTH_SECRET.into())]),
        1,
        Some(AUTH_SECRET.into()),
    )
    .unwrap();
    let envelope=keys.encrypt(&serde_json::json!({"user":"a","subject":"00000000-0000-4000-8000-000000000001","session":"00000000-0000-4000-8000-000000000002","cookie":"owned-provider-cookie"}).to_string()).unwrap();
    f.owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.neon_session SET encrypted_cookie=$1 WHERE session_id='sa'",
            [envelope.into()],
        ))
        .await
        .unwrap();
    sql(
        &f.owner,
        "UPDATE neon_auth.session SET \"expiresAt\"=now()-interval '1 second'",
    )
    .await;
    assert!(
        native
            .authenticate_cookie(&cookie("owned-a-token"))
            .await
            .unwrap()
            .is_none()
    );
    native.logout(&cookie("owned-a-token")).await.unwrap();
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.session WHERE id='sa'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.neon_session WHERE session_id='sa'"
        )
        .await,
        0
    );
    assert!(
        native
            .authenticate_cookie(&cookie("owned-b-token"))
            .await
            .unwrap()
            .is_some()
    );
    native.logout(&cookie("owned-a-token")).await.unwrap();
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; fresh acting scope after canonical share-lock wait"]
async fn native_caller_revalidation_refreshes_acting_scope_after_lock_wait() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    sql(&f.owner,r#"UPDATE webdock_auth."user" SET role='operator',"twoFactorEnabled"=true WHERE id='a';
 WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"100","organization_id":"101"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '100','Owned scope' FROM hint;
 INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES('sa','a','100','101')"#).await;
    let (native, pool) = runtime(&f).await;
    let gate = f.owner.begin().await.unwrap();
    sql(
        &gate,
        "SELECT 1 FROM webdock_auth.\"user\" WHERE id='a' FOR UPDATE",
    )
    .await;
    let task = tokio::spawn(async move {
        let mut connection = pool.acquire().await.unwrap();
        let tx = connection.transaction().await.unwrap();
        let (tx, principal) = native
            .authenticate_session_native_in(tx, "a", "sa")
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        principal.unwrap()
    });
    blocked(&f, "%FOR SHARE OF u,s%").await;
    sql(
        &f.owner,
        "UPDATE webdock_auth.studio_tenant_preview SET mode='owner' WHERE session_id='sa'",
    )
    .await;
    gate.commit().await.unwrap();
    let principal = task.await.unwrap();
    assert!(
        principal.is_acting_customer(),
        "acting scope must be read after acquiring the canonical share locks"
    );
    assert!(!principal.is_operator());
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; canceled native mutation releases unsafe connection"]
async fn cancelled_native_mutation_cannot_delete_sessions_or_challenges() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let (native, pool) = runtime(&f).await;
    let gate = f.owner.begin().await.unwrap();
    sql(
        &gate,
        "SELECT 1 FROM webdock_auth.\"user\" WHERE id='a' FOR UPDATE",
    )
    .await;
    let request_native = native.clone();
    let task = tokio::spawn(async move {
        request_native
            .revoke_other_sessions(&cookie("owned-a-token"))
            .await
    });
    blocked(&f, "%FOR UPDATE OF u,s%").await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(pool.status().size, 0);
    gate.rollback().await.unwrap();
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.session WHERE \"userId\"='a'"
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &f,
            "SELECT count(*) AS n FROM webdock_auth.identity_mfa_challenge WHERE user_id='a'"
        )
        .await,
        1
    );
    assert_eq!(
        native
            .list_sessions(&cookie("owned-a-token"))
            .await
            .unwrap()
            .len(),
        2
    );
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; invalid cookies must not wait on saturated DB pool"]
async fn invalid_management_cookie_is_rejected_before_database_acquisition() {
    let f = fixture::Fixture::start().await;
    let (native, pool) = runtime(&f).await;
    let _held = pool.acquire().await.unwrap();
    for operation in ["list", "other", "target", "refresh"] {
        let result = tokio::time::timeout(Duration::from_millis(500), async {
            match operation {
                "list" => native
                    .list_sessions("webdock-auth.session_token=forged")
                    .await
                    .map(|_| ()),
                "other" => native
                    .revoke_other_sessions("webdock-auth.session_token=forged")
                    .await
                    .map(|_| ()),
                "target" => native
                    .revoke_owned_session("webdock-auth.session_token=forged", "target")
                    .await
                    .map(|_| ()),
                _ => native
                    .refresh_session("webdock-auth.session_token=forged")
                    .await
                    .map(|_| ()),
            }
        })
        .await;
        assert!(
            matches!(result, Ok(Err(IdentityError::Unauthorized))),
            "invalid cookie must not acquire or wait for a DB connection: {operation}"
        );
    }
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; expiry during canonical lock wait must not resurrect sessions"]
async fn expired_session_waiting_for_lock_cannot_be_renewed_in_either_backend() {
    for managed in [true, false] {
        expiry_during_wait(managed, "refresh").await;
    }
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; expired sessions cannot list after waiting for a lock"]
async fn expired_session_cannot_list_after_lock_wait_in_either_backend() {
    for managed in [true, false] {
        expiry_during_wait(managed, "list").await;
    }
}
async fn expiry_during_wait(managed: bool, operation: &'static str) {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = if managed {
        runtime(&f).await.0
    } else {
        Arc::new(IdentityService::new(
            Database::connect(&f.bootstrap_url).await.unwrap(),
            CookiePolicy::new(false, AUTH_SECRET).unwrap(),
        ))
    };
    sql(&f.owner,"UPDATE webdock_auth.session SET \"expiresAt\"=now()+interval '2 seconds',\"updatedAt\"=now()-interval '16 minutes' WHERE id='sa'").await;
    let gate = f.owner.begin().await.unwrap();
    sql(
        &gate,
        "SELECT 1 FROM webdock_auth.\"user\" WHERE id='a' FOR UPDATE",
    )
    .await;
    let task = tokio::spawn(async move {
        if operation == "list" {
            service
                .list_sessions(&cookie("owned-a-token"))
                .await
                .map(|_| ())
        } else {
            service
                .refresh_session(&cookie("owned-a-token"))
                .await
                .map(|_| ())
        }
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        let waiting=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%FOR UPDATE OF u,s%') AS blocked")).await.unwrap().unwrap().try_get::<bool>("","blocked").unwrap();
        if waiting {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "refresh must begin while cookie is still valid"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(2200)).await;
    gate.commit().await.unwrap();
    assert!(
        matches!(task.await.unwrap(), Err(IdentityError::Unauthorized)),
        "session expiry during lock wait must not be renewed: native={managed}"
    );
    assert_eq!(count(&f,"SELECT count(*) AS n FROM webdock_auth.session WHERE id='sa' AND \"expiresAt\"<=statement_timestamp()").await,1);
}
