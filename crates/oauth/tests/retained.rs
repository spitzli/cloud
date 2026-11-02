use sea_orm::{ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{collections::BTreeMap, sync::Arc};
use webdock_identity::{CookiePolicy, IdentityService, PasswordAuthentication, SecretKeys};
use webdock_oauth::*;
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
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
fn code(location: &str) -> String {
    url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned()
}

// Only called by the ignored, genuinely issued OAuth fixture below. No fabricated
// session or role is used to exercise MCP locking, reload or transaction lifetime.
async fn mcp_identity_before_access<T>(
    db: &sea_orm::DatabaseConnection,
    owner: &sea_orm::DatabaseConnection,
    subject: &str,
    token_hash: &str,
    new_expiry: Option<i64>,
    verify: impl std::future::Future<Output = webdock_oauth::Result<T>>,
) -> T {
    let identity = db.begin().await.unwrap();
    identity
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"user\" WHERE id=$1 FOR UPDATE",
            [subject.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let identity_pid: i32 = identity
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    tokio::pin!(verify);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let observed = owner.query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.datname=current_database() AND $1::integer=ANY(pg_blocking_pids(a.pid))) AS blocked",
                [identity_pid.into()],
            ));
            tokio::select! {
                _ = &mut verify => panic!("MCP verification must wait for native identity"),
                row = observed => {
                    if row.unwrap().unwrap().try_get::<bool>("", "blocked").unwrap() {
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("MCP identity lock must be observed");
    let probe = owner.begin().await.unwrap();
    let available = probe
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"oauthAccessToken\" WHERE token=$1 FOR UPDATE NOWAIT",
            [token_hash.into()],
        ))
        .await;
    assert!(
        available.unwrap().is_some(),
        "MCP must not hold the access row while waiting for native identity"
    );
    if let Some(expiry) = new_expiry {
        probe
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=$2 WHERE token=$1",
                [
                    token_hash.into(),
                    chrono::DateTime::from_timestamp(expiry, 0).unwrap().into(),
                ],
            ))
            .await
            .unwrap();
    }
    probe.commit().await.unwrap();
    identity.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), verify)
        .await
        .expect("MCP verification must finish after identity release")
        .unwrap()
}
#[tokio::test]
#[ignore = "requires owned retained-schema restricted PostgreSQL fixture"]
async fn real_oidc_pkce_consent_refresh_and_mcp_authority() {
    let auth_url = guarded("AUTH_DATABASE_URL");
    let offline = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0001_oauth_authorization.sql"))
        .await
        .unwrap();
    let db = Database::connect(auth_url.clone()).await.unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let keyring = || {
        SecretKeys::new(
            BTreeMap::from([(0, secret.clone())]),
            0,
            Some(secret.clone()),
        )
        .unwrap()
    };
    let identity = Arc::new(
        IdentityService::new(
            db.clone(),
            CookiePolicy::new(false, secret.clone()).unwrap(),
        )
        .with_secret_keys(keyring())
        .with_allocator(allocator.clone()),
    );
    let config = OAuthConfig::new(
        "https://auth.example.invalid/api/auth",
        "https://studio.example.invalid/api/mcp",
        None,
    )
    .unwrap();
    let oauth = OAuthService::new(
        db.clone(),
        identity.clone(),
        allocator.clone(),
        Arc::new(keyring()),
        config,
    );
    let signing_id = oauth.provision_signing_key(&offline).await.unwrap();
    let id = identity
        .create_local_account(
            &format!(
                "oauth-{}@example.invalid",
                allocator.allocate().await.unwrap()
            ),
            "OAuth fixture",
            "initial-password-123",
            true,
        )
        .await
        .unwrap();
    let user = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1 RETURNING email",
            [id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let email: String = user.try_get("", "email").unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = identity
        .authenticate_password(&email, "initial-password-123")
        .await
        .unwrap()
    else {
        panic!("setup")
    };
    let changed = identity
        .change_password(
            cookie(&setup.set_cookie),
            "initial-password-123",
            "changed-password-123",
        )
        .await
        .unwrap();
    let enrollment = identity
        .begin_totp_enrollment(cookie(&changed.set_cookie), "changed-password-123")
        .await
        .unwrap();
    let device = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let session = identity
        .confirm_totp_enrollment(
            cookie(&changed.set_cookie),
            &device.generate_current().to_string(),
        )
        .await
        .unwrap();
    let client = oauth
        .provision_client(
            &offline,
            ClientRegistration {
                name: "OAuth fixture".into(),
                redirect_uris: vec!["https://client.example.invalid/callback".into()],
                post_logout_redirect_uris: vec!["https://client.example.invalid/signed-out".into()],
                scopes: vec![
                    "openid".into(),
                    "profile".into(),
                    "email".into(),
                    "webdock:read".into(),
                    "offline_access".into(),
                ],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: true,
                skip_consent: false,
                purpose: ClientPurpose::McpOperator,
            },
        )
        .await
        .unwrap();
    let credentials = || ClientAuthentication {
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
        method: ClientAuthMethod::ClientSecretPost,
    };
    let verifier = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-._~";
    let challenge = oauth2::PkceCodeChallenge::from_code_verifier_sha256(
        &oauth2::PkceCodeVerifier::new(verifier.into()),
    )
    .as_str()
    .to_owned();
    let mut request = AuthorizationRequest::code(
        client.client_id.clone(),
        "https://client.example.invalid/callback".into(),
        "openid profile email webdock:read offline_access".into(),
        challenge,
    );
    request.nonce = Some("fixture-nonce".into());
    request.state = Some("fixture-state".into());
    request.resources = vec!["https://studio.example.invalid/api/mcp".into()];
    let AuthorizationOutcome::ConsentRequired(consent) = oauth
        .authorize(cookie(&session.set_cookie), request.clone())
        .await
        .unwrap()
    else {
        panic!("consent required")
    };
    let AuthorizationOutcome::Redirect(location) = oauth
        .consent(cookie(&session.set_cookie), &consent.request_id, true)
        .await
        .unwrap()
    else {
        panic!("authorization redirect")
    };
    assert!(location.contains("state=fixture-state"));
    let access = oauth
        .exchange_code(CodeExchange {
            client: credentials(),
            code: code(&location),
            redirect_uri: request.redirect_uri.clone(),
            code_verifier: verifier.into(),
        })
        .await
        .unwrap();
    assert_eq!(access.token_type, "Bearer");
    assert_eq!(access.expires_in, 300);
    assert!(access.refresh_token.is_some());
    let jwks = oauth.jwks().await.unwrap();
    let jwk = josekit::jwk::Jwk::from_map(
        jwks["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kid"] == signing_id)
            .unwrap()
            .as_object()
            .unwrap()
            .clone(),
    )
    .unwrap();
    let jwt = access.id_token.as_ref().unwrap();
    let (payload, header) = josekit::jws::deserialize_compact(
        jwt,
        &josekit::jws::RS256.verifier_from_jwk(&jwk).unwrap(),
    )
    .unwrap();
    assert_eq!(header.algorithm(), Some("RS256"));
    let claims: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(claims["nonce"], "fixture-nonce");
    assert_eq!(claims["aud"], client.client_id);
    assert_eq!(claims["sub"], id);
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        300
    );
    assert!(
        oauth
            .exchange_code(CodeExchange {
                client: credentials(),
                code: code(&location),
                redirect_uri: request.redirect_uri.clone(),
                code_verifier: verifier.into()
            })
            .await
            .is_err()
    );
    assert_eq!(
        oauth.introspect(&credentials(), jwt).await.unwrap()["active"],
        false,
        "ID tokens must not become bearer access tokens"
    );
    let encrypted: String = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT \"privateKey\" FROM webdock_auth.jwks WHERE id=$1",
            [signing_id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "privateKey")
        .unwrap();
    let encrypted: String = serde_json::from_str(&encrypted).unwrap();
    let key =
        josekit::jwk::Jwk::from_bytes(keyring().decrypt(&encrypted).unwrap().as_bytes()).unwrap();
    let mut legacy_header = josekit::jws::JwsHeader::new();
    legacy_header.set_algorithm("RS256");
    legacy_header.set_key_id(&signing_id);
    let legacy_claims = serde_json::json!({"iss":"https://auth.example.invalid/api/auth","aud":"https://studio.example.invalid/api/mcp","azp":client.client_id,"sub":id,"sid":session.principal.session_id,"scope":"webdock:read","iat":chrono::Utc::now().timestamp(),"exp":chrono::Utc::now().timestamp()+300});
    let legacy_jwt = josekit::jws::serialize_compact(
        &serde_json::to_vec(&legacy_claims).unwrap(),
        &legacy_header,
        &josekit::jws::RS256.signer_from_jwk(&key).unwrap(),
    )
    .unwrap();
    assert_eq!(
        oauth
            .introspect_mcp(&credentials(), &legacy_jwt)
            .await
            .unwrap()["active"],
        true
    );
    oauth.revoke(&credentials(), &legacy_jwt).await.unwrap();
    assert_eq!(
        oauth
            .introspect_mcp(&credentials(), &legacy_jwt)
            .await
            .unwrap()["active"],
        false
    );
    let active = oauth
        .introspect(&credentials(), &access.access_token)
        .await
        .unwrap();
    assert_eq!(active["active"], true);
    assert_eq!(active["webdock_role"], "operator");
    assert!(
        active.get("webdock_email").is_none(),
        "MCP scope does not add profile claims"
    );
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
        [id.clone().into()],
    ))
    .await
    .unwrap();
    assert_eq!(
        oauth
            .introspect_mcp(&credentials(), &access.access_token)
            .await
            .unwrap()["active"],
        false
    );
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
        [id.clone().into()],
    ))
    .await
    .unwrap();
    assert_eq!(
        oauth
            .introspect_mcp(&credentials(), &access.access_token)
            .await
            .unwrap()["active"],
        true
    );
    assert!(
        oauth
            .authorize_mcp(&credentials(), &access.access_token, "webdock:read")
            .await
            .is_ok()
    );
    let mcp_credentials = credentials();
    let hash = legacy_token_hash(&access.access_token);
    let reloaded_expiry = active["exp"].as_i64().unwrap() - 1;
    let mcp = mcp_identity_before_access(
        &db,
        &offline,
        &id,
        &hash,
        Some(reloaded_expiry),
        oauth.authorize_mcp_reference(&mcp_credentials, &access.access_token, "webdock:read"),
    )
    .await;
    assert_eq!(mcp.authority.expires_at(), reloaded_expiry);
    assert!(!mcp.authority.can_write());
    let stored = serde_json::to_string(&mcp.authority).unwrap();
    assert!(!stored.contains(&access.access_token));
    let transaction = db.begin().await.unwrap();
    mcp_identity_before_access(
        &db,
        &offline,
        &id,
        &hash,
        None,
        oauth.revalidate_mcp_authority_in(&transaction, &mcp.authority),
    )
    .await;
    let probe = offline.begin().await.unwrap();
    let held = probe
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"oauthAccessToken\" WHERE token=$1 FOR UPDATE NOWAIT",
            [hash.into()],
        ))
        .await;
    let Err(sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(error))) = held else {
        panic!("The actual MCP verification transaction must retain its access lock");
    };
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03"),
        "Only lock_not_available proves retention; other database failures do not"
    );
    probe.rollback().await.unwrap();
    transaction.commit().await.unwrap();
    let mut tampered = serde_json::to_value(&mcp.authority).unwrap();
    tampered["subject"] = serde_json::json!("other-subject");
    let tampered: McpAuthority = serde_json::from_value(tampered).unwrap();
    let transaction = db.begin().await.unwrap();
    assert!(
        oauth
            .revalidate_mcp_authority_in(&transaction, &tampered)
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    assert!(
        oauth
            .authorize_mcp(&credentials(), &access.access_token, "webdock:write")
            .await
            .is_err()
    );
    assert!(
        oauth
            .authorize_mcp(
                &credentials(),
                access.refresh_token.as_ref().unwrap(),
                "webdock:read"
            )
            .await
            .is_err()
    );
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=now()-interval '1 second' WHERE token=$1",[legacy_token_hash(&access.access_token).into()])).await.unwrap();
    let transaction = db.begin().await.unwrap();
    assert!(
        oauth
            .revalidate_mcp_authority_in(&transaction, &mcp.authority)
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    assert_eq!(
        oauth
            .introspect(&credentials(), &access.access_token)
            .await
            .unwrap()["active"],
        false
    );
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"oauthAccessToken\" SET \"expiresAt\"=now()+interval '5 minutes' WHERE token=$1",[legacy_token_hash(&access.access_token).into()])).await.unwrap();
    let refreshed = oauth
        .refresh(RefreshExchange {
            client: credentials(),
            refresh_token: access.refresh_token.clone().unwrap(),
            scope: None,
        })
        .await
        .unwrap();
    assert_eq!(
        oauth
            .introspect(&credentials(), &refreshed.access_token)
            .await
            .unwrap()["active"],
        true
    );
    assert!(
        oauth
            .refresh(RefreshExchange {
                client: credentials(),
                refresh_token: access.refresh_token.unwrap(),
                scope: None
            })
            .await
            .is_err()
    );
    assert_eq!(
        oauth
            .introspect(&credentials(), &refreshed.access_token)
            .await
            .unwrap()["active"],
        false,
        "refresh replay revokes the family"
    );
    let AuthorizationOutcome::Redirect(expired_code) = oauth
        .authorize(cookie(&session.set_cookie), request.clone())
        .await
        .unwrap()
    else {
        panic!("code")
    };
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.identity_oauth_request SET expires_at=now()-interval '1 second' WHERE client_id=$1 AND kind='code'",[client.client_id.clone().into()])).await.unwrap();
    assert!(
        oauth
            .exchange_code(CodeExchange {
                client: credentials(),
                code: code(&expired_code),
                redirect_uri: request.redirect_uri.clone(),
                code_verifier: verifier.into()
            })
            .await
            .is_err()
    );
    let mut invalid = request.clone();
    invalid.redirect_uri = "https://attacker.example.invalid/callback".into();
    assert!(
        oauth
            .authorize(cookie(&session.set_cookie), invalid)
            .await
            .is_err()
    );
    let AuthorizationOutcome::Redirect(location) = oauth
        .authorize(cookie(&session.set_cookie), request.clone())
        .await
        .unwrap()
    else {
        panic!("saved consent")
    };
    assert!(
        oauth
            .exchange_code(CodeExchange {
                client: credentials(),
                code: code(&location),
                redirect_uri: request.redirect_uri.clone(),
                code_verifier: "wrong".repeat(12)
            })
            .await
            .is_err()
    );
    let next = oauth
        .exchange_code(CodeExchange {
            client: credentials(),
            code: code(&location),
            redirect_uri: request.redirect_uri.clone(),
            code_verifier: verifier.into(),
        })
        .await
        .unwrap();
    assert!(
        oauth
            .end_session(
                cookie(&session.set_cookie),
                LogoutRequest {
                    id_token_hint: next.id_token.clone().unwrap(),
                    post_logout_redirect_uri: Some("https://attacker.example.invalid".into()),
                    state: None
                }
            )
            .await
            .is_err()
    );
    let LogoutOutcome::Complete {
        redirect,
        set_cookie,
    } = oauth
        .end_session(
            cookie(&session.set_cookie),
            LogoutRequest {
                id_token_hint: next.id_token.clone().unwrap(),
                post_logout_redirect_uri: Some("https://client.example.invalid/signed-out".into()),
                state: Some("logout-state".into()),
            },
        )
        .await
        .unwrap()
    else {
        panic!("logout")
    };
    assert!(redirect.unwrap().contains("state=logout-state"));
    assert!(set_cookie.contains("Max-Age=0"));
    assert_eq!(
        oauth
            .introspect(&credentials(), &next.access_token)
            .await
            .unwrap()["active"],
        false
    );
    assert!(
        oauth
            .refresh(RefreshExchange {
                client: credentials(),
                refresh_token: next.refresh_token.unwrap(),
                scope: None
            })
            .await
            .is_err(),
        "NULL session FK cannot refresh into sessionless access"
    );
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
            [client.client_id.into()],
        ))
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        [id.into()],
    ))
    .await
    .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.jwks WHERE id=$1",
            [signing_id.into()],
        ))
        .await
        .unwrap();
    db.close().await.unwrap();
    offline.close().await.unwrap();
}
