//! Real first-party OAuth handoff using the existing isolated operator fixture.
use super::*;
use webdock_browser_session::{BrowserConfig, BrowserSessions, StoredAuthority};
use webdock_identity::SecretKeys;
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientPurpose,
    ClientRegistration, OAuthConfig, OAuthService,
};
pub struct BrowserFixture {
    pub identity: Arc<IdentityService>,
    pub browser: Arc<BrowserSessions>,
    pub authority: StoredAuthority,
    #[allow(
        dead_code,
        reason = "Shared browser fixture is compiled by suites that use different fields."
    )]
    pub cookie: String,
    pub client: String,
    signing: Option<String>,
}
impl BrowserFixture {
    pub async fn new(f: &Fixture) -> Self {
        let auth = db("AUTH_DATABASE_URL").await;
        let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
        let keys = || {
            SecretKeys::new(
                std::collections::BTreeMap::from([(0, secret.clone()), (1, secret.clone())]),
                1,
                Some(secret.clone()),
            )
            .unwrap()
        };
        let ids = Arc::new(webdock_persistence::IdAllocator::new(
            db("AUTH_DATABASE_URL").await,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        ));
        let cookies =
            CookiePolicy::new(false, "fixture-hosting-cookie-key-at-least-32-bytes").unwrap();
        let account_cookie = format!(
            "{}={}",
            cookies.name(),
            cookies.sign(&format!("{}-token", f.nonce)).unwrap()
        );
        let identity = Arc::new(
            IdentityService::new(auth.clone(), cookies)
                .with_secret_keys(keys())
                .with_allocator(ids.clone()),
        );
        let bootstrap = OAuthService::new(
            auth.clone(),
            identity.clone(),
            ids.clone(),
            Arc::new(keys()),
            OAuthConfig::new(
                "https://auth.example.invalid/api/auth",
                "https://studio.example.invalid/api/mcp",
                None,
            )
            .unwrap(),
        );
        let signing = if bootstrap.jwks().await.unwrap()["keys"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            Some(bootstrap.provision_signing_key(&f.owner).await.unwrap())
        } else {
            None
        };
        let client = bootstrap
            .provision_client(
                &f.owner,
                ClientRegistration {
                    name: format!("{} browser", f.nonce),
                    redirect_uris: vec!["https://console.example.invalid/api/sso/callback".into()],
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
            auth.clone(),
            identity.clone(),
            ids,
            Arc::new(keys()),
            OAuthConfig::new(
                "https://auth.example.invalid/api/auth",
                "https://studio.example.invalid/api/mcp",
                Some(client.client_id.clone()),
            )
            .unwrap(),
        ));
        let browser = Arc::new(
            BrowserSessions::new(
                auth,
                identity.clone(),
                oauth.clone(),
                Arc::new(keys()),
                &secret,
                BrowserConfig::new(
                    "https://console.example.invalid",
                    "https://auth.example.invalid/api/auth",
                    client.client_id.clone(),
                    client.client_secret.clone().unwrap(),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        let flow = browser
            .begin("https://console.example.invalid", "/projects")
            .await
            .unwrap();
        let url = url::Url::parse(&flow.location).unwrap();
        let values = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let state = values["state"].to_string();
        let request = AuthorizationRequest {
            client_id: values["client_id"].to_string(),
            redirect_uri: values["redirect_uri"].to_string(),
            response_type: "code".into(),
            scope: values["scope"].to_string(),
            state: Some(state.clone()),
            nonce: Some(values["nonce"].to_string()),
            code_challenge: values["code_challenge"].to_string(),
            code_challenge_method: "S256".into(),
            prompt: None,
            max_age: None,
            resources: vec![],
        };
        let AuthorizationOutcome::Redirect(callback) =
            oauth.authorize(&account_cookie, request).await.unwrap()
        else {
            panic!("fixture portal authorization")
        };
        let code = url::Url::parse(&callback)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        let exchanged = browser
            .callback(
                "https://console.example.invalid",
                flow.set_cookies[0].split(';').next().unwrap(),
                &code,
                &state,
            )
            .await
            .unwrap();
        let cookie = exchanged.set_cookies[0]
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let auth = browser
            .resolve_authority("https://console.example.invalid", &cookie)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(auth.principal.user_id, f.uid);
        assert!(!auth.authority.read_only);
        Self {
            identity,
            browser,
            authority: StoredAuthority::FirstPartyBrowser {
                browser: auth.authority,
            },
            cookie,
            client: client.client_id,
            signing,
        }
    }
    pub async fn revoke(&self, owner: &DatabaseConnection) {
        sql(
            owner,
            "UPDATE webdock_auth.\"oauthClient\" SET disabled=true WHERE \"clientId\"=$1",
            vec![self.client.clone().into()],
        )
        .await;
    }
    pub async fn cleanup(&self, owner: &DatabaseConnection) {
        for statement in [
            "DELETE FROM webdock_auth.identity_browser_session WHERE client_id=$1",
            "DELETE FROM webdock_auth.identity_browser_flow WHERE client_id=$1",
            "DELETE FROM webdock_auth.app_binding WHERE client_id=$1",
            "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
        ] {
            sql(owner, statement, vec![self.client.clone().into()]).await;
        }
        if let Some(id) = &self.signing {
            sql(
                owner,
                "DELETE FROM webdock_auth.jwks WHERE id=$1",
                vec![id.clone().into()],
            )
            .await;
        }
    }
}
