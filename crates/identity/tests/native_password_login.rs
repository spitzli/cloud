//! Real password/MFA persistence against restricted local PostgreSQL, no retained DB.
#[path = "support/native_pg.rs"]
mod fixture;
use fixture::AUTH_SECRET;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use spitzli_identity::{
    CookiePolicy, IdentityError, IdentityService, MfaProof, PasswordAuthentication, SecretKeys,
};
use spitzli_persistence::{
    IdAllocator,
    postgres::{Limits, Pool},
};
use std::{collections::BTreeMap, sync::Arc};

fn keys() -> SecretKeys {
    SecretKeys::new(
        BTreeMap::from([(0, AUTH_SECRET.into())]),
        0,
        Some(AUTH_SECRET.into()),
    )
    .unwrap()
}
fn cookie(value: &str) -> &str {
    value.split(';').next().unwrap()
}
async fn sql(f: &fixture::Fixture, value: &str) {
    f.owner.execute_unprepared(value).await.unwrap();
}
async fn count(f: &fixture::Fixture, table: &str) -> i64 {
    f.owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT count(*) AS n FROM webdock_auth.{table}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
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
    .with_native_password_login(pool)
    .with_secret_keys(keys())
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
async fn seed(f: &fixture::Fixture) {
    sql(f,r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('op','Operator','op@example.test',true,'operator',true,false),('a','A','a@example.test',true,'user',false,false),('b','B','b@example.test',true,'user',false,false)"#).await;
    let hash = spitzli_identity::hash_password("Owned-original-password-123").unwrap();
    f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,r#"INSERT INTO webdock_auth.account(id,"accountId","providerId","userId",password,"updatedAt") VALUES('21','op','credential','op',$1,now()),('22','a','credential','a',$1,now()),('23','b','credential','b',$1,now())"#,[hash.into()])).await.unwrap();
    f.owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,r#"INSERT INTO webdock_auth."twoFactor"(id,secret,"backupCodes","userId",verified) VALUES('31',$1,$2,'op',true)"#,[keys().encrypt("0123456789abcdefghijklmnopqrstuv").unwrap().into(),keys().encrypt("[\"owned-recovery-one\",\"owned-recovery-two\"]").unwrap().into()])).await.unwrap();
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; native password/MFA/session flow with retained storage disconnected"]
async fn native_password_step_up_issues_only_a_confirmed_mfa_session_and_rejects_replay() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let PasswordAuthentication::MfaRequired {
        user_id,
        set_cookie,
    } = service
        .authenticate_password(" OP@EXAMPLE.TEST ", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    assert_eq!(user_id, "op");
    assert_eq!(count(&f, "session").await, 0);
    assert!(
        service
            .authenticate_cookie(cookie(&set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    let session = service
        .complete_mfa(
            cookie(&set_cookie),
            MfaProof::RecoveryCode("owned-recovery-one".into()),
        )
        .await
        .unwrap();
    assert!(session.principal.is_operator());
    assert_eq!(session.principal.user_id, "op");
    assert!(
        service
            .authenticate_cookie(cookie(&session.set_cookie))
            .await
            .unwrap()
            .unwrap()
            .is_operator()
    );
    assert_eq!(count(&f, "identity_mfa_challenge").await, 0);
    assert_eq!(count(&f, "session").await, 1);
    assert!(matches!(
        service
            .complete_mfa(
                cookie(&set_cookie),
                MfaProof::RecoveryCode("owned-recovery-two".into())
            )
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    assert!(matches!(
        service
            .complete_mfa(
                cookie(&set_cookie),
                MfaProof::RecoveryCode("owned-recovery-one".into())
            )
            .await,
        Err(IdentityError::InvalidCode)
    ));
    assert_eq!(count(&f, "session").await, 1);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; native local login preserves setup, bans and operator gates"]
async fn native_password_login_preserves_setup_role_and_ban_boundaries() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    assert!(matches!(
        service
            .authenticate_password("a@example.test", "wrong-password")
            .await
            .unwrap(),
        PasswordAuthentication::Invalid
    ));
    assert!(matches!(
        service
            .authenticate_password("missing@example.test", "Owned-original-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::Invalid
    ));
    let PasswordAuthentication::Authenticated(session) = service
        .authenticate_password("a@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("ordinary verified account")
    };
    assert!(session.principal.can_access());
    assert!(!session.principal.is_operator());
    sql(
        &f,
        r#"UPDATE webdock_auth."user" SET "emailVerified"=false WHERE id='a'"#,
    )
    .await;
    assert!(matches!(
        service
            .authenticate_password("a@example.test", "Owned-original-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::SetupRequired { session: None, .. }
    ));
    sql(
        &f,
        r#"UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id='op'"#,
    )
    .await;
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("operator must enroll MFA")
    };
    assert!(!setup.principal.can_access());
    assert!(!setup.principal.is_operator());
    sql(
        &f,
        r#"UPDATE webdock_auth."user" SET banned=true WHERE id='b'"#,
    )
    .await;
    assert!(matches!(
        service
            .authenticate_password("b@example.test", "Owned-original-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::Invalid
    ));
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; native step-up enforces durable attempt and replay counters"]
async fn native_mfa_failure_limit_is_durable_and_never_issues_a_session() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    for _ in 0..5 {
        assert!(matches!(
            service
                .complete_mfa(cookie(&set_cookie), MfaProof::RecoveryCode("wrong".into()))
                .await,
            Err(IdentityError::InvalidCode)
        ));
    }
    assert!(matches!(
        service
            .complete_mfa(
                cookie(&set_cookie),
                MfaProof::RecoveryCode("owned-recovery-one".into())
            )
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT attempts FROM webdock_auth.identity_mfa_challenge",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i32>("", "attempts").unwrap(), 5);
    assert_eq!(count(&f, "session").await, 0);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; failed final commit preserves step-up and recovery code"]
async fn native_mfa_failed_commit_cannot_return_a_cookie_or_consume_the_proof() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    sql(&f,"CREATE FUNCTION webdock_auth.owned_login_commit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned deferred failure'; END $$;CREATE CONSTRAINT TRIGGER owned_login_commit_failure AFTER INSERT ON webdock_auth.session DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION webdock_auth.owned_login_commit_failure()").await;
    assert!(matches!(
        service
            .complete_mfa(
                cookie(&set_cookie),
                MfaProof::RecoveryCode("owned-recovery-one".into())
            )
            .await,
        Err(IdentityError::Persistence)
    ));
    assert_eq!(count(&f, "session").await, 0);
    assert_eq!(count(&f, "identity_mfa_challenge").await, 1);
    sql(&f,"DROP TRIGGER owned_login_commit_failure ON webdock_auth.session;DROP FUNCTION webdock_auth.owned_login_commit_failure()").await;
    assert!(
        service
            .complete_mfa(
                cookie(&set_cookie),
                MfaProof::RecoveryCode("owned-recovery-one".into())
            )
            .await
            .unwrap()
            .principal
            .is_operator()
    );
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; managed enforcement never falls back to a local session"]
async fn native_password_requires_managed_transport_when_policy_or_mapping_selects_it() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    sql(&f,"UPDATE webdock_auth.neon_auth_policy SET require_managed=true,activated_by='op',activated_at=now()").await;
    assert!(matches!(
        service
            .authenticate_password("a@example.test", "Owned-original-password-123")
            .await,
        Err(IdentityError::Configuration)
    ));
    sql(&f,"INSERT INTO webdock_auth.neon_identity(user_id,provider_origin,provider_subject,enabled,linked_by) VALUES('a','https://owned.neonauth.eu.neon.tech/neondb/auth','00000000-0000-4000-8000-000000000001',true,'owned')").await;
    assert!(matches!(
        service
            .authenticate_password("a@example.test", "Owned-original-password-123")
            .await,
        Err(IdentityError::Configuration)
    ));
    sql(
        &f,
        "UPDATE webdock_auth.neon_identity SET enabled=false,revision=revision+1 WHERE user_id='a'",
    )
    .await;
    assert!(matches!(
        service
            .authenticate_password("a@example.test", "Owned-original-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::Invalid
    ));
    assert_eq!(count(&f, "session").await, 0);
    assert_eq!(count(&f, "identity_mfa_challenge").await, 0);
}

async fn wait_for_lock(f: &fixture::Fixture, pattern: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(10),async{
        loop{
            let row=f.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND wait_event_type='Lock' AND query LIKE $1) AS blocked",[pattern.into()])).await.unwrap().unwrap();
            if row.try_get::<bool>("","blocked").unwrap(){break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; native login rechecks credential after lock waits"]
async fn native_password_cannot_use_a_credential_replaced_after_verification() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f).await);
    let changed = spitzli_identity::hash_password("Owned-replaced-password-456").unwrap();
    let guard = f.owner.begin().await.unwrap();
    guard
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.account SET password=$1 WHERE id='22'",
            [changed.into()],
        ))
        .await
        .unwrap();
    let job = service.clone();
    let task = tokio::spawn(async move {
        job.authenticate_password("a@example.test", "Owned-original-password-123")
            .await
    });
    wait_for_lock(&f, "SELECT password%").await;
    guard.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        PasswordAuthentication::Invalid
    ));
    assert_eq!(count(&f, "session").await, 0);
}

#[tokio::test]
#[ignore = "owned PostgreSQL18; deleted credentials retain the invalid-login contract"]
async fn native_password_deleted_during_verification_returns_invalid_without_a_session() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f).await);
    let guard = f.owner.begin().await.unwrap();
    guard
        .execute_unprepared("DELETE FROM webdock_auth.account WHERE id='22'")
        .await
        .unwrap();
    let job = service.clone();
    let task = tokio::spawn(async move {
        job.authenticate_password("a@example.test", "Owned-original-password-123")
            .await
    });
    wait_for_lock(&f, "SELECT password%").await;
    guard.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        PasswordAuthentication::Invalid
    ));
    assert_eq!(count(&f, "session").await, 0);
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; native MFA refreshes expiry after factor lock waits"]
async fn native_mfa_expiring_during_a_factor_wait_cannot_consume_recovery_code() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = Arc::new(service(&f).await);
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
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
    sql(&f,"UPDATE webdock_auth.identity_mfa_challenge SET expires_at=statement_timestamp()+interval '2 seconds'").await;
    let job = service.clone();
    let task = tokio::spawn(async move {
        job.complete_mfa(
            cookie(&set_cookie),
            MfaProof::RecoveryCode("owned-recovery-one".into()),
        )
        .await
    });
    wait_for_lock(&f, "SELECT id FROM webdock_auth.%twoFactor%").await;
    tokio::time::timeout(std::time::Duration::from_secs(3),async{loop{let row=f.owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT expires_at<clock_timestamp() AS expired FROM webdock_auth.identity_mfa_challenge")).await.unwrap().unwrap();if row.try_get::<bool>("","expired").unwrap(){break;}tokio::time::sleep(std::time::Duration::from_millis(20)).await;}}).await.unwrap();
    guard.commit().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(count(&f, "session").await, 0);
    let PasswordAuthentication::MfaRequired { set_cookie, .. } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    assert!(
        service
            .complete_mfa(
                cookie(&set_cookie),
                MfaProof::RecoveryCode("owned-recovery-one".into())
            )
            .await
            .unwrap()
            .principal
            .is_operator()
    );
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; native TOTP counter is one-use across challenges"]
async fn native_totp_replay_is_rejected_across_distinct_login_challenges() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    let PasswordAuthentication::MfaRequired {
        set_cookie: first, ..
    } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    let PasswordAuthentication::MfaRequired {
        set_cookie: second, ..
    } = service
        .authenticate_password("op@example.test", "Owned-original-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA required")
    };
    let otp = totp_rs::Builder::new()
        .with_secret(b"0123456789abcdefghijklmnopqrstuv".to_vec())
        .with_algorithm(totp_rs::Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .build()
        .unwrap();
    let code = otp.generate_current().to_string();
    assert!(
        service
            .complete_mfa(cookie(&first), MfaProof::Totp(code.clone()))
            .await
            .unwrap()
            .principal
            .is_operator()
    );
    assert!(matches!(
        service
            .complete_mfa(cookie(&second), MfaProof::Totp(code))
            .await,
        Err(IdentityError::InvalidCode)
    ));
    assert_eq!(count(&f, "session").await, 1);
    assert_eq!(count(&f, "identity_totp_replay").await, 1);
}
#[tokio::test]
#[ignore = "owned PostgreSQL18; durable native first-factor rate limit rejects without issuance"]
async fn native_password_rate_admission_commits_before_rejecting_the_attempt() {
    let f = fixture::Fixture::start().await;
    seed(&f).await;
    let service = service(&f).await;
    sql(&f,"INSERT INTO webdock_auth.\"rateLimit\"(id,key,count,\"lastRequest\") VALUES('rate','identity:password:'||encode(sha256(convert_to('a@example.test','UTF8')),'hex'),10,floor(extract(epoch FROM statement_timestamp())*1000)::bigint)").await;
    assert!(matches!(
        service
            .authenticate_password("a@example.test", "Owned-original-password-123")
            .await,
        Err(IdentityError::RateLimited)
    ));
    let row = f
        .owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count FROM webdock_auth.\"rateLimit\" WHERE id='rate'",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i32>("", "count").unwrap(), 11);
    assert_eq!(count(&f, "session").await, 0);
}
