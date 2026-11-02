use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, Statement};
use std::collections::BTreeMap;
use webdock_identity::{
    CookiePolicy, IdentityError, IdentityService, MfaProof, PasswordAuthentication, SecretKeys,
};
const SECRET: &str = "fixture-secret-with-at-least-32-bytes";
fn cookie(header: &str) -> &str {
    header.split(';').next().unwrap()
}

#[tokio::test]
#[ignore = "requires an explicitly configured isolated identity test database"]
async fn operator_setup_mfa_recovery_and_revocation_are_real_and_single_use() {
    let raw = std::env::var("WEBDOCK_IDENTITY_TEST_DATABASE_URL").unwrap();
    let url = url::Url::parse(&raw).unwrap();
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&url.host_str()));
    assert!(["postgres", "postgresql"].contains(&url.scheme()));
    assert_eq!(url.path(), "/webdock_identity_test");
    assert!(url.query().is_none() && url.fragment().is_none());
    let mut options = ConnectOptions::new(raw);
    options.max_connections(3);
    let db = Database::connect(options).await.unwrap();
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
    let keys = SecretKeys::new(
        BTreeMap::from([(0, SECRET.to_owned())]),
        0,
        Some(SECRET.to_owned()),
    )
    .unwrap();
    let service = IdentityService::new(db.clone(), CookiePolicy::new(false, SECRET).unwrap())
        .with_secret_keys(keys);
    let user = service
        .provision_first_operator(
            "operator@example.invalid",
            "Operator",
            "initial-password-123",
        )
        .await
        .unwrap();
    assert!(
        service
            .provision_first_operator("second@example.invalid", "Second", "initial-password-123")
            .await
            .is_err()
    );
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = service
        .authenticate_password("operator@example.invalid", "initial-password-123")
        .await
        .unwrap()
    else {
        panic!("limited setup session required")
    };
    assert!(!setup.principal.can_access());
    let changed = service
        .change_password(
            cookie(&setup.set_cookie),
            "initial-password-123",
            "changed-password-123",
        )
        .await
        .unwrap();
    assert!(!changed.principal.can_access());
    assert!(
        service
            .authenticate_cookie(cookie(&setup.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    let enrollment = service
        .begin_totp_enrollment(cookie(&changed.set_cookie), "changed-password-123")
        .await
        .unwrap();
    assert_eq!(enrollment.backup_codes.len(), 10);
    assert!(
        !service
            .authenticate_cookie(cookie(&changed.set_cookie))
            .await
            .unwrap()
            .unwrap()
            .two_factor_enabled()
    );
    let authenticator = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    assert!(
        service
            .confirm_totp_enrollment(cookie(&changed.set_cookie), "wrong")
            .await
            .is_err()
    );
    let verified = service
        .confirm_totp_enrollment(
            cookie(&changed.set_cookie),
            &authenticator.generate_current().to_string(),
        )
        .await
        .unwrap();
    assert!(verified.principal.is_operator());
    assert!(
        service
            .authenticate_cookie(cookie(&changed.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge,
        ..
    } = service
        .authenticate_password("operator@example.invalid", "changed-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA challenge required")
    };
    assert!(
        service
            .authenticate_cookie(cookie(&challenge))
            .await
            .unwrap()
            .is_none()
    );
    // Reusing the enrollment time step must fail, even with a freshly password-authenticated challenge.
    assert!(
        service
            .complete_mfa(
                cookie(&challenge),
                MfaProof::Totp(authenticator.generate_current().to_string())
            )
            .await
            .is_err()
    );
    let recovered = service
        .complete_mfa(
            cookie(&challenge),
            MfaProof::RecoveryCode(enrollment.backup_codes[0].clone()),
        )
        .await
        .unwrap();
    assert!(recovered.principal.is_operator());
    assert!(
        service
            .complete_mfa(
                cookie(&challenge),
                MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
            )
            .await
            .is_err()
    );
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge2,
        ..
    } = service
        .authenticate_password("operator@example.invalid", "changed-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA challenge required")
    };
    assert!(
        service
            .complete_mfa(
                cookie(&challenge2),
                MfaProof::RecoveryCode(enrollment.backup_codes[0].clone())
            )
            .await
            .is_err()
    );
    assert_eq!(
        service
            .list_sessions(cookie(&recovered.set_cookie))
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        service
            .revoke_other_sessions(cookie(&recovered.set_cookie))
            .await
            .unwrap(),
        1
    );
    assert!(
        service
            .authenticate_cookie(cookie(&verified.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    // Revoking other sessions also revokes outstanding challenges.
    assert!(
        service
            .complete_mfa(
                cookie(&challenge2),
                MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
            )
            .await
            .is_err()
    );
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge3,
        ..
    } = service
        .authenticate_password("operator@example.invalid", "changed-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA challenge required")
    };
    for _ in 0..5 {
        assert!(
            service
                .complete_mfa(cookie(&challenge3), MfaProof::Totp("000000".into()))
                .await
                .is_err()
        );
    }
    assert!(
        service
            .complete_mfa(
                cookie(&challenge3),
                MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
            )
            .await
            .is_err()
    );
    let count: i64 = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS count FROM webdock_auth.session WHERE \"userId\"=$1",
            [user.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap();
    assert_eq!(count, 1, "failed MFA must not issue sessions");
    assert!(matches!(
        service
            .change_password(
                cookie(&verified.set_cookie),
                "changed-password-123",
                "another-password-123"
            )
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let PasswordAuthentication::MfaRequired {
        set_cookie: expired,
        ..
    } = service
        .authenticate_password("operator@example.invalid", "changed-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA challenge")
    };
    db.execute_unprepared(
        "UPDATE webdock_auth.identity_mfa_challenge SET expires_at=now()-interval '1 second'",
    )
    .await
    .unwrap();
    assert!(matches!(
        service
            .complete_mfa(
                cookie(&expired),
                MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
            )
            .await,
        Err(IdentityError::Unauthorized)
    ));
    let PasswordAuthentication::MfaRequired {
        set_cookie: changed_credential,
        ..
    } = service
        .authenticate_password("operator@example.invalid", "changed-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA challenge")
    };
    db.execute_unprepared("UPDATE webdock_auth.account SET password=password||'changed' WHERE \"providerId\"='credential'").await.unwrap();
    assert!(matches!(
        service
            .complete_mfa(
                cookie(&changed_credential),
                MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
            )
            .await,
        Err(IdentityError::Unauthorized)
    ));
    db.execute_unprepared("UPDATE webdock_auth.account SET password=left(password,length(password)-7) WHERE \"providerId\"='credential'").await.unwrap();
    let PasswordAuthentication::MfaRequired {
        set_cookie: race, ..
    } = service
        .authenticate_password("operator@example.invalid", "changed-password-123")
        .await
        .unwrap()
    else {
        panic!("MFA challenge")
    };
    let (first, second) = tokio::join!(
        service.complete_mfa(
            cookie(&race),
            MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
        ),
        service.complete_mfa(
            cookie(&race),
            MfaProof::RecoveryCode(enrollment.backup_codes[1].clone())
        )
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .iter()
            .filter(|success| **success)
            .count(),
        1,
        "concurrent completion must issue exactly one session"
    );
    // Across challenges, ten failed proofs lock the account for fifteen minutes.
    for _ in 0..2 {
        let PasswordAuthentication::MfaRequired {
            set_cookie: failed, ..
        } = service
            .authenticate_password("operator@example.invalid", "changed-password-123")
            .await
            .unwrap()
        else {
            panic!("MFA challenge")
        };
        for _ in 0..5 {
            assert!(
                service
                    .complete_mfa(cookie(&failed), MfaProof::Totp("invalid".into()))
                    .await
                    .is_err()
            );
        }
    }
    let locked: bool = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT \"lockedUntil\">now() AS locked FROM webdock_auth.\"twoFactor\"",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "locked")
        .unwrap();
    assert!(locked);
    let regenerated = service
        .regenerate_recovery_codes(cookie(&recovered.set_cookie), "changed-password-123")
        .await
        .unwrap();
    assert_eq!(regenerated.len(), 10);
    assert!(!regenerated.contains(&enrollment.backup_codes[0]));
    let disabled = service
        .disable_two_factor(cookie(&recovered.set_cookie), "changed-password-123")
        .await
        .unwrap();
    assert!(!disabled.principal.two_factor_enabled());
    assert!(!disabled.principal.can_access());
    assert!(
        service
            .authenticate_cookie(cookie(&recovered.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    db.close().await.unwrap();
}
