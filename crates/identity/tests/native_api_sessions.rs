//! Owned restricted PostgreSQL; disconnected legacy DB proves actual native storage.
#[path = "support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use spitzli_identity::{CookiePolicy, IdentityError, IdentityService};
use spitzli_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
use std::sync::Arc;
async fn sql(f: &fixture::Fixture, text: &str) {
    f.owner.execute_unprepared(text).await.unwrap();
}
async fn service(f: &fixture::Fixture) -> IdentityService {
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
    IdentityService::new(
        DatabaseConnection::default(),
        CookiePolicy::new(false, AUTH_SECRET).unwrap(),
    )
    .with_native_session_management(pool)
    .with_allocator(Arc::new(
        IdAllocator::connect_native(
            &f.bootstrap_url,
            spitzli_ids::Identity::platform(0, 2).unwrap(),
            "webdock_auth_bootstrap",
            false,
        )
        .await
        .unwrap(),
    ))
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; opaque automation issuance/revocation uses native storage"]
async fn native_automation_session_is_opaque_finite_and_owner_bound() {
    let f = fixture::Fixture::start().await;
    sql(&f,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('a','A','a@example.test',true,'user',false,false),('b','B','b@example.test',true,'user',false,false)"#).await;
    let native = service(&f).await;
    let sid = native
        .issue_api_session("a", "Owned scoped automation", 1)
        .await
        .unwrap();
    assert!(sid.parse::<u64>().is_ok());
    let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,token,\"userId\",\"userAgent\",extract(epoch FROM(\"expiresAt\"-statement_timestamp()))::bigint AS remaining FROM webdock_auth.session WHERE id=$1",[sid.clone().into()])).await.unwrap().unwrap();
    assert_eq!(row.try_get::<String>("", "userId").unwrap(), "a");
    assert_eq!(
        row.try_get::<String>("", "userAgent").unwrap(),
        "Owned scoped automation"
    );
    let token: String = row.try_get("", "token").unwrap();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_ne!(token, sid);
    let remaining: i64 = row.try_get("", "remaining").unwrap();
    assert!((86_390..=86_400).contains(&remaining));
    let id_cookie = CookiePolicy::new(false, AUTH_SECRET)
        .unwrap()
        .header(&sid, 3600)
        .unwrap();
    assert!(
        native
            .authenticate_cookie(id_cookie.split(';').next().unwrap())
            .await
            .unwrap()
            .is_none(),
        "returned ID is not a cookie credential"
    );
    assert!(!native.revoke_session("b", &sid).await.unwrap());
    assert!(native.revoke_session("a", &sid).await.unwrap());
    assert!(!native.revoke_session("a", &sid).await.unwrap());
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native input bounds do not allocate or persist"]
async fn native_automation_input_bounds_precede_allocation_and_storage() {
    let f = fixture::Fixture::start().await;
    let native = service(&f).await;
    let before = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT high_water FROM webdock_ids.generator WHERE slot=2",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Option<i64>>("", "high_water")
        .unwrap();
    for (label, days) in [("", 1), ("valid", 0), ("valid", 367)] {
        assert!(matches!(
            native.issue_api_session("absent", label, days).await,
            Err(IdentityError::InvalidInput)
        ));
    }
    assert!(matches!(
        native
            .issue_api_session("absent", &"x".repeat(121), 1)
            .await,
        Err(IdentityError::InvalidInput)
    ));
    let after = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT high_water FROM webdock_ids.generator WHERE slot=2",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Option<i64>>("", "high_water")
        .unwrap();
    assert_eq!(before, after);
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; managed policy grants only explicitly scoped automation"]
async fn native_automation_requires_explicit_scope_binding_under_managed_enforcement() {
    let f = fixture::Fixture::start().await;
    sql(&f,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('a','A','a@example.test',true,'user',false,false),('op','Operator','op@example.test',true,'operator',true,false);
 WITH hint AS MATERIALIZED(SELECT set_config('webdock.organization_allocation','{"customer_id":"100","organization_id":"101"}',true)) INSERT INTO webdock_admin.customers(id,name) SELECT '100','Owned automation' FROM hint;
 UPDATE webdock_auth.neon_auth_policy SET require_managed=true,activated_by='op',activated_at=now()"#).await;
    let native = service(&f).await;
    let sid = native
        .issue_api_session("a", "Owned scoped automation", 366)
        .await
        .unwrap();
    let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT token,extract(epoch FROM(\"expiresAt\"-statement_timestamp()))::bigint AS remaining FROM webdock_auth.session WHERE id=$1",[sid.clone().into()])).await.unwrap().unwrap();
    let token: String = row.try_get("", "token").unwrap();
    let remaining: i64 = row.try_get("", "remaining").unwrap();
    assert!((31_622_390..=31_622_400).contains(&remaining));
    let cookie = CookiePolicy::new(false, AUTH_SECRET)
        .unwrap()
        .header(&token, 3600)
        .unwrap();
    let cookie = cookie.split(';').next().unwrap();
    assert!(
        native.authenticate_cookie(cookie).await.unwrap().is_none(),
        "unbound automation cannot bypass managed enforcement"
    );
    f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.api_token(id,customer_id,session_id,user_id,name,token_hash,can_write,expires_at) SELECT '102','100',id,'a','Owned automation',repeat('a',64),false,\"expiresAt\" FROM webdock_auth.session WHERE id=$1",[sid.clone().into()])).await.unwrap();
    let principal = native.authenticate_cookie(cookie).await.unwrap().unwrap();
    assert!(principal.can_access());
    assert!(!principal.is_operator());
    assert!(native.revoke_session("a", &sid).await.unwrap());
    assert!(native.authenticate_cookie(cookie).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; native issuance/revocation require confirmed commit"]
async fn native_automation_never_reports_success_for_failed_commit() {
    let f = fixture::Fixture::start().await;
    sql(&f,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('a','A','a@example.test',true,'user',false,false)"#).await;
    let native = service(&f).await;
    sql(&f,"CREATE FUNCTION webdock_auth.owned_api_commit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned deferred failure'; END $$;CREATE CONSTRAINT TRIGGER owned_api_commit_failure AFTER INSERT ON webdock_auth.session DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_auth.owned_api_commit_failure()").await;
    assert!(matches!(
        native.issue_api_session("a", "Owned", 1).await,
        Err(IdentityError::Persistence)
    ));
    let count = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.session",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(count, 0);
    let lost = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT high_water FROM webdock_ids.generator WHERE slot=2",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "high_water")
        .unwrap();
    sql(
        &f,
        "DROP TRIGGER owned_api_commit_failure ON webdock_auth.session",
    )
    .await;
    let sid = native.issue_api_session("a", "Owned", 1).await.unwrap();
    assert!(sid.parse::<i64>().unwrap() > lost);
    sql(&f,"CREATE CONSTRAINT TRIGGER owned_api_commit_failure AFTER DELETE ON webdock_auth.session DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_auth.owned_api_commit_failure()").await;
    assert!(matches!(
        native.revoke_session("a", &sid).await,
        Err(IdentityError::Persistence)
    ));
    sql(&f,"DROP TRIGGER owned_api_commit_failure ON webdock_auth.session;DROP FUNCTION webdock_auth.owned_api_commit_failure()").await;
    assert!(native.revoke_session("a", &sid).await.unwrap());
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; native issuance cannot use implicit legacy allocation"]
async fn native_automation_requires_explicit_allocator_without_legacy_fallback() {
    let f = fixture::Fixture::start().await;
    let pool = Pool::connect(
        &f.bootstrap_url,
        "webdock_auth_bootstrap",
        false,
        Limits::default(),
    )
    .await
    .unwrap();
    let native = IdentityService::new(
        DatabaseConnection::default(),
        CookiePolicy::new(false, AUTH_SECRET).unwrap(),
    )
    .with_native_session_management(pool);
    assert!(matches!(
        native.issue_api_session("a", "Owned", 1).await,
        Err(IdentityError::Configuration)
    ));
}
