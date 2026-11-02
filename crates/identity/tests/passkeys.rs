use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webdock_identity::{CookiePolicy, IdentityService, PasswordAuthentication};
fn cookie(v: &str) -> &str {
    v.split(';').next().unwrap()
}
#[tokio::test]
#[ignore = "requires isolated owned identity database"]
async fn retained_cose_passkey_verifies_real_signed_ceremonies() {
    let raw = std::env::var("WEBDOCK_IDENTITY_TEST_DATABASE_URL").unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&url.host_str()));
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
        include_str!("../migrations/0004_passkey_ceremonies.sql"),
    ] {
        db.execute_unprepared(migration).await.unwrap();
    }
    let origin = url::Url::parse("https://localhost:3125").unwrap();
    let service = IdentityService::new(
        db.clone(),
        CookiePolicy::new(true, "fixture-secret-with-at-least-32-bytes").unwrap(),
    )
    .with_passkeys(origin.clone())
    .unwrap();
    let id = service
        .create_local_account(
            "passkey@example.invalid",
            "Passkey",
            "initial-password-123",
            false,
        )
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[id.clone().into()])).await.unwrap();
    let PasswordAuthentication::Authenticated(session) = service
        .authenticate_password("passkey@example.invalid", "initial-password-123")
        .await
        .unwrap()
    else {
        panic!("password session")
    };
    let unverified = service
        .begin_passkey_registration(cookie(&session.set_cookie))
        .await
        .unwrap();
    let mut no_uv_options = unverified.options;
    let selection = no_uv_options
        .public_key
        .authenticator_selection
        .as_mut()
        .unwrap();
    selection.require_resident_key = false;
    selection.resident_key = None;
    selection.user_verification = webauthn_rs_core::proto::UserVerificationPolicy::Preferred;
    let mut no_uv = WebauthnAuthenticator::new(SoftPasskey::new(false));
    let no_uv_response = no_uv
        .do_registration(origin.clone(), no_uv_options)
        .unwrap();
    let no_uv_cookie = format!(
        "{}; {}",
        cookie(&session.set_cookie),
        cookie(&unverified.set_cookie)
    );
    assert!(
        service
            .finish_passkey_registration(&no_uv_cookie, &no_uv_response, None)
            .await
            .is_err(),
        "valid attestation without verified user must be denied"
    );
    let registration = service
        .begin_passkey_registration(cookie(&session.set_cookie))
        .await
        .unwrap();
    let mut fixture_options = registration.options;
    assert_eq!(
        fixture_options
            .public_key
            .authenticator_selection
            .as_ref()
            .unwrap()
            .user_verification,
        webauthn_rs_core::proto::UserVerificationPolicy::Required
    );
    assert!(
        fixture_options
            .public_key
            .authenticator_selection
            .as_ref()
            .unwrap()
            .require_resident_key
    );
    // Maintained software authenticator cannot store resident keys. Override only its client selection;
    // production options remain resident-required, and signature/UV verification remains unchanged.
    fixture_options
        .public_key
        .authenticator_selection
        .as_mut()
        .unwrap()
        .require_resident_key = false;
    fixture_options
        .public_key
        .authenticator_selection
        .as_mut()
        .unwrap()
        .resident_key = None;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let response = authenticator
        .do_registration(origin.clone(), fixture_options)
        .unwrap();
    let credential_id = response.raw_id.clone();
    let combined = format!(
        "{}; {}",
        cookie(&session.set_cookie),
        cookie(&registration.set_cookie)
    );
    let saved = service
        .finish_passkey_registration(&combined, &response, Some("Fixture key"))
        .await
        .unwrap();
    assert!(
        service
            .finish_passkey_registration(&combined, &response, None)
            .await
            .is_err()
    );
    assert_eq!(
        service
            .list_passkeys(cookie(&session.set_cookie))
            .await
            .unwrap()
            .len(),
        1
    );
    let ceremony = service.begin_passkey_authentication().await.unwrap();
    let mut options = ceremony.options;
    // Select our known software credential; browser discovery performs this selection on a real device.
    options.public_key.allow_credentials = vec![webauthn_rs_core::proto::AllowCredentials {
        type_: "public-key".into(),
        id: credential_id.clone(),
        transports: None,
    }];
    options.mediation = None;
    let assertion = authenticator
        .do_authentication(origin.clone(), options)
        .unwrap();
    let signed = service
        .finish_passkey_authentication(cookie(&ceremony.set_cookie), &assertion)
        .await
        .unwrap();
    assert_eq!(signed.principal.user_id, id);
    assert!(
        service
            .finish_passkey_authentication(cookie(&ceremony.set_cookie), &assertion)
            .await
            .is_err()
    );
    let wrong = service.begin_passkey_authentication().await.unwrap();
    assert!(
        service
            .finish_passkey_authentication(cookie(&wrong.set_cookie), &assertion)
            .await
            .is_err(),
        "old assertion cannot sign a new challenge"
    );
    let bad_origin = service.begin_passkey_authentication().await.unwrap();
    let mut options = bad_origin.options;
    options.public_key.allow_credentials = vec![webauthn_rs_core::proto::AllowCredentials {
        type_: "public-key".into(),
        id: credential_id.clone(),
        transports: None,
    }];
    options.mediation = None;
    let wrong_origin_assertion = authenticator
        .do_authentication(url::Url::parse("https://localhost:9999").unwrap(), options)
        .unwrap();
    assert!(
        service
            .finish_passkey_authentication(cookie(&bad_origin.set_cookie), &wrong_origin_assertion)
            .await
            .is_err(),
        "signed assertion for another origin must fail"
    );
    let stale_counter = service.begin_passkey_authentication().await.unwrap();
    let mut options = stale_counter.options;
    options.public_key.allow_credentials = vec![webauthn_rs_core::proto::AllowCredentials {
        type_: "public-key".into(),
        id: credential_id.clone(),
        transports: None,
    }];
    options.mediation = None;
    let counter_assertion = authenticator
        .do_authentication(origin.clone(), options)
        .unwrap();
    db.execute_unprepared("UPDATE webdock_auth.passkey SET counter=1000")
        .await
        .unwrap();
    assert!(
        service
            .finish_passkey_authentication(cookie(&stale_counter.set_cookie), &counter_assertion)
            .await
            .is_err(),
        "signed non-increasing authenticator counter must fail"
    );
    db.execute_unprepared("UPDATE webdock_auth.passkey SET counter=1")
        .await
        .unwrap();
    let pending = service.begin_passkey_authentication().await.unwrap();
    db.execute_unprepared(
        "UPDATE webdock_auth.identity_passkey_ceremony SET expires_at=now()-interval '1 second'",
    )
    .await
    .unwrap();
    assert!(
        service
            .finish_passkey_authentication(cookie(&pending.set_cookie), &assertion)
            .await
            .is_err()
    );
    assert!(
        service
            .delete_passkey(cookie(&session.set_cookie), &saved.id)
            .await
            .unwrap()
    );
    assert!(
        service
            .list_passkeys(cookie(&session.set_cookie))
            .await
            .unwrap()
            .is_empty()
    );
    db.close().await.unwrap();
}
