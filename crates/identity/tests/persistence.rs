//! Run only against an explicitly supplied disposable PostgreSQL fixture.
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, Statement};
use webdock_identity::{
    CookiePolicy, IdentityError, IdentityService, PasswordAuthentication, RlsSigner,
};

#[tokio::test]
#[ignore = "requires WEBDOCK_IDENTITY_TEST_DATABASE_URL pointing at a disposable fixture"]
async fn persists_revokes_and_denies_security_setup_and_mfa_bypass() {
    let url =
        std::env::var("WEBDOCK_IDENTITY_TEST_DATABASE_URL").expect("explicit test fixture URL");
    assert!(
        safe_fixture_url(&url),
        "fixture URL must be an explicit loopback _test database without query overrides"
    );
    let mut options = ConnectOptions::new(url);
    options.max_connections(1);
    let db = Database::connect(options).await.unwrap();
    let name = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_database() AS name",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "name")
        .unwrap();
    assert!(name.ends_with("_test"), "refuse non-test database");
    db.execute_unprepared("SELECT pg_advisory_lock(hashtext('webdock-identity-tests'))")
        .await
        .unwrap();
    db.execute_unprepared(include_str!("fixture.sql"))
        .await
        .unwrap();
    db.execute_unprepared(include_str!("../migrations/0001_security_challenges.sql"))
        .await
        .unwrap();
    db.execute_unprepared(include_str!(
        "../migrations/0002_authentication_preview.sql"
    ))
    .await
    .unwrap();
    db.execute_unprepared(include_str!("../migrations/0005_customer_acting.sql"))
        .await
        .unwrap();
    let policy = CookiePolicy::new(false, "fixture-secret-with-at-least-32-bytes").unwrap();
    let service = IdentityService::new(db.clone(), policy);
    let account = service
        .create_local_account(
            "fixture@example.invalid",
            "Fixture",
            "new-password-123",
            false,
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .authenticate_password("fixture@example.invalid", "new-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::SetupRequired { .. }
    ));
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres, "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1", [account.into()])).await.unwrap();
    let PasswordAuthentication::Authenticated(session) = service
        .authenticate_password("fixture@example.invalid", "new-password-123")
        .await
        .unwrap()
    else {
        panic!("authenticated")
    };
    let cookie = session.set_cookie.split(';').next().unwrap();
    let principal = service.authenticate_cookie(cookie).await.unwrap().unwrap();
    assert!(principal.can_access());
    assert!(!principal.is_operator());
    let transaction = RlsSigner::new(
        "fixture-rls-secret-with-at-least-32-bytes",
        "webdock_auth_runtime",
    )
    .unwrap()
    .begin(&db, &principal)
    .await
    .unwrap();
    let scope: String = transaction
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_setting('webdock.rls_context') AS scope",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "scope")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&scope).unwrap()["scope"]["subject"],
        principal.user_id
    );
    transaction.commit().await.unwrap();
    let leaked: Option<String> = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT nullif(current_setting('webdock.rls_context',true),'') AS scope",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "scope")
        .unwrap();
    assert!(
        leaked.is_none(),
        "signed context must not survive transaction commit on reused pooled connection"
    );
    assert!(
        !service
            .revoke_session("wrong-user", &principal.session_id)
            .await
            .unwrap()
    );
    for (change, access, operator) in [
        ("\"mustChangePassword\"=true", false, false),
        (
            "\"mustChangePassword\"=false,\"emailVerified\"=false",
            false,
            false,
        ),
        (
            "\"emailVerified\"=true,role='operator',\"twoFactorEnabled\"=false",
            false,
            false,
        ),
        ("\"twoFactorEnabled\"=true", true, true),
    ] {
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("UPDATE webdock_auth.\"user\" SET {change} WHERE id=$1"),
            [principal.user_id.clone().into()],
        ))
        .await
        .unwrap();
        let current = service.authenticate_cookie(cookie).await.unwrap().unwrap();
        assert_eq!(current.can_access(), access);
        assert_eq!(current.is_operator(), operator);
    }
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres, "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,expires_at) VALUES($1,$2,'customer','org',now()-interval '1 hour')", [principal.session_id.clone().into(),principal.user_id.clone().into()])).await.unwrap();
    let preview = service.authenticate_cookie(cookie).await.unwrap().unwrap();
    assert!(preview.in_tenant_preview());
    assert!(
        !preview.is_operator(),
        "even expired preview must not restore root access"
    );
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 second' WHERE id=$1",
        [principal.session_id.clone().into()],
    ))
    .await
    .unwrap();
    assert!(service.authenticate_cookie(cookie).await.unwrap().is_none());
    assert!(
        service
            .revoke_session(&principal.user_id, &principal.session_id)
            .await
            .unwrap()
    );
    assert!(service.authenticate_cookie(cookie).await.unwrap().is_none());
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"twoFactorEnabled\"=true WHERE id=$1",
        [principal.user_id.clone().into()],
    ))
    .await
    .unwrap();
    assert!(matches!(
        service
            .authenticate_password("fixture@example.invalid", "new-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::MfaRequired { .. }
    ));
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET banned=true WHERE id=$1",
        [principal.user_id.clone().into()],
    ))
    .await
    .unwrap();
    assert!(matches!(
        service
            .authenticate_password("fixture@example.invalid", "new-password-123")
            .await
            .unwrap(),
        PasswordAuthentication::Invalid
    ));
    for _ in 0..6 {
        assert!(matches!(
            service
                .authenticate_password("fixture@example.invalid", "wrong-password")
                .await
                .unwrap(),
            PasswordAuthentication::Invalid
        ));
    }
    assert!(matches!(
        service
            .authenticate_password("fixture@example.invalid", "wrong-password")
            .await,
        Err(IdentityError::RateLimited)
    ));
}

fn safe_fixture_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    let name = url.path().strip_prefix('/').unwrap_or_default();
    ["postgres", "postgresql"].contains(&url.scheme())
        && [Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&url.host_str())
        && url.query().is_none()
        && url.fragment().is_none()
        && name.ends_with("_test")
        && name.len() <= 63
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[test]
fn refuses_remote_or_overridden_fixture_urls_before_connecting() {
    for value in [
        "postgres://localhost/example_test",
        "postgres://127.0.0.1/example_test",
        "postgres://[::1]/example_test",
    ] {
        assert!(safe_fixture_url(value));
    }
    for value in [
        "postgres://example.org/example_test",
        "postgres://localhost/production",
        "postgres://localhost/example_test?host=example.org",
        "postgres://localhost/example_test?service=production",
        "postgres://localhost/other/example_test",
        "postgres://localhost/example_test#x",
        "postgres://localhost/%65xample_test",
        "http://localhost/example_test",
    ] {
        assert!(!safe_fixture_url(value), "{value}");
    }
}
