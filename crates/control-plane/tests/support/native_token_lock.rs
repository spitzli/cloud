//! Ordinary owned-database transaction test for native authority row-lock lifetime.
use super::*;
use sea_orm::TransactionTrait;

pub(super) struct Context<'a> {
    pub app: &'a Router,
    pub owner: &'a DatabaseConnection,
    pub auth: &'a DatabaseConnection,
    pub business: &'a DatabaseConnection,
    pub native: &'a str,
    pub secrets: &'a Value,
    pub keys: &'a webdock_identity::SecretKeys,
    pub browser: &'a webdock_browser_session::BrowserSessions,
    pub oauth: &'a webdock_oauth::OAuthService,
}

async fn console_session(app: &Router, native: &str) -> String {
    let begin = request(
        app,
        CONSOLE,
        "GET",
        "/api/sso/login?returnTo=%2Fprojects",
        "",
        Value::Null,
    )
    .await;
    assert!(begin.status.is_redirection());
    let authorize = begin.location();
    let granted = request(
        app,
        AUTH,
        "GET",
        &format!("{}?{}", authorize.path(), authorize.query().unwrap()),
        native,
        Value::Null,
    )
    .await;
    assert!(granted.status.is_redirection());
    let callback = granted.location();
    let issued = request(
        app,
        CONSOLE,
        "GET",
        &format!("{}?{}", callback.path(), callback.query().unwrap()),
        &begin.session(),
        Value::Null,
    )
    .await;
    assert!(issued.status.is_redirection());
    issued.session()
}
async fn access_token(
    owner: &DatabaseConnection,
    session: &str,
    keys: &webdock_identity::SecretKeys,
    browser: &webdock_browser_session::BrowserSessions,
) -> String {
    // Resolve only this genuinely issued cookie; never fabricate a session or expose token bytes.
    let issued = browser
        .resolve_authority(CONSOLE, session)
        .await
        .unwrap()
        .unwrap();
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT payload FROM webdock_auth.identity_browser_session WHERE token_hash=$1",
            [issued.authority.reference.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let stored: Value = serde_json::from_str(
        &keys
            .decrypt(&row.try_get::<String>("", "payload").unwrap())
            .unwrap(),
    )
    .unwrap();
    stored["access_token"].as_str().unwrap().to_owned()
}
async fn post_form(app: &Router, path: &str, fields: &[(&str, &str)]) -> (StatusCode, Value) {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("host", "auth.example.test")
                .header("origin", AUTH)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn revoke(app: &Router, secrets: &Value, token: &str) -> StatusCode {
    post_form(
        app,
        "/api/auth/oauth2/revoke",
        &[
            ("client_id", secrets["clientID"].as_str().unwrap()),
            (
                "client_secret",
                secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
                    .as_str()
                    .unwrap(),
            ),
            ("token", token),
        ],
    )
    .await
    .0
}
async fn revoked(owner: &DatabaseConnection, hash: &str) -> bool {
    owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT revoked IS NOT NULL AS value FROM webdock_auth.\"oauthAccessToken\" WHERE token=$1",[hash.into()])).await.unwrap().unwrap().try_get("","value").unwrap()
}

async fn identity_before_token(
    c: &Context<'_>,
    credentials: &webdock_oauth::ClientAuthentication,
    token: &str,
    hash: &str,
) {
    let subject: String = c
        .owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT "userId" AS subject FROM webdock_auth."oauthAccessToken" WHERE token=$1"#,
            [hash.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "subject")
        .unwrap();
    let identity = c.auth.begin().await.unwrap();
    identity
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id FROM webdock_auth."user" WHERE id=$1 FOR UPDATE"#,
            [subject.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let identity_pid: i32 = identity
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    let guard = c.auth.begin().await.unwrap();
    {
        let verify = c.oauth.introspect_portal_in(&guard, credentials, token);
        tokio::pin!(verify);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let observed = c.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.datname=current_database() AND $1::integer=ANY(pg_blocking_pids(a.pid))) AS blocked", [identity_pid.into()]));
            tokio::select! {
                _ = &mut verify => panic!("Verifier must wait for the current identity row"),
                row = observed => { if row.unwrap().unwrap().try_get::<bool>("", "blocked").unwrap() { break; } }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Identity lock must be observed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let probe = c.owner.begin().await.unwrap();
        let available = probe.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            r#"SELECT id FROM webdock_auth."oauthAccessToken" WHERE token=$1 FOR UPDATE NOWAIT"#, [hash.into()])).await;
        assert!(
            available.is_ok(),
            "Introspection must lock identity before token, matching refresh order"
        );
        probe.rollback().await.unwrap();
        identity.commit().await.unwrap();
        assert_eq!(verify.await.unwrap()["active"], true);
    }
    guard.commit().await.unwrap();
}

pub(super) async fn check(c: Context<'_>) {
    let credentials = webdock_oauth::ClientAuthentication {
        client_id: c.secrets["clientID"].as_str().unwrap().to_owned(),
        client_secret: Some(
            c.secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
                .as_str()
                .unwrap()
                .to_owned(),
        ),
        method: webdock_oauth::ClientAuthMethod::ClientSecretPost,
    };
    let session = console_session(c.app, c.native).await;
    let token = access_token(c.owner, &session, c.keys, c.browser).await;
    let hash = webdock_oauth::legacy_token_hash(&token);
    assert!(!revoked(c.owner, &hash).await);
    identity_before_token(&c, &credentials, &token, &hash).await;
    let guard = c.auth.begin().await.unwrap();
    let claims = c
        .oauth
        .introspect_portal_in(&guard, &credentials, &token)
        .await
        .unwrap();
    assert_eq!(claims["active"], true);
    let pid: i32 = guard
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();

    // The production verifier's retained transaction must hold this specific token row.
    // No transport interposition or production timing hooks are involved.
    let revoke_call = revoke(c.app, c.secrets, &token);
    tokio::pin!(revoke_call);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let observed = c.owner.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.datname=current_database() AND $1::integer=ANY(pg_blocking_pids(a.pid)) AND a.query LIKE 'UPDATE webdock_auth.%oauthAccessToken%') AS blocked", [pid.into()]));
        tokio::select! {
            status = &mut revoke_call => panic!("Revocation completed before retained authorization released (HTTP {})", status.as_u16()),
            row = observed => {
                if row.unwrap().unwrap().try_get::<bool>("", "blocked").unwrap() { break; }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Owned token revocation must wait on the retained authorization transaction"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!revoked(c.owner, &hash).await);
    // Host admin_commit retains its authorization guard across this business boundary.
    // This component regression proves the database lock lifetime; it does not claim
    // a post-final-verification interception of the complete endpoint.
    let business = c.business.begin().await.unwrap();
    business.execute_unprepared("SELECT 1").await.unwrap();
    business.commit().await.unwrap();
    assert!(!revoked(c.owner, &hash).await);
    guard.commit().await.unwrap();
    assert_eq!(revoke_call.await, StatusCode::OK);
    assert!(revoked(c.owner, &hash).await);
    let after = c.auth.begin().await.unwrap();
    assert_eq!(
        c.oauth
            .introspect_portal_in(&after, &credentials, &token)
            .await
            .unwrap()["active"],
        false
    );
    after.commit().await.unwrap();
    assert!(matches!(
        activate(c.app, &session, HOSTING, "revoked-native-authority-01")
            .await
            .status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.plugin_installation"
        )
        .await,
        0
    );

    // A separately issued real cookie also fails when revocation commits first.
    let first = console_session(c.app, c.native).await;
    let first_token = access_token(c.owner, &first, c.keys, c.browser).await;
    assert_eq!(revoke(c.app, c.secrets, &first_token).await, StatusCode::OK);
    let after = c.auth.begin().await.unwrap();
    assert_eq!(
        c.oauth
            .introspect_portal_in(&after, &credentials, &first_token)
            .await
            .unwrap()["active"],
        false
    );
    after.commit().await.unwrap();
    assert!(matches!(
        activate(c.app, &first, HOSTING, "revoked-native-authority-02")
            .await
            .status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(
        scalar(
            c.owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.plugin_operation"
        )
        .await,
        0
    );
}

pub(super) async fn refresh_regression(
    app: &Router,
    owner: &DatabaseConnection,
    native: &str,
    oauth: &webdock_oauth::OAuthService,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let client = oauth.provision_client(owner, webdock_oauth::ClientRegistration {
            name:"Owned refresh regression".into(), redirect_uris:vec!["https://refresh.example.test/callback".into()],
            post_logout_redirect_uris:vec![], scopes:vec!["openid".into(),"profile".into(),"email".into(),"offline_access".into()],
            auth_method:webdock_oauth::ClientAuthMethod::ClientSecretPost, allow_refresh:true, skip_consent:true, purpose:webdock_oauth::ClientPurpose::Portal,
        }).await.unwrap();
        let secret=client.client_secret.as_deref().unwrap();
        let mut authorize=url::Url::parse(&format!("{AUTH}/api/auth/oauth2/authorize")).unwrap();
        authorize.query_pairs_mut().extend_pairs([
            ("client_id",client.client_id.as_str()),("redirect_uri","https://refresh.example.test/callback"),
            ("response_type","code"),("scope","openid profile email offline_access"),("state","owned-refresh"),
            ("code_challenge","E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),("code_challenge_method","S256")
        ]);
        let granted=request(app,AUTH,"GET",&format!("{}?{}",authorize.path(),authorize.query().unwrap()),native,Value::Null).await;
        assert!(granted.status.is_redirection());
        let code=granted.location().query_pairs().find(|(k,_)|k=="code").unwrap().1.into_owned();
        let (status,tokens)=post_form(app,"/api/auth/oauth2/token",&[
            ("grant_type","authorization_code"),("client_id",&client.client_id),("client_secret",secret),
            ("redirect_uri","https://refresh.example.test/callback"),("code",&code),
            ("code_verifier","dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")
        ]).await;
        assert_eq!(status,StatusCode::OK);
        let refresh=tokens["refresh_token"].as_str().unwrap();
        let fields=[("grant_type","refresh_token"),("client_id",client.client_id.as_str()),("client_secret",secret),("refresh_token",refresh)];
        let (status,rotated)=post_form(app,"/api/auth/oauth2/token",&fields).await;
        assert_eq!(status,StatusCode::OK);
        assert!(oauth.userinfo(rotated["access_token"].as_str().unwrap()).await.is_ok());
        assert_eq!(post_form(app,"/api/auth/oauth2/token",&fields).await.0,StatusCode::BAD_REQUEST);
        assert!(oauth.userinfo(rotated["access_token"].as_str().unwrap()).await.is_err());
        eprintln!("builtin_revocation refresh=rotated userinfo=live family_replay=denied-without-self-block");
    }).await.expect("refresh, userinfo and replay complete without self-block");
}
