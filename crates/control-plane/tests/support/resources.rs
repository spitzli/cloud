use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tower::ServiceExt;
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    resource_auth::ResourceClientConfig,
};
use webdock_identity::{CookiePolicy, IdentityService, RlsSigner, SecretKeys};
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientAuthentication,
    ClientPurpose, ClientRegistration, CodeExchange, OAuthConfig, OAuthService,
};
use webdock_persistence::IdAllocator;
use webdock_registry::RegistryService;

pub async fn db(key: &str) -> DatabaseConnection {
    let raw = std::env::var(key).expect("explicit local fixture environment");
    let url = url::Url::parse(&raw).unwrap();
    assert!(
        matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
            && url.path() == "/webdock_rls_test"
            && url.query().is_none(),
        "owned local fixture only"
    );
    let mut options = sea_orm::ConnectOptions::new(raw);
    options
        .max_connections(1)
        .min_connections(0)
        .acquire_timeout(std::time::Duration::from_secs(1))
        .sqlx_logging(false);
    Database::connect(options).await.unwrap()
}
pub async fn sql(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}
pub async fn count(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}
pub async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    data: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost:3120")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = if let Some(data) = data {
        builder = builder.header("content-type", "application/json");
        Body::from(data.to_string())
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
pub struct Client {
    pub id: String,
    pub secret: String,
    pub read: String,
    pub write: String,
}
pub struct Fixture {
    pub owner: DatabaseConnection,
    pub oauth: Arc<OAuthService>,
    pub identity: Arc<IdentityService>,
    pub registry: Arc<RegistryService>,
    pub hosting: Arc<webdock_hosting::HostingService>,
    pub resource: String,
    owned_resource: bool,
    pub nonce: String,
    pub uid: String,
    pub sid: String,
    pub mapped: String,
    pub cookie: String,
    pub registry_client: Client,
    pub hosting_client: Client,
    signing: Option<String>,
    module_added: bool,
    secret_file: tempfile::NamedTempFile,
}
impl Fixture {
    pub async fn new() -> Self {
        Self::create(false).await
    }
    pub async fn isolated_resource() -> Self {
        Self::create(true).await
    }
    async fn create(owned_resource: bool) -> Self {
        let owner = db("WEBDOCK_TEST_MIGRATION_DATABASE_URL").await;
        let nonce = format!("rust-resources-{}", uuid::Uuid::new_v4());
        let resource = if owned_resource {
            format!("http://localhost:3120/api/mcp/{nonce}")
        } else {
            "http://localhost:3120/api/mcp".into()
        };
        let uid = nonce.clone();
        let sid = format!("{nonce}-session");
        let token = format!("{nonce}-token");
        let email = format!("{nonce}@example.invalid");
        let ids = Arc::new(IdAllocator::new(
            db("AUTH_DATABASE_URL").await,
            webdock_ids::Identity::platform(0, 2).unwrap(),
        ));
        let mapped = ids.allocate().await.unwrap().to_string();
        sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,'operator',true,false)",vec![uid.clone().into(),email.clone().into()]).await;
        sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())",vec![sid.clone().into(),token.clone().into(),uid.clone().into()]).await;
        sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,$2,$3,'operator',$2)",vec![mapped.clone().into(),uid.clone().into(),email.into()]).await;
        let secret = std::env::var("BETTER_AUTH_SECRET").unwrap();
        let cookies = CookiePolicy::new(false, secret.clone()).unwrap();
        let cookie = format!("{}={}", cookies.name(), cookies.sign(&token).unwrap());
        let keys = Arc::new(
            SecretKeys::new(
                BTreeMap::from([(1, secret.clone())]),
                1,
                Some(secret.clone()),
            )
            .unwrap(),
        );
        let identity = Arc::new(IdentityService::new(db("AUTH_DATABASE_URL").await, cookies));
        let oauth = Arc::new(OAuthService::new(
            db("AUTH_DATABASE_URL").await,
            identity.clone(),
            ids.clone(),
            keys,
            OAuthConfig::new("http://localhost:3125/api/auth", &resource, None).unwrap(),
        ));
        let signing = if oauth.jwks().await.unwrap()["keys"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            Some(oauth.provision_signing_key(&owner).await.unwrap())
        } else {
            None
        };
        let registry_client = client(
            &owner,
            &oauth,
            &cookie,
            &nonce,
            &resource,
            ClientPurpose::McpOperator,
            "webdock",
        )
        .await;
        let hosting_client = client(
            &owner,
            &oauth,
            &cookie,
            &nonce,
            &resource,
            ClientPurpose::McpHosting,
            "hosting",
        )
        .await;
        let mut secret_file = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        secret_file
            .write_all(registry_client.secret.as_bytes())
            .unwrap();
        let registry = Arc::new(
            RegistryService::new(
                db("WEBDOCK_REGISTRY_DATABASE_URL").await,
                RlsSigner::new(
                    std::env::var("DATABASE_RLS_SECRET").unwrap(),
                    "webdock_registry_runtime",
                )
                .unwrap(),
            )
            .with_allocator(ids.clone()),
        );
        let hosting = Arc::new(
            webdock_hosting::HostingService::new(
                db("DATABASE_URL").await,
                db("AUTH_DATABASE_URL").await,
                db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,
                std::env::var("DATABASE_RLS_SECRET").unwrap(),
                secret,
                ids,
            )
            .await
            .unwrap(),
        );
        let module_added=owner.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('hosting','builtin','enabled','enabled',$1) ON CONFLICT DO NOTHING",[nonce.clone().into()])).await.unwrap().rows_affected()==1;
        Self {
            owner,
            oauth,
            identity,
            registry,
            hosting,
            resource,
            owned_resource,
            nonce,
            uid,
            sid,
            mapped,
            cookie,
            registry_client,
            hosting_client,
            signing,
            module_added,
            secret_file,
        }
    }
    pub async fn app(&self, registry: Option<Arc<RegistryService>>) -> Router {
        self.app_with_client(registry, None).await
    }
    pub async fn app_with_client(
        &self,
        registry: Option<Arc<RegistryService>>,
        client: Option<ResourceClientConfig>,
    ) -> Router {
        build_app_with_domains(
            webdock_core::config::PlatformConfig::from_map(&HashMap::from([(
                "WEBDOCK_FEATURES".into(),
                r#"{"hosting":"enabled"}"#.into(),
            )]))
            .unwrap(),
            db("AUTH_DATABASE_URL").await,
            Services {
                identity: Some(self.identity.clone()),
                oauth: Some(self.oauth.clone()),
                registry: Some(registry.unwrap_or_else(|| self.registry.clone())),
                hosting: Some(self.hosting.clone()),
                resource_client: Some(client.unwrap_or_else(|| ResourceClientConfig {
                    client_id: self.registry_client.id.clone(),
                    secret: self.secret_file.path().to_path_buf(),
                })),
                ..Default::default()
            },
        )
    }
    pub async fn cleanup(&self) {
        for id in [&self.registry_client.id, &self.hosting_client.id] {
            sql(
                &self.owner,
                "DELETE FROM webdock_auth.\"oauthClient\" WHERE \"clientId\"=$1",
                vec![id.clone().into()],
            )
            .await;
        }
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.audit_events WHERE actor_id=$1",
            vec![self.mapped.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_operation WHERE subject=$1",
            vec![self.uid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_audit WHERE subject=$1",
            vec![self.uid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.hosting_cluster WHERE name=$1",
            vec![self.nonce.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.projects WHERE name=$1",
            vec![self.nonce.clone().into()],
        )
        .await;
        sql(&self.owner,"DELETE FROM webdock_auth.tenant_customer WHERE customer_id IN (SELECT id FROM webdock_admin.customers WHERE name=$1 OR name=$2)",vec![self.nonce.clone().into(),format!("{}-expiry",self.nonce).into()]).await;
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.customers WHERE name=$1 OR name=$2",
            vec![
                self.nonce.clone().into(),
                format!("{}-expiry", self.nonce).into(),
            ],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.organization WHERE name=$1 OR name=$2",
            vec![
                self.nonce.clone().into(),
                format!("{}-expiry", self.nonce).into(),
            ],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_admin.users WHERE id=$1",
            vec![self.mapped.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.session WHERE id=$1",
            vec![self.sid.clone().into()],
        )
        .await;
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.\"user\" WHERE id=$1",
            vec![self.uid.clone().into()],
        )
        .await;
        if self.owned_resource {
            sql(
                &self.owner,
                "DELETE FROM webdock_auth.\"oauthResource\" WHERE identifier=$1",
                vec![self.resource.clone().into()],
            )
            .await;
        }
        if let Some(id) = &self.signing {
            sql(
                &self.owner,
                "DELETE FROM webdock_auth.jwks WHERE id=$1",
                vec![id.clone().into()],
            )
            .await;
        }
        if self.module_added {
            sql(
                &self.owner,
                "DELETE FROM webdock_auth.plugin_installation WHERE id='hosting' AND version=$1",
                vec![self.nonce.clone().into()],
            )
            .await;
        }
    }
}
async fn client(
    owner: &DatabaseConnection,
    oauth: &OAuthService,
    cookie: &str,
    nonce: &str,
    resource: &str,
    purpose: ClientPurpose,
    prefix: &str,
) -> Client {
    let client = oauth
        .provision_client(
            owner,
            ClientRegistration {
                name: format!("{nonce} {prefix}"),
                redirect_uris: vec!["https://client.example.invalid/callback".into()],
                post_logout_redirect_uris: vec!["https://client.example.invalid/logout".into()],
                scopes: vec![
                    "openid".into(),
                    format!("{prefix}:read"),
                    format!("{prefix}:write"),
                ],
                auth_method: ClientAuthMethod::ClientSecretPost,
                allow_refresh: false,
                skip_consent: false,
                purpose,
            },
        )
        .await
        .unwrap();
    let credentials = ClientAuthentication {
        client_id: client.client_id.clone(),
        client_secret: client.client_secret.clone(),
        method: ClientAuthMethod::ClientSecretPost,
    };
    let mut tokens = vec![];
    for scope in [
        format!("openid {prefix}:read"),
        format!("openid {prefix}:read {prefix}:write"),
    ] {
        tokens.push(issue(oauth, cookie, &credentials, &scope, resource).await);
    }
    Client {
        id: client.client_id,
        secret: client.client_secret.unwrap(),
        read: tokens.remove(0),
        write: tokens.remove(0),
    }
}
pub async fn issue(
    oauth: &OAuthService,
    cookie: &str,
    credentials: &ClientAuthentication,
    scope: &str,
    resource: &str,
) -> String {
    let verifier = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-._~";
    let mut request = AuthorizationRequest::code(
        credentials.client_id.clone(),
        "https://client.example.invalid/callback".into(),
        scope.into(),
        URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
    );
    request.resources = vec![resource.into()];
    let result = oauth.authorize(cookie, request).await.unwrap();
    let result = match result {
        AuthorizationOutcome::ConsentRequired(consent) => oauth
            .consent(cookie, &consent.request_id, true)
            .await
            .unwrap(),
        result => result,
    };
    let AuthorizationOutcome::Redirect(location) = result else {
        panic!("authorization redirect required")
    };
    let code = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned();
    oauth
        .exchange_code(CodeExchange {
            client: credentials.clone(),
            code,
            redirect_uri: "https://client.example.invalid/callback".into(),
            code_verifier: verifier.into(),
        })
        .await
        .unwrap()
        .access_token
}
