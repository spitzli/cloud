//! Uses the parent's retained-schema/RLS disposable fixture; never resets it.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use webdock_identity::{
    CookiePolicy, IdentityService, PasswordAuthentication, RlsSigner, SecretKeys,
};
fn guarded(name: &str) -> String {
    let raw = std::env::var(name).expect("explicit disposable fixture configuration");
    let url = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&url.scheme()));
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&url.host_str()));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none() && url.fragment().is_none());
    raw
}
fn cookie(header: &str) -> &str {
    header.split(';').next().unwrap()
}
#[tokio::test]
#[ignore = "requires parent's explicit owned retained-schema/RLS fixture"]
async fn bootstrap_authentication_works_without_widening_business_role_permissions() {
    let migration_url = guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL");
    let auth_url = guarded("AUTH_DATABASE_URL");
    let runtime_url = guarded("DATABASE_URL");
    let offline = Database::connect(migration_url).await.unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0001_security_challenges.sql"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!(
            "../migrations/0002_authentication_preview.sql"
        ))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0005_customer_acting.sql"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0003_account_recovery.sql"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0004_passkey_ceremonies.sql"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../../persistence/migrations/0001_ids.sql"))
        .await
        .unwrap();
    offline.close().await.unwrap();
    let auth = Database::connect(auth_url.clone()).await.unwrap();
    let identity=auth.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT current_user::text AS name,(SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname=current_user) AS privileged")).await.unwrap().unwrap();
    assert_eq!(
        identity.try_get::<String>("", "name").unwrap(),
        "webdock_auth_bootstrap"
    );
    assert!(!identity.try_get::<bool>("", "privileged").unwrap());
    assert!(
        auth.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT actor_id FROM webdock_auth.studio_tenant_preview LIMIT 1"
        ))
        .await
        .is_err()
    );
    let runtime = Database::connect(runtime_url).await.unwrap();
    for sql in [
        "SELECT password FROM webdock_auth.account LIMIT 1",
        "SELECT token FROM webdock_auth.session LIMIT 1",
        "SELECT * FROM webdock_auth.identity_mfa_challenge LIMIT 1",
    ] {
        let result = runtime
            .query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
            .await;
        assert!(
            matches!(result, Err(_) | Ok(None)),
            "unscoped business role read protected data via {sql}"
        );
    }
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let keys = SecretKeys::new(
        BTreeMap::from([(0, secret.clone())]),
        0,
        Some(secret.clone()),
    )
    .unwrap();
    let service = IdentityService::new(auth.clone(), CookiePolicy::new(false, secret).unwrap())
        .with_secret_keys(keys)
        .with_passkeys(url::Url::parse("https://localhost:3125").unwrap())
        .unwrap()
        .with_allocator(std::sync::Arc::new(
            webdock_persistence::IdAllocator::connect(
                &auth_url,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        ));
    let email = format!("rust-security-{}@example.invalid", unique_suffix());
    let id = service
        .create_local_account(
            &email,
            "Rust security fixture",
            "initial-password-123",
            true,
        )
        .await
        .unwrap();
    assert_eq!((id.parse::<u64>().unwrap() >> 12) & 1023, 2);
    auth.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
        [id.clone().into()],
    ))
    .await
    .unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = service
        .authenticate_password(&email, "initial-password-123")
        .await
        .unwrap()
    else {
        panic!("setup session")
    };
    let changed = service
        .change_password(
            cookie(&setup.set_cookie),
            "initial-password-123",
            "changed-password-123",
        )
        .await
        .unwrap();
    let enrollment = service
        .begin_totp_enrollment(cookie(&changed.set_cookie), "changed-password-123")
        .await
        .unwrap();
    let authenticator = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let session = service
        .confirm_totp_enrollment(
            cookie(&changed.set_cookie),
            &authenticator.generate_current().to_string(),
        )
        .await
        .unwrap();
    assert!(session.principal.is_operator());
    let passkey = service
        .begin_passkey_registration(cookie(&session.set_cookie))
        .await
        .unwrap();
    let mut options = passkey.options;
    options
        .public_key
        .authenticator_selection
        .as_mut()
        .unwrap()
        .require_resident_key = false;
    options
        .public_key
        .authenticator_selection
        .as_mut()
        .unwrap()
        .resident_key = None;
    let mut device = webauthn_authenticator_rs::WebauthnAuthenticator::new(
        webauthn_authenticator_rs::softpasskey::SoftPasskey::new(true),
    );
    let response = device
        .do_registration(url::Url::parse("https://localhost:3125").unwrap(), options)
        .unwrap();
    let combined = format!(
        "{}; {}",
        cookie(&session.set_cookie),
        cookie(&passkey.set_cookie)
    );
    let saved = service
        .finish_passkey_registration(&combined, &response, Some("Restricted fixture"))
        .await
        .unwrap();
    assert_eq!((saved.id.parse::<u64>().unwrap() >> 12) & 1023, 2);
    service.request_password_reset(&email).await.unwrap();
    assert_eq!(
        service.dispatch_delivery_jobs(10).await.unwrap().failed,
        1,
        "default sink must not send mail"
    );
    let ids=auth.query_all_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM webdock_auth.\"user\" WHERE id=$1 UNION ALL SELECT id FROM webdock_auth.account WHERE \"userId\"=$1 UNION ALL SELECT id FROM webdock_auth.session WHERE \"userId\"=$1 UNION ALL SELECT id FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1 UNION ALL SELECT id FROM webdock_auth.\"rateLimit\" WHERE key=$2 UNION ALL SELECT d.id FROM webdock_auth.identity_delivery_job d JOIN webdock_auth.identity_recovery_token t ON t.token_hash=d.token_hash WHERE t.user_id=$1",[id.clone().into(),format!("identity:password:{}",hex::encode(Sha256::digest(email.as_bytes()))).into()])).await.unwrap();
    assert!(ids.len() >= 6);
    for row in ids {
        assert_eq!(
            (row.try_get::<String>("", "id")
                .unwrap()
                .parse::<u64>()
                .unwrap()
                >> 12)
                & 1023,
            2
        );
    }

    let signer = RlsSigner::new(
        std::env::var("DATABASE_RLS_SECRET").unwrap(),
        "webdock_auth_runtime",
    )
    .unwrap();
    let tx = signer.begin(&runtime, &session.principal).await.unwrap();
    let allowed: bool = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.operator() AS allowed",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "allowed")
        .unwrap();
    assert!(allowed);
    auth.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
        [id.clone().into()],
    ))
    .await
    .unwrap();
    let allowed: bool = tx
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT webdock_rls.operator() AS allowed",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "allowed")
        .unwrap();
    assert!(!allowed, "same transaction must recheck current ban");
    tx.commit().await.unwrap();
    auth.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        [id.into()],
    ))
    .await
    .unwrap();
    auth.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"rateLimit\" WHERE key=$1",
        [format!(
            "identity:password:{}",
            hex::encode(Sha256::digest(email.as_bytes()))
        )
        .into()],
    ))
    .await
    .unwrap();
    auth.close().await.unwrap();
    runtime.close().await.unwrap();
}
fn unique_suffix() -> String {
    let mut bytes = [0; 8];
    getrandom::getrandom(&mut bytes).unwrap();
    hex::encode(bytes)
}
