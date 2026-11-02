use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientAuthentication,
    ClientPurpose, ClientRegistration, CodeExchange, OAuthConfig, OAuthService,
};
pub struct McpFixture {
    pub oauth: Arc<OAuthService>,
    pub client: String,
    pub secret: String,
    pub read: String,
    pub write: String,
    signing: Option<String>,
}
impl McpFixture {
    pub async fn new(
        owner: &DatabaseConnection,
        identity: Arc<IdentityService>,
        cookie: &str,
        nonce: &str,
    ) -> Self {
        let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
        let keys = webdock_identity::SecretKeys::new(
            std::collections::BTreeMap::from([(0, secret.clone()), (1, secret.clone())]),
            1,
            Some(secret),
        )
        .unwrap();
        let oauth = Arc::new(OAuthService::new(
            db("AUTH_DATABASE_URL").await,
            identity,
            Arc::new(webdock_persistence::IdAllocator::new(
                db("AUTH_DATABASE_URL").await,
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )),
            Arc::new(keys),
            OAuthConfig::new(
                "http://localhost:3121/api/auth",
                "http://localhost:3120/api/mcp",
                None,
            )
            .unwrap(),
        ));
        let signing = if oauth.jwks().await.unwrap()["keys"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            Some(oauth.provision_signing_key(owner).await.unwrap())
        } else {
            None
        };
        let client = oauth
            .provision_client(
                owner,
                ClientRegistration {
                    name: format!("{nonce} MCP"),
                    redirect_uris: vec!["https://client.example.invalid/callback".into()],
                    post_logout_redirect_uris: vec!["https://client.example.invalid/logout".into()],
                    scopes: vec![
                        "openid".into(),
                        "webdock:read".into(),
                        "webdock:write".into(),
                    ],
                    auth_method: ClientAuthMethod::ClientSecretPost,
                    allow_refresh: false,
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
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut tokens = Vec::new();
        for scopes in ["openid webdock:read", "openid webdock:read webdock:write"] {
            let mut request = AuthorizationRequest::code(
                client.client_id.clone(),
                "https://client.example.invalid/callback".into(),
                scopes.into(),
                challenge.clone(),
            );
            request.resources = vec!["http://localhost:3120/api/mcp".into()];
            let result = oauth.authorize(cookie, request).await.unwrap();
            let result = match result {
                AuthorizationOutcome::ConsentRequired(consent) => oauth
                    .consent(cookie, &consent.request_id, true)
                    .await
                    .unwrap(),
                result => result,
            };
            let AuthorizationOutcome::Redirect(location) = result else {
                panic!("MCP authorization required")
            };
            let code = url::Url::parse(&location)
                .unwrap()
                .query_pairs()
                .find(|(k, _)| k == "code")
                .unwrap()
                .1
                .into_owned();
            let token = oauth
                .exchange_code(CodeExchange {
                    client: credentials(),
                    code,
                    redirect_uri: "https://client.example.invalid/callback".into(),
                    code_verifier: verifier.into(),
                })
                .await
                .unwrap();
            tokens.push(token.access_token);
        }
        Self {
            oauth,
            client: client.client_id,
            secret: client.client_secret.unwrap(),
            read: tokens.remove(0),
            write: tokens.remove(0),
            signing,
        }
    }
    pub async fn cleanup(&self, owner: &DatabaseConnection) {
        sql(
            owner,
            "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
            vec![self.client.clone().into()],
        )
        .await;
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
