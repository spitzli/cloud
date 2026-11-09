//! Test the internal verified-provider-result boundary; never weaken production TLS.
#[path = "../tests/support/native_pg.rs"]
mod fixture;
use super::{IdentityService, NeonMapping};
use crate::{
    CookiePolicy, IdentityError, MfaProof, NeonAuthClient, PasswordAuthentication, SecretKeys,
    neon_client::NeonLogin,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use std::{collections::BTreeMap, sync::Arc};
use webdock_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
const ISSUER: &str = "https://owned.neonauth.eu.neon.tech/neondb/auth";
const SUBJECT: &str = "00000000-0000-4000-8000-000000000001";
const SESSION: &str = "00000000-0000-4000-8000-000000000002";
fn keys() -> SecretKeys {
    SecretKeys::new(
        BTreeMap::from([(0, fixture::AUTH_SECRET.into())]),
        0,
        Some(fixture::AUTH_SECRET.into()),
    )
    .unwrap()
}
fn binding() -> NeonMapping {
    NeonMapping {
        user: "op".into(),
        subject: SUBJECT.into(),
        issuer: ISSUER.into(),
        revision: 1,
        enabled: true,
    }
}
fn login() -> NeonLogin {
    NeonLogin {
        token: "owned-provider-first-factor-token".into(),
        subject: SUBJECT.into(),
        cookie: "neon-auth.session_token=owned-provider-cookie".into(),
    }
}
#[test]
fn native_provider_facts_cannot_cross_the_configured_issuer() {
    let service = IdentityService::new(
        DatabaseConnection::default(),
        CookiePolicy::new(false, fixture::AUTH_SECRET).unwrap(),
    )
    .with_secret_keys(keys())
    .with_neon_auth(
        NeonAuthClient::new(
            "https://other.neonauth.eu.neon.tech/neondb/auth",
            "https://auth.example.test",
        )
        .unwrap(),
    );
    let facts = serde_json::json!({"subject":SUBJECT,"sessionID":SESSION,"passwordHash":"owned-provider-hash","email":"op@example.test","emailVerified":true,"expiresAt":"2027-01-01T00:00:00Z"});
    assert!(matches!(
        service.source_from_login_facts("op", &binding(), login(), facts),
        Err(IdentityError::Unauthorized)
    ));
}
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
        CookiePolicy::new(false, fixture::AUTH_SECRET).unwrap(),
    )
    .with_native_password_login(pool)
    .with_secret_keys(keys())
    .with_allocator(Arc::new(
        IdAllocator::connect_native(
            &f.bootstrap_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
            "webdock_auth_bootstrap",
            false,
        )
        .await
        .unwrap(),
    ))
    .with_neon_auth(NeonAuthClient::new(ISSUER, "https://auth.example.test").unwrap())
}
async fn seed(f: &fixture::Fixture) {
    sql(f,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('op','Operator','op@example.test',true,'operator',true,false);
    INSERT INTO webdock_auth.account(id,"accountId","providerId","userId",password,"updatedAt") VALUES('21','op','credential','op','old-local-hash',now());
    CREATE SCHEMA neon_auth;CREATE TABLE neon_auth."user"(id uuid PRIMARY KEY,email text,"emailVerified" boolean,banned boolean);CREATE TABLE neon_auth.session(id uuid PRIMARY KEY,"userId" uuid,token text,"expiresAt" timestamptz,"impersonatedBy" uuid);CREATE TABLE neon_auth.account("userId" uuid,"providerId" text,password text);
    GRANT USAGE ON SCHEMA neon_auth TO webdock_platform_owner;GRANT SELECT ON ALL TABLES IN SCHEMA neon_auth TO webdock_platform_owner;
    INSERT INTO neon_auth."user" VALUES('00000000-0000-4000-8000-000000000001','op@example.test',true,false);
    INSERT INTO neon_auth.session VALUES('00000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000001','owned-provider-first-factor-token',now()+interval '1 hour',null);
    INSERT INTO neon_auth.account VALUES('00000000-0000-4000-8000-000000000001','credential','owned-provider-hash');
    INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,linked_by) VALUES('op','https://owned.neonauth.eu.neon.tech/neondb/auth','00000000-0000-4000-8000-000000000001','owned')"#).await;
    f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,r#"INSERT INTO webdock_auth."twoFactor"(id,secret,"backupCodes","userId",verified) VALUES('31',$1,$2,'op',true)"#,[keys().encrypt("0123456789abcdefghijklmnopqrstuv").unwrap().into(),keys().encrypt("[\"owned-recovery-one\",\"owned-recovery-two\"]").unwrap().into()])).await.unwrap();
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; native verified provider result remains MFA-bound and current"]
async fn native_provider_result_produces_custodied_mfa_and_canonical_session() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .native_provider_login("op@example.test", binding(), login())
        .await
        .unwrap()
    else {
        panic!("managed first factor still needs MFA")
    };
    let session = service
        .complete_mfa(
            set_cookie.split(';').next().unwrap(),
            MfaProof::RecoveryCode("owned-recovery-one".into()),
        )
        .await
        .unwrap();
    assert!(session.principal.is_operator());
    let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT provider_subject::text,provider_session_id::text,mfa_verified,encrypted_cookie FROM webdock_auth.neon_session")).await.unwrap().unwrap();
    assert_eq!(
        row.try_get::<String>("", "provider_subject").unwrap(),
        SUBJECT
    );
    assert_eq!(
        row.try_get::<String>("", "provider_session_id").unwrap(),
        SESSION
    );
    assert!(row.try_get::<bool>("", "mfa_verified").unwrap());
    let encrypted: String = row.try_get("", "encrypted_cookie").unwrap();
    assert!(!encrypted.contains("owned-provider-cookie"));
    let envelope: serde_json::Value =
        serde_json::from_str(&keys().decrypt(&encrypted).unwrap()).unwrap();
    assert_eq!(
        envelope,
        serde_json::json!({"user":"op","subject":SUBJECT,"session":SESSION,"cookie":"neon-auth.session_token=owned-provider-cookie"})
    );
    sql(
        &f,
        "UPDATE neon_auth.account SET password='changed-provider-password'",
    )
    .await;
    assert!(
        service
            .authenticate_cookie(session.set_cookie.split(';').next().unwrap())
            .await
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; native provider proof rejects unknown token and changed binding"]
async fn native_provider_result_requires_exact_current_subject_token_issuer_and_binding() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let mut wrong = login();
    wrong.token = SUBJECT.into();
    assert!(matches!(
        service
            .native_provider_login("op@example.test", binding(), wrong)
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let mut wrong = login();
    wrong.subject = "00000000-0000-4000-8000-000000000099".into();
    assert!(matches!(
        service
            .native_provider_login("op@example.test", binding(), wrong)
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let mut wrong = binding();
    wrong.issuer = "https://other.neonauth.eu.neon.tech/neondb/auth".into();
    assert!(matches!(
        service
            .native_provider_login("op@example.test", wrong, login())
            .await,
        Err(IdentityError::Unauthorized)
    ));
    sql(&f,"UPDATE webdock_auth.neon_identity SET enabled=false,revision=revision+1 WHERE user_id='op'").await;
    assert!(matches!(
        service
            .native_provider_login("op@example.test", binding(), login())
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT password FROM webdock_auth.account WHERE id='21'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.try_get::<String>("", "password").unwrap(),
        "old-local-hash"
    );
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; provider revocation or corrupt custody cannot complete step-up"]
async fn native_provider_step_up_rejects_revoked_session_and_foreign_cookie_custody() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .native_provider_login("op@example.test", binding(), login())
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    let cookie = set_cookie.split(';').next().unwrap();
    sql(
        &f,
        "UPDATE neon_auth.session SET \"expiresAt\"=now()-interval '1 second'",
    )
    .await;
    assert!(matches!(
        service
            .complete_mfa(cookie, MfaProof::RecoveryCode("owned-recovery-one".into()))
            .await,
        Err(IdentityError::Unauthorized)
    ));
    sql(
        &f,
        "UPDATE neon_auth.session SET \"expiresAt\"=now()+interval '1 hour'",
    )
    .await;
    let foreign=keys().encrypt(&serde_json::json!({"user":"foreign","subject":SUBJECT,"session":SESSION,"cookie":"neon-auth.session_token=owned-provider-cookie"}).to_string()).unwrap();
    f.owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.neon_mfa_challenge SET encrypted_cookie=$1",
            [foreign.into()],
        ))
        .await
        .unwrap();
    assert!(matches!(
        service
            .complete_mfa(cookie, MfaProof::RecoveryCode("owned-recovery-one".into()))
            .await,
        Err(IdentityError::InvalidEncoding)
    ));
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.session",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; queued password hashing holds no max1 database connection"]
async fn native_password_worker_wait_releases_pool_and_cancellation_issues_no_proof() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f).await);
    sql(
        &f,
        "DELETE FROM webdock_auth.neon_identity WHERE user_id='op'",
    )
    .await;
    let encoded = crate::hash_password("Owned-original-password-123").unwrap();
    f.owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.account SET password=$1 WHERE id='21'",
            [encoded.into()],
        ))
        .await
        .unwrap();
    let permits = service.password_workers.acquire_many(4).await.unwrap();
    let job = service.clone();
    let task = tokio::spawn(async move {
        job.authenticate_password("op@example.test", "Owned-original-password-123")
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let row = f
                .owner
                .query_one_raw(Statement::from_string(
                    DbBackend::Postgres,
                    "SELECT count(*) AS n FROM webdock_auth.\"rateLimit\"",
                ))
                .await
                .unwrap()
                .unwrap();
            if row.try_get::<i64>("", "n").unwrap() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let cookie = CookiePolicy::new(false, fixture::AUTH_SECRET)
        .unwrap()
        .header("owned-no-session", 3600)
        .unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            service.authenticate_cookie(cookie.split(';').next().unwrap())
        )
        .await
        .unwrap()
        .unwrap()
        .is_none()
    );
    task.abort();
    assert!(matches!(task.await,Err(error) if error.is_cancelled()));
    drop(permits);
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.identity_mfa_challenge",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; provider binding remains current after factor lock waits"]
async fn native_provider_mfa_cannot_outlive_revocation_during_factor_wait() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f).await);
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .native_provider_login("op@example.test", binding(), login())
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    let guard = f.owner.begin().await.unwrap();
    guard
        .execute_unprepared("UPDATE webdock_auth.\"twoFactor\" SET secret=secret WHERE id='31'")
        .await
        .unwrap();
    let job = service.clone();
    let task = tokio::spawn(async move {
        job.complete_mfa(
            set_cookie.split(';').next().unwrap(),
            MfaProof::RecoveryCode("owned-recovery-one".into()),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3),async{loop{let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE 'SELECT id FROM webdock_auth.%twoFactor%') AS blocked")).await.unwrap().unwrap();if row.try_get::<bool>("","blocked").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}}).await.unwrap();
    sql(&f,"UPDATE webdock_auth.neon_identity SET enabled=false,revision=revision+1 WHERE user_id='op'").await;
    guard.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(IdentityError::Unauthorized)
    ));
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM webdock_auth.session",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
    sql(
        &f,
        "UPDATE webdock_auth.neon_identity SET enabled=true,revision=revision+1 WHERE user_id='op'",
    )
    .await;
    let mut current = binding();
    current.revision = 3;
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .native_provider_login("op@example.test", current, login())
        .await
        .unwrap()
    else {
        panic!("fresh first factor required")
    };
    assert!(
        service
            .complete_mfa(
                set_cookie.split(';').next().unwrap(),
                MfaProof::RecoveryCode("owned-recovery-one".into())
            )
            .await
            .unwrap()
            .principal
            .is_operator()
    );
}
