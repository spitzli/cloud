use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use webdock_identity::{
    CookiePolicy, Delivery, DeliveryError, DeliveryRequest, IdentityService,
    PasswordAuthentication, SecretKeys,
};
#[derive(Default)]
struct Capture(Mutex<Vec<DeliveryRequest>>);
#[async_trait::async_trait]
impl Delivery for Capture {
    async fn deliver(&self, request: &DeliveryRequest) -> Result<(), DeliveryError> {
        self.0.lock().unwrap().push(request.clone());
        Ok(())
    }
}
fn cookie(v: &str) -> &str {
    v.split(';').next().unwrap()
}
#[tokio::test]
#[ignore = "requires isolated owned identity database"]
async fn activation_reset_and_delivery_are_durable_scoped_and_single_use() {
    let raw = std::env::var("WEBDOCK_IDENTITY_TEST_DATABASE_URL").unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert!([Some("127.0.0.1"), Some("localhost"), Some("[::1]")].contains(&url.host_str()));
    assert_eq!(url.path(), "/webdock_identity_test");
    assert!(url.query().is_none() && url.fragment().is_none());
    assert!(["postgres", "postgresql"].contains(&url.scheme()));
    let db = Database::connect(raw).await.unwrap();
    db.execute_unprepared(include_str!("fixture.sql"))
        .await
        .unwrap();
    for migration in [
        include_str!("../migrations/0001_security_challenges.sql"),
        include_str!("../migrations/0002_authentication_preview.sql"),
        include_str!("../migrations/0005_customer_acting.sql"),
        include_str!("../migrations/0003_account_recovery.sql"),
    ] {
        db.execute_unprepared(migration).await.unwrap();
    }
    let secret = "fixture-secret-with-at-least-32-bytes";
    let service = IdentityService::new(db.clone(), CookiePolicy::new(false, secret).unwrap())
        .with_secret_keys(
            SecretKeys::new(BTreeMap::from([(0, secret.into())]), 0, Some(secret.into())).unwrap(),
        )
        .with_account_origin(url::Url::parse("http://localhost:3125").unwrap())
        .unwrap();
    let id = service
        .create_local_account(
            "recovery@example.invalid",
            "Recovery",
            "initial-password-123",
            false,
        )
        .await
        .unwrap();
    service.queue_account_activation(&id, None).await.unwrap();
    let unavailable = service.dispatch_delivery_jobs(10).await.unwrap();
    assert_eq!(unavailable.failed, 1);
    let capture = Arc::new(Capture::default());
    let service = service.with_delivery(capture.clone());
    db.execute_unprepared("UPDATE webdock_auth.identity_delivery_job SET next_attempt_at=now()")
        .await
        .unwrap();
    assert_eq!(
        service.dispatch_delivery_jobs(10).await.unwrap().delivered,
        1
    );
    let activation = capture.0.lock().unwrap()[0].url.clone();
    let token = url::Url::parse(&activation)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "token")
        .unwrap()
        .1
        .to_string();
    service
        .complete_password_reset(&token, "activated-password-123")
        .await
        .unwrap();
    assert!(
        service
            .complete_password_reset(&token, "another-password-123")
            .await
            .is_err()
    );
    let PasswordAuthentication::Authenticated(session) = service
        .authenticate_password("recovery@example.invalid", "activated-password-123")
        .await
        .unwrap()
    else {
        panic!("activated user")
    };
    service
        .request_password_reset("missing@example.invalid")
        .await
        .unwrap();
    service
        .request_password_reset("recovery@example.invalid")
        .await
        .unwrap();
    assert_eq!(
        service.dispatch_delivery_jobs(10).await.unwrap().delivered,
        1
    );
    let reset = capture.0.lock().unwrap()[1].url.clone();
    let token = url::Url::parse(&reset)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "token")
        .unwrap()
        .1
        .to_string();
    service
        .complete_password_reset(&token, "reset-password-123")
        .await
        .unwrap();
    assert!(
        service
            .authenticate_cookie(cookie(&session.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    let PasswordAuthentication::Authenticated(first) = service
        .authenticate_password("recovery@example.invalid", "reset-password-123")
        .await
        .unwrap()
    else {
        panic!("reset login")
    };
    let PasswordAuthentication::Authenticated(second) = service
        .authenticate_password("recovery@example.invalid", "reset-password-123")
        .await
        .unwrap()
    else {
        panic!("second login")
    };
    assert!(
        service
            .revoke_owned_session(cookie(&first.set_cookie), &second.principal.session_id)
            .await
            .unwrap()
    );
    assert!(
        service
            .authenticate_cookie(cookie(&second.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.session SET \"updatedAt\"=now()-interval '16 minutes',\"expiresAt\"=now()+interval '1 hour' WHERE id=$1",[first.principal.session_id.clone().into()])).await.unwrap();
    let renewed = service
        .refresh_session(cookie(&first.set_cookie))
        .await
        .unwrap();
    assert_eq!(renewed.principal.session_id, first.principal.session_id);
    assert!(renewed.principal.expires_at > chrono::Utc::now() + chrono::Duration::hours(7));
    service
        .request_password_reset("recovery@example.invalid")
        .await
        .unwrap();
    service.dispatch_delivery_jobs(10).await.unwrap();
    let expired = capture.0.lock().unwrap().last().unwrap().url.clone();
    let token = url::Url::parse(&expired)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "token")
        .unwrap()
        .1
        .to_string();
    db.execute_unprepared(
        "UPDATE webdock_auth.identity_recovery_token SET expires_at=now()-interval '1 second'",
    )
    .await
    .unwrap();
    assert!(
        service
            .complete_password_reset(&token, "another-password-123")
            .await
            .is_err()
    );
    db.close().await.unwrap();
}
