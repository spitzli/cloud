use sea_orm::{ConnectOptions, ConnectionTrait, Database, DbBackend, Statement, TransactionTrait};
use std::{collections::BTreeMap, sync::Arc};
use webdock_browser_session::*;
use webdock_identity::{CookiePolicy, IdentityService, PasswordAuthentication, SecretKeys};
use webdock_oauth::*;
fn guarded(key: &str) -> String {
    let raw = std::env::var(key).unwrap();
    let u = url::Url::parse(&raw).unwrap();
    assert!(["postgres", "postgresql"].contains(&u.scheme()));
    assert!([Some("localhost"), Some("127.0.0.1"), Some("[::1]")].contains(&u.host_str()));
    assert_eq!(u.path(), "/webdock_rls_test");
    assert!(u.query().is_none() && u.fragment().is_none());
    raw
}
fn cookie(header: &str) -> &str {
    header.split(';').next().unwrap()
}
fn request(location: &str) -> AuthorizationRequest {
    let u = url::Url::parse(location).unwrap();
    let v = u.query_pairs().collect::<std::collections::HashMap<_, _>>();
    AuthorizationRequest {
        client_id: v["client_id"].to_string(),
        redirect_uri: v["redirect_uri"].to_string(),
        response_type: "code".into(),
        scope: v["scope"].to_string(),
        state: Some(v["state"].to_string()),
        nonce: Some(v["nonce"].to_string()),
        code_challenge: v["code_challenge"].to_string(),
        code_challenge_method: "S256".into(),
        prompt: None,
        max_age: None,
        resources: vec![],
    }
}
#[tokio::test]
#[ignore = "requires owned restricted local fixture"]
async fn confidential_browser_handoff_is_one_use_origin_bound_and_live() {
    let offline = Database::connect(guarded("WEBDOCK_TEST_MIGRATION_DATABASE_URL"))
        .await
        .unwrap();
    offline
        .execute_unprepared(include_str!("../migrations/0001_browser_sessions.sql"))
        .await
        .unwrap();
    let auth_url = guarded("AUTH_DATABASE_URL");
    let mut options = ConnectOptions::new(auth_url.clone());
    options
        .max_connections(2)
        .min_connections(0)
        .acquire_timeout(std::time::Duration::from_millis(750));
    let db = Database::connect(options).await.unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            &auth_url,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
    let keys = Arc::new(
        SecretKeys::new(
            BTreeMap::from([(1, secret.clone())]),
            1,
            Some(secret.clone()),
        )
        .unwrap(),
    );
    let identity = Arc::new(
        IdentityService::new(db.clone(), CookiePolicy::new(true, secret.clone()).unwrap())
            .with_allocator(allocator.clone())
            .with_secret_keys(
                SecretKeys::new(
                    BTreeMap::from([(1, secret.clone())]),
                    1,
                    Some(secret.clone()),
                )
                .unwrap(),
            ),
    );
    let bootstrap = OAuthService::new(
        db.clone(),
        identity.clone(),
        allocator.clone(),
        keys.clone(),
        OAuthConfig::new(
            "https://auth.example.invalid/api/auth",
            "https://studio.example.invalid/api/mcp",
            None,
        )
        .unwrap(),
    );
    let signing = bootstrap.provision_signing_key(&offline).await.unwrap();
    let client = bootstrap
        .provision_client(
            &offline,
            ClientRegistration {
                name: "Browser fixture".into(),
                redirect_uris: vec![
                    "https://console.example.invalid/api/sso/callback".into(),
                    "https://studio.example.invalid/api/sso/callback".into(),
                ],
                post_logout_redirect_uris: vec!["https://console.example.invalid/login".into()],
                scopes: vec!["openid".into(), "profile".into(), "email".into()],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: true,
                skip_consent: true,
                purpose: ClientPurpose::Portal,
            },
        )
        .await
        .unwrap();
    let oauth = Arc::new(OAuthService::new(
        db.clone(),
        identity.clone(),
        allocator.clone(),
        keys.clone(),
        OAuthConfig::new(
            "https://auth.example.invalid/api/auth",
            "https://studio.example.invalid/api/mcp",
            Some(client.client_id.clone()),
        )
        .unwrap(),
    ));
    let config = BrowserConfig::new(
        "https://console.example.invalid",
        "https://auth.example.invalid/api/auth",
        client.client_id.clone(),
        client.client_secret.clone().unwrap(),
    )
    .unwrap()
    .with_additional_origins(&["https://studio.example.invalid".into()])
    .unwrap();
    let browser = BrowserSessions::new(
        db.clone(),
        identity.clone(),
        oauth.clone(),
        keys.clone(),
        &secret,
        config,
    )
    .unwrap()
    .with_legacy(
        LegacyCookies::new(
            std::collections::HashMap::from([("1".into(), secret.clone())]),
            chrono::Utc::now().timestamp() + 600,
        )
        .unwrap(),
    );
    let email = format!(
        "browser-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let user = identity
        .create_local_account(&email, "Browser fixture", "fixture-password-123", false)
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",[user.clone().into()])).await.unwrap();
    let customer = allocator.allocate().await.unwrap().to_string();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "WITH hint AS MATERIALIZED (SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id',$1::text,'organization_id',$2::text)::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT $1,'Browser fixture' FROM hint",
            [customer.clone().into(),allocator.allocate().await.unwrap().to_string().into()],
        ))
        .await
        .unwrap();
    let org: String = offline
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [customer.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "organization_id")
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,$3,'member',now())",[allocator.allocate().await.unwrap().to_string().into(),org.clone().into(),user.clone().into()])).await.unwrap();
    let PasswordAuthentication::Authenticated(auth) = identity
        .authenticate_password(&email, "fixture-password-123")
        .await
        .unwrap()
    else {
        panic!("native sign in")
    };
    let bad_nonce = browser
        .begin("https://console.example.invalid", "/projects")
        .await
        .unwrap();
    let mut bad_request = request(&bad_nonce.location);
    let bad_state = bad_request.state.clone().unwrap();
    bad_request.nonce = Some("mismatched-nonce".into());
    let AuthorizationOutcome::Redirect(bad_callback) = oauth
        .authorize(cookie(&auth.set_cookie), bad_request)
        .await
        .unwrap()
    else {
        panic!("authorize")
    };
    let bad_code = url::Url::parse(&bad_callback)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();
    assert!(
        browser
            .callback(
                "https://console.example.invalid",
                cookie(&bad_nonce.set_cookies[0]),
                &bad_code,
                &bad_state
            )
            .await
            .is_err(),
        "ID token nonce must match the bound browser flow"
    );
    let begin = browser
        .begin("https://console.example.invalid", "/projects?view=all")
        .await
        .unwrap();
    let auth_request = request(&begin.location);
    let state = auth_request.state.clone().unwrap();
    let AuthorizationOutcome::Redirect(callback) = oauth
        .authorize(cookie(&auth.set_cookie), auth_request)
        .await
        .unwrap()
    else {
        panic!("authorize")
    };
    let callback = url::Url::parse(&callback).unwrap();
    assert_eq!(
        callback
            .query_pairs()
            .find(|(key, _)| key == "iss")
            .unwrap()
            .1,
        "https://auth.example.invalid/api/auth"
    );
    let code = callback
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();
    assert!(
        browser
            .callback(
                "https://console.example.invalid",
                cookie(&begin.set_cookies[0]),
                &code,
                "wrong-state"
            )
            .await
            .is_err()
    );
    assert!(
        browser
            .callback(
                "https://studio.example.invalid",
                cookie(&begin.set_cookies[0]),
                &code,
                &state
            )
            .await
            .is_err()
    );
    let session = browser
        .callback(
            "https://console.example.invalid",
            cookie(&begin.set_cookies[0]),
            &code,
            &state,
        )
        .await
        .unwrap();
    assert_eq!(
        session.location,
        "https://console.example.invalid/projects?view=all"
    );
    let session_cookie = cookie(&session.set_cookies[0]);
    assert!(!session_cookie.contains("access_token"));
    assert_eq!(
        browser
            .authenticate("https://console.example.invalid", session_cookie)
            .await
            .unwrap()
            .unwrap()
            .user_id,
        user
    );
    assert!(
        browser
            .authenticate("https://studio.example.invalid", session_cookie)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        browser
            .authenticate(
                "https://console.example.invalid",
                &format!("{session_cookie}; {session_cookie}")
            )
            .await
            .is_err()
    );
    assert!(
        browser
            .callback(
                "https://console.example.invalid",
                cookie(&begin.set_cookies[0]),
                &code,
                &state
            )
            .await
            .is_err()
    );
    // The actual retained encrypted cookie remains usable only with live exact-client authority.
    let payload: String = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT payload FROM webdock_auth.identity_browser_session WHERE client_id=$1",
            [client.client_id.clone().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "payload")
        .unwrap();
    let tokens: serde_json::Value = serde_json::from_str(&keys.decrypt(&payload).unwrap()).unwrap();
    let old = serde_json::json!({"kind":"session","accessToken":tokens["access_token"],"sub":user,"exp":chrono::Utc::now().timestamp()+300,"idToken":tokens["id_token"]});
    let options = iron_webcrypto_rs::SealOptions {
        ttl: 300000,
        ..Default::default()
    };
    let old_token = iron_webcrypto_rs::seal(
        &old,
        &iron_webcrypto_rs::Password::WithId {
            id: "1",
            encryption: secret.as_str().into(),
            integrity: secret.as_str().into(),
        },
        &options,
    )
    .unwrap();
    let old_cookie = format!("__Host-webdock-sso={old_token}~2");
    assert_eq!(
        browser
            .authenticate("https://console.example.invalid", &old_cookie)
            .await
            .unwrap()
            .unwrap()
            .user_id,
        user
    );
    let logout = browser
        .logout("https://console.example.invalid", session_cookie)
        .await
        .unwrap();
    assert!(
        browser
            .authenticate("https://console.example.invalid", &old_cookie)
            .await
            .is_err(),
        "local logout revokes its application bearer before global confirmation"
    );
    let logout_request = url::Url::parse(&logout.location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "request")
        .unwrap()
        .1
        .to_string();
    assert!(browser.logout_details("", &logout_request).await.is_err());
    assert_eq!(
        browser
            .logout_details(cookie(&auth.set_cookie), &logout_request)
            .await
            .unwrap()["applicationOrigin"],
        "https://console.example.invalid"
    );
    browser
        .finish_logout(cookie(&auth.set_cookie), &logout_request, true)
        .await
        .unwrap();
    assert!(
        browser
            .finish_logout(cookie(&auth.set_cookie), &logout_request, true)
            .await
            .is_err()
    );
    assert!(
        browser
            .authenticate("https://console.example.invalid", &old_cookie)
            .await
            .is_err(),
        "legacy cookie cannot revive a revoked central session"
    );
    identity
        .revoke_session(&user, &auth.principal.session_id)
        .await
        .unwrap();
    assert!(
        browser
            .authenticate("https://console.example.invalid", session_cookie)
            .await
            .unwrap()
            .is_none()
    );
    let operator_email = format!(
        "browser-operator-{}@example.invalid",
        allocator.allocate().await.unwrap()
    );
    let operator = identity
        .create_local_account(
            &operator_email,
            "Preview fixture",
            "fixture-password-123",
            true,
        )
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true WHERE id=$1",
        [operator.clone().into()],
    ))
    .await
    .unwrap();
    let PasswordAuthentication::SetupRequired {
        session: Some(setup),
        ..
    } = identity
        .authenticate_password(&operator_email, "fixture-password-123")
        .await
        .unwrap()
    else {
        panic!("operator setup")
    };
    let changed = identity
        .change_password(
            cookie(&setup.set_cookie),
            "fixture-password-123",
            "fixture-password-456",
        )
        .await
        .unwrap();
    let enrollment = identity
        .begin_totp_enrollment(cookie(&changed.set_cookie), "fixture-password-456")
        .await
        .unwrap();
    let otp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let op_session = identity
        .confirm_totp_enrollment(
            cookie(&changed.set_cookie),
            &otp.generate_current().to_string(),
        )
        .await
        .unwrap();
    let flow = browser
        .begin("https://console.example.invalid", "/projects")
        .await
        .unwrap();
    let req = request(&flow.location);
    let state = req.state.clone().unwrap();
    let AuthorizationOutcome::Redirect(cb) = oauth
        .authorize(cookie(&op_session.set_cookie), req)
        .await
        .unwrap()
    else {
        panic!("operator authorize")
    };
    let code = url::Url::parse(&cb)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();
    let op_browser = browser
        .callback(
            "https://console.example.invalid",
            cookie(&flow.set_cookies[0]),
            &code,
            &state,
        )
        .await
        .unwrap();
    let business = Database::connect(guarded("DATABASE_URL")).await.unwrap();
    let access = webdock_access::AccessService::new(
        business,
        db.clone(),
        identity.clone(),
        allocator.clone(),
        webdock_identity::RlsSigner::new(
            std::env::var("DATABASE_RLS_SECRET").unwrap(),
            "webdock_auth_runtime",
        )
        .unwrap(),
        keys.clone(),
        webdock_access::AccessConfig {
            auth_origin: url::Url::parse("https://auth.example.invalid").unwrap(),
            console_origins: vec![],
            allow_local_http: false,
        },
    )
    .unwrap();
    let authenticated = browser
        .resolve_authority(
            "https://console.example.invalid",
            cookie(&op_browser.set_cookies[0]),
        )
        .await
        .unwrap()
        .unwrap();
    let authority = authenticated.authority;
    // Both callers hold every pool connection before proof validation starts. No nested acquisition is possible.
    let first = db.begin().await.unwrap();
    let second = db.begin().await.unwrap();
    let stored = StoredAuthority::FirstPartyBrowser {
        browser: authority.clone(),
    };
    let proofs = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(
            revalidate_stored_authority(
                &identity,
                &first,
                Some(&browser),
                &op_session.principal.user_id,
                &op_session.principal.session_id,
                &stored
            ),
            revalidate_stored_authority(
                &identity,
                &second,
                Some(&browser),
                &op_session.principal.user_id,
                &op_session.principal.session_id,
                &stored
            )
        )
    })
    .await;
    for kind in ["expiry", "client", "role", "scope"] {
        let mut invalid = authority.clone();
        match kind {
            "expiry" => invalid.expires_at = chrono::Utc::now().timestamp() - 1,
            "client" => invalid.client_id = "unrelated-client".into(),
            "role" => invalid.role = "reader".into(),
            _ => invalid.scopes = vec!["openid".into()],
        };
        assert!(
            browser.revalidate_in(&first, &invalid).await.is_err(),
            "caller-transaction proof must reject {kind}"
        );
    }
    first.rollback().await.unwrap();
    second.rollback().await.unwrap();
    let (first_proof, second_proof) = proofs.expect("bounded-pool proof validation must finish");
    assert!(
        first_proof.is_ok() && second_proof.is_ok(),
        "proof validation must reuse each caller transaction: {:?}, {:?}",
        first_proof.err(),
        second_proof.err()
    );

    assert!(
        browser
            .revalidate(&authority)
            .await
            .unwrap()
            .principal
            .is_operator()
    );
    let mut narrow = authority.clone();
    narrow.role = "reader".into();
    assert!(browser.revalidate(&narrow).await.is_err());
    let mut narrow = authority.clone();
    narrow.scopes = vec!["openid".into()];
    assert!(browser.revalidate(&narrow).await.is_err());
    let mut narrow = authority.clone();
    narrow.read_only = true;
    assert!(
        browser
            .revalidate(&narrow)
            .await
            .unwrap()
            .authority
            .read_only
    );
    let mut wrong = authority.clone();
    wrong.client_id = "another-client".into();
    assert!(browser.revalidate(&wrong).await.is_err());
    let mut wrong = authority.clone();
    wrong.app_origin = "https://studio.example.invalid".into();
    assert!(browser.revalidate(&wrong).await.is_err());
    access
        .start_preview_as(&op_session.principal, &customer)
        .await
        .unwrap();
    let preview = browser
        .authenticate(
            "https://console.example.invalid",
            cookie(&op_browser.set_cookies[0]),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(preview.in_tenant_preview());
    assert!(!preview.is_operator());
    assert!(
        access
            .manage_access_as(
                &preview,
                webdock_access::AccessCommand::CreateOrganization {
                    name: "Denied".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(access.list_tenants_as(&preview).await.unwrap().len(), 1);
    access.exit_preview_as(&preview).await.unwrap();
    assert!(
        browser
            .authenticate(
                "https://console.example.invalid",
                cookie(&op_browser.set_cookies[0])
            )
            .await
            .unwrap()
            .unwrap()
            .is_operator()
    );
    access
        .start_customer_scope_as(
            &op_session.principal,
            &customer,
            webdock_access::PreviewMode::Owner,
        )
        .await
        .unwrap();
    let acting = browser
        .resolve_authority(
            "https://console.example.invalid",
            cookie(&op_browser.set_cookies[0]),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(acting.principal.is_acting_customer());
    assert!(!acting.principal.is_operator());
    assert!(!acting.authority.read_only);
    assert_eq!(acting.authority.role, "admin");
    let persisted = db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_browser_session WHERE user_id=$1 AND session_id=$2",[operator.clone().into(),acting.principal.session_id.clone().into()])).await.unwrap().unwrap();
    let persisted: serde_json::Value = serde_json::from_str(
        &keys
            .decrypt(&persisted.try_get::<String>("", "payload").unwrap())
            .unwrap(),
    )
    .unwrap();
    let public = oauth
        .introspect(
            &ClientAuthentication {
                client_id: client.client_id.clone(),
                client_secret: client.client_secret.clone(),
                method: ClientAuthMethod::ClientSecretPost,
            },
            persisted["access_token"].as_str().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        public["active"], false,
        "owner acting never grants public OAuth authority"
    );
    access
        .create_project_as(&acting.principal, &customer, "Acting browser owner")
        .await
        .unwrap();
    access.exit_preview_as(&acting.principal).await.unwrap();
    assert!(browser.revalidate(&acting.authority).await.is_err());
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        [operator.into()],
    ))
    .await
    .unwrap();
    for sql in [
        "DELETE FROM webdock_auth.identity_browser_flow WHERE client_id=$1",
        "DELETE FROM webdock_auth.app_binding WHERE client_id=$1",
        "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
    ] {
        offline
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [client.client_id.clone().into()],
            ))
            .await
            .unwrap();
    }
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [customer.clone().into()],
        ))
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
        [user.into()],
    ))
    .await
    .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_admin.projects WHERE customer_id=$1",
            [customer.clone().into()],
        ))
        .await
        .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_admin.customers WHERE id=$1",
            [customer.into()],
        ))
        .await
        .unwrap();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM webdock_auth.organization WHERE id=$1",
        [org.into()],
    ))
    .await
    .unwrap();
    offline
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webdock_auth.jwks WHERE id=$1",
            [signing.into()],
        ))
        .await
        .unwrap();
}
