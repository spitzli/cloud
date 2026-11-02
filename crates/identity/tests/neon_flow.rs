//! Opt-in, synthetic identities on the explicitly owned disposable Neon qualification branch.
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use serde::Deserialize;
use sha2::Digest;
use std::{collections::BTreeMap, sync::Arc};
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webdock_identity::{
    CookiePolicy, IdentityService, MfaProof, NeonAuthClient, PasswordAuthentication, RlsSigner,
    SecretKeys,
};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    branch_id: String,
    owner_url: String,
    bootstrap_url: String,
    runtime_url: String,
    provider_url: String,
    auth_secret: String,
    rls_secret: String,
}
fn cookie(value: &str) -> &str {
    value.split(';').next().unwrap()
}
// Remote integration deliberately exercises multiple logins; honor provider throttling without weakening it.
async fn paced_signin(
    service: &IdentityService,
    email: &str,
    password: &str,
) -> webdock_identity::Result<PasswordAuthentication> {
    for _ in 0..7 {
        match service.authenticate_password(email, password).await {
            Err(webdock_identity::IdentityError::RateLimited) => {
                tokio::time::sleep(std::time::Duration::from_secs(11)).await
            }
            result => return result,
        }
    }
    Err(webdock_identity::IdentityError::RateLimited)
}
#[tokio::test]
#[ignore = "requires owned Neon child fixture; creates synthetic account, never production"]
async fn managed_first_factor_requires_mfa_and_revokes_native_and_rls_authority() {
    let path = std::env::var("WEBDOCK_NEON_AUTH_TEST_FIXTURE").unwrap();
    let f: Fixture = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(f.branch_id, "br-round-star-b9gnbe89");
    let url = url::Url::parse(&f.owner_url).unwrap();
    assert!(url.host_str().unwrap().ends_with(".neon.tech"));
    assert_eq!(url.path(), "/neondb");
    assert_eq!(url.query(), Some("sslmode=verify-full"));
    for value in [&f.bootstrap_url, &f.runtime_url] {
        let parsed = url::Url::parse(value).unwrap();
        assert_eq!(parsed.host_str(), url.host_str());
        assert_eq!(parsed.path(), url.path());
    }
    let owner = Database::connect(&f.owner_url).await.unwrap();
    let auth = Database::connect(&f.bootstrap_url).await.unwrap();
    let runtime = Database::connect(&f.runtime_url).await.unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &f.bootstrap_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let keys = || {
        SecretKeys::new(
            BTreeMap::from([(1, f.auth_secret.clone())]),
            1,
            Some(f.auth_secret.clone()),
        )
        .unwrap()
    };
    let service = IdentityService::new(auth, CookiePolicy::new(true, &f.auth_secret).unwrap())
        .with_allocator(allocator.clone())
        .with_secret_keys(keys())
        .with_neon_auth(NeonAuthClient::new(&f.provider_url, "https://auth.webdock.dev").unwrap())
        .with_passkeys(url::Url::parse("https://auth.webdock.dev").unwrap())
        .unwrap();
    let email = format!(
        "neon-operator-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let initial = "owned-native-password-9384";
    let password = "owned-neon-password-48273";
    let id = service
        .create_local_account(&email, "Owned Neon operator", initial, true)
        .await
        .unwrap();
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = paced_signin(&service, &email, initial).await.unwrap()
    else {
        panic!("native setup expected")
    };
    let setup = service
        .change_password(cookie(&setup.set_cookie), initial, password)
        .await
        .unwrap();
    let enrollment = service
        .begin_totp_enrollment(cookie(&setup.set_cookie), password)
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let native = service
        .confirm_totp_enrollment(
            cookie(&setup.set_cookie),
            &totp.generate_current().to_string(),
        )
        .await
        .unwrap();
    assert!(native.principal.is_operator());
    let before = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT secret FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "secret")
        .unwrap();

    let origin = url::Url::parse("https://auth.webdock.dev").unwrap();
    let register = service
        .begin_passkey_registration(cookie(&native.set_cookie))
        .await
        .unwrap();
    let mut options = register.options;
    // Only the maintained software test authenticator's client-side resident-key selection is relaxed.
    // Production options still require resident credentials and verified users.
    let selection = options.public_key.authenticator_selection.as_mut().unwrap();
    assert!(selection.require_resident_key);
    selection.require_resident_key = false;
    selection.resident_key = None;
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let registration = authenticator
        .do_registration(origin.clone(), options)
        .unwrap();
    let credential_id = registration.raw_id.clone();
    service
        .finish_passkey_registration(
            &format!(
                "{}; {}",
                cookie(&native.set_cookie),
                cookie(&register.set_cookie)
            ),
            &registration,
            Some("Retained qualification key"),
        )
        .await
        .unwrap();
    service
        .link_neon_identity(cookie(&native.set_cookie), password)
        .await
        .unwrap();
    let mapped = service
        .authenticate_cookie(cookie(&native.set_cookie))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mapped.user_id, id);
    assert!(mapped.is_operator());
    let after = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT secret FROM webdock_auth.\"twoFactor\" WHERE \"userId\"=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "secret")
        .unwrap();
    assert_eq!(before, after, "factor ciphertext must be retained");

    let ceremony = service.begin_passkey_authentication().await.unwrap();
    let mut options = ceremony.options;
    options.public_key.allow_credentials = vec![webauthn_rs_core::proto::AllowCredentials {
        type_: "public-key".into(),
        id: credential_id.clone(),
        transports: None,
    }];
    options.mediation = None;
    let assertion = authenticator
        .do_authentication(origin.clone(), options)
        .unwrap();
    assert!(
        service
            .finish_passkey_authentication(cookie(&ceremony.set_cookie), &assertion)
            .await
            .is_err(),
        "Mapped passkey bypassed Managed first factor"
    );
    let PasswordAuthentication::MfaRequired {
        set_cookie: pk_first,
        ..
    } = paced_signin(&service, &email, password).await.unwrap()
    else {
        panic!("MFA required before passkey step-up")
    };
    let ceremony = service.begin_passkey_authentication().await.unwrap();
    let mut options = ceremony.options;
    options.public_key.allow_credentials = vec![webauthn_rs_core::proto::AllowCredentials {
        type_: "public-key".into(),
        id: credential_id,
        transports: None,
    }];
    options.mediation = None;
    let assertion = authenticator
        .do_authentication(origin.clone(), options)
        .unwrap();
    let passkey_session = service
        .finish_passkey_authentication(
            &format!("{}; {}", cookie(&pk_first), cookie(&ceremony.set_cookie)),
            &assertion,
        )
        .await
        .unwrap();
    assert!(passkey_session.principal.is_operator());
    service
        .logout(cookie(&passkey_session.set_cookie))
        .await
        .unwrap();
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge,
        ..
    } = paced_signin(&service, &email, password).await.unwrap()
    else {
        panic!("Managed first factor alone granted a session")
    };
    assert!(
        service
            .authenticate_cookie(cookie(&challenge))
            .await
            .unwrap()
            .is_none()
    );
    let mut full = service
        .complete_mfa(
            cookie(&challenge),
            MfaProof::RecoveryCode(enrollment.backup_codes[0].clone()),
        )
        .await
        .unwrap();
    assert_eq!(full.principal.user_id, id);
    assert!(full.principal.is_operator());
    let signer = RlsSigner::new(&f.rls_secret, "webdock_auth_runtime").unwrap();
    let scoped = signer.begin(&runtime, &full.principal).await.unwrap();
    assert!(
        scoped
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS allowed"
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "allowed")
            .unwrap()
    );
    scoped.rollback().await.unwrap();
    // A forged native row/cookie has no Managed/MFA provenance and must fail in both resolvers.
    let token = hex::encode(sha2::Sha256::digest(email.as_bytes()));
    let forged = allocator.allocate().await.unwrap().to_string();
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())",[forged.clone().into(),token.clone().into(),id.clone().into()])).await.unwrap();
    let forged_cookie = CookiePolicy::new(true, &f.auth_secret)
        .unwrap()
        .header(&token, 3600)
        .unwrap();
    assert!(
        service
            .authenticate_cookie(cookie(&forged_cookie))
            .await
            .unwrap()
            .is_none()
    );
    // Even valid first-factor provenance cannot authorize an operator without an explicit MFA marker.
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.neon_session(session_id,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,mfa_verified) SELECT $1,user_id,provider_session_id,provider_subject,identity_revision,credential_digest,encrypted_cookie,false FROM webdock_auth.neon_session WHERE session_id=$2",[forged.clone().into(),full.principal.session_id.clone().into()])).await.unwrap();
    assert!(
        service
            .authenticate_cookie(cookie(&forged_cookie))
            .await
            .unwrap()
            .is_none()
    );
    let actual_session = full.principal.session_id.clone();
    full.principal.session_id = forged.clone();
    let scoped = signer.begin(&runtime, &full.principal).await.unwrap();
    assert!(
        !scoped
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS allowed"
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "allowed")
            .unwrap()
    );
    scoped.rollback().await.unwrap();
    full.principal.session_id = actual_session;
    assert!(
        runtime
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.actor_before_neon()"
            ))
            .await
            .is_err(),
        "Unwrapped actor helper was executable by runtime role"
    );
    // Revoke only at the real Managed endpoint; the native row deliberately remains.
    let provenance=owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT encrypted_cookie,provider_session_id::text FROM webdock_auth.neon_session WHERE session_id=$1",[full.principal.session_id.clone().into()])).await.unwrap().unwrap();
    let envelope: serde_json::Value = serde_json::from_str(
        &keys()
            .decrypt(
                &provenance
                    .try_get::<String>("", "encrypted_cookie")
                    .unwrap(),
            )
            .unwrap(),
    )
    .unwrap();
    let response = reqwest::Client::new()
        .post(format!("{}/sign-out", f.provider_url))
        .header("origin", "https://auth.webdock.dev")
        .header("cookie", envelope["cookie"].as_str().unwrap())
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        service
            .authenticate_cookie(cookie(&full.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    let check = owner.begin().await.unwrap();
    assert!(
        service
            .authenticate_session_in(&check, &id, &full.principal.session_id)
            .await
            .unwrap()
            .is_none(),
        "Session revalidation used by OAuth/browser authority ignored provider logout"
    );
    check.rollback().await.unwrap();
    let scoped = signer.begin(&runtime, &full.principal).await.unwrap();
    assert!(
        !scoped
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT webdock_rls.operator() AS allowed"
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "allowed")
            .unwrap()
    );
    scoped.rollback().await.unwrap();
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge,
        ..
    } = paced_signin(&service, &email, password).await.unwrap()
    else {
        panic!("MFA required after reauthentication")
    };
    assert!(
        service
            .complete_mfa(
                cookie(&challenge),
                MfaProof::RecoveryCode(enrollment.backup_codes[0].clone())
            )
            .await
            .is_err(),
        "recovery code replay accepted"
    );
    let next = service
        .complete_mfa(
            cookie(&challenge),
            MfaProof::RecoveryCode(enrollment.backup_codes[1].clone()),
        )
        .await
        .unwrap();
    let replacement = "owned-managed-replacement-28374";
    assert!(
        service
            .change_managed_password(cookie(&next.set_cookie), password, replacement)
            .await
            .unwrap()
    );
    assert!(
        service
            .authenticate_cookie(cookie(&next.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        paced_signin(&service, &email, password).await,
        Err(webdock_identity::IdentityError::Unauthorized)
    ));
    // Respect Managed Auth's real endpoint throttle after the deliberate wrong-password probe.
    tokio::time::sleep(std::time::Duration::from_secs(11)).await;
    let PasswordAuthentication::MfaRequired {
        set_cookie: challenge,
        ..
    } = paced_signin(&service, &email, replacement).await.unwrap()
    else {
        panic!("Managed password change not authoritative")
    };
    let last = service
        .complete_mfa(
            cookie(&challenge),
            MfaProof::RecoveryCode(enrollment.backup_codes[2].clone()),
        )
        .await
        .unwrap();
    assert!(last.principal.is_operator());
    service.logout(cookie(&last.set_cookie)).await.unwrap();
    assert!(
        service
            .authenticate_cookie(cookie(&last.set_cookie))
            .await
            .unwrap()
            .is_none()
    );
    if std::env::var("WEBDOCK_NEON_KEEP_SYNTHETIC_FIXTURE").as_deref() == Ok("true") {
        let path = std::path::Path::new(&std::env::var("WEBDOCK_NEON_AUTH_TEST_FIXTURE").unwrap())
            .parent()
            .unwrap()
            .join("owned-neon-browser-operator.private.json");
        let data = serde_json::json!({"email":email,"password":replacement,"userID":id,"totpURI":enrollment.totp_uri,"remainingBackupCodes":&enrollment.backup_codes[3..]});
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(serde_json::to_string(&data).unwrap().as_bytes())
                .unwrap();
        }
        println!(
            "Owned synthetic qualification fixture retained privately for actual image verification"
        );
        owner.close().await.unwrap();
        runtime.close().await.unwrap();
        return;
    }
    let subject = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT provider_subject::text FROM webdock_auth.neon_identity WHERE user_id=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "provider_subject")
        .unwrap();
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.session WHERE \"userId\"=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap();
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.neon_identity WHERE user_id=$1",
            [id.clone().into()],
        ))
        .await
        .unwrap();
    owner
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
            [id.into()],
        ))
        .await
        .unwrap();
    let deleted = std::process::Command::new("neon")
        .args([
            "neon-auth",
            "user",
            "delete",
            &subject,
            "--branch",
            "operator-auth-qualify-20261031",
        ])
        .output()
        .unwrap();
    assert!(
        deleted.status.success(),
        "Owned Managed test user cleanup failed"
    );
    println!(
        "Real Managed first factor + Core MFA/passkey, retained ID/factor, forged-cookie denial, SQL-RLS revocation, recovery replay denial, provider password change and logout passed on owned Neon child"
    );
    owner.close().await.unwrap();
    runtime.close().await.unwrap();
}

#[tokio::test]
#[ignore = "owned child-only fixture administration for actual immutable image qualification"]
async fn prepare_owned_image_operator_registry_mapping() {
    let path = std::env::var("WEBDOCK_NEON_AUTH_TEST_FIXTURE").unwrap();
    let file = std::path::Path::new(&path);
    let f: Fixture = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(f.branch_id, "br-round-star-b9gnbe89");
    let owned: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            file.parent()
                .unwrap()
                .join("owned-neon-browser-operator.private.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let email = owned["email"].as_str().unwrap();
    assert!(email.starts_with("neon-operator-") && email.ends_with("@example.invalid"));
    let user = owned["userID"].as_str().unwrap();
    let owner = Database::connect(&f.owner_url).await.unwrap();
    let allocator = webdock_persistence::IdAllocator::connect(
        &f.bootstrap_url,
        webdock_ids::Identity::platform(0, 2).unwrap(),
    )
    .await
    .unwrap();
    let native = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"user\" WHERE id=$1 AND email=$2 AND role='operator'",
            [user.into(), email.into()],
        ))
        .await
        .unwrap();
    assert!(native.is_some());
    owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Owned image fixture',$2,'operator',$3) ON CONFLICT(auth_subject) DO NOTHING",[allocator.allocate().await.unwrap().to_string().into(),email.into(),user.into()])).await.unwrap();
    owner.close().await.unwrap();
    println!(
        "Owned synthetic image fixture has the ordinary mapped operator registry record; no production changes"
    );
}
