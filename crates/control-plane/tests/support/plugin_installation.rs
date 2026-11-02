//! Owned PostgreSQL and real native services for the installation HTTP boundary only.
use axum::Router;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::Arc,
    time::Duration,
};
use webdock_browser_session::{BrowserConfig, BrowserSessions};
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    plugin_config::RuntimeConnections,
    plugins::PluginEndpoints,
};
use webdock_identity::{CookiePolicy, IdentityService, SecretKeys};
use webdock_oauth::{
    AuthorizationOutcome, AuthorizationRequest, ClientAuthMethod, ClientPurpose,
    ClientRegistration, OAuthConfig, OAuthService,
};
use webdock_plugin_host::{Catalog, FileBindings, HostService};
pub const SECRET: &str = "owned-install-http-secret-at-least32";
pub const ORIGIN: &str = "http://localhost:3120";

fn docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", "unix:///var/run/docker.sock"]);
    command
}
struct OwnedDatabase {
    name: String,
    url: url::Url,
    passwords: BTreeMap<String, String>,
}
impl Drop for OwnedDatabase {
    fn drop(&mut self) {
        let _ = docker().args(["rm", "-f", &self.name]).output();
    }
}
impl OwnedDatabase {
    async fn new() -> Self {
        let name = format!("wd-install-http-{}", uuid::Uuid::new_v4().simple());
        assert!(
            docker()
                .args([
                    "run",
                    "-d",
                    "--rm",
                    "--name",
                    &name,
                    "-e",
                    "POSTGRES_HOST_AUTH_METHOD=trust",
                    "-e",
                    "POSTGRES_DB=webdock_install_http",
                    "-p",
                    "127.0.0.1::5432",
                    "postgres:18-alpine"
                ])
                .output()
                .unwrap()
                .status
                .success()
        );
        let port = docker().args(["port", &name, "5432/tcp"]).output().unwrap();
        let address = String::from_utf8(port.stdout).unwrap();
        assert!(address.trim().starts_with("127.0.0.1:"));
        let url = url::Url::parse(&format!(
            "postgres://postgres@{}/webdock_install_http?sslmode=disable",
            address.trim()
        ))
        .unwrap();
        let passwords = webdock_persistence::roles::LOGIN_ROLES
            .iter()
            .copied()
            .chain(["webdock_offline_migrate"])
            .map(|role| (role.into(), uuid::Uuid::new_v4().simple().to_string()))
            .collect();
        let fixture = Self {
            name,
            url,
            passwords,
        };
        let owner = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(db) = Database::connect(fixture.url.as_str()).await {
                    break db;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("owned PostgreSQL startup");
        webdock_persistence::roles::provision_roles(
            &owner,
            "webdock_install_http",
            &fixture.passwords,
        )
        .await
        .unwrap();
        webdock_persistence::migrations::bootstrap(&owner, "webdock_install_http", SECRET)
            .await
            .unwrap();
        fixture
    }
    async fn role(&self, role: &str) -> DatabaseConnection {
        let mut url = self.url.clone();
        url.set_username(role).unwrap();
        url.set_password(Some(&self.passwords[role])).unwrap();
        let mut options = sea_orm::ConnectOptions::new(url.as_str());
        options
            .max_connections(4)
            .min_connections(0)
            .sqlx_logging(false);
        Database::connect(options).await.unwrap()
    }
}
pub async fn sql(db: &DatabaseConnection, query: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        query,
        values,
    ))
    .await
    .unwrap();
}
pub async fn count(db: &DatabaseConnection, query: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, query))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
pub fn cookie(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap()
}
pub struct Fixture {
    pub owner: DatabaseConnection,
    pub host: Arc<HostService>,
    pub local: Router,
    pub first_party: Router,
    pub operator_cookie: String,
    pub customer_cookie: String,
    pub scope_cookie: String,
    pub browser: Arc<BrowserSessions>,
    pub oauth: Arc<OAuthService>,
    pub catalog: Value,
    _directory: tempfile::TempDir,
    _database: OwnedDatabase,
}
impl Fixture {
    pub async fn new(catalog: Value) -> Self {
        let database = OwnedDatabase::new().await;
        let owner = Database::connect(database.url.as_str()).await.unwrap();
        for (id, role, mfa) in [("operator", "operator", true), ("customer", "user", false)] {
            sql(&owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES($1,$1,$2,true,$3,$4,false)",vec![id.into(),format!("{id}@example.test").into(),role.into(),mfa.into()]).await;
        }
        for (id, subject, token) in [
            ("operator-session", "operator", "operator-token"),
            ("scope-session", "operator", "scope-token"),
            ("customer-session", "customer", "customer-token"),
        ] {
            sql(&owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES($1,$2,$3,now()+interval '1 hour',now())",vec![id.into(),token.into(),subject.into()]).await;
        }
        sql(&owner,"INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES('100','Operator','operator@example.test','operator','operator')",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,runtime_installation,runtime_kind) VALUES('42','Owned HTTP fixture','fixture','eu','owned fixture',true,'fixture','kubernetes')",vec![]).await;
        // Negative customer-scope fixtures never become installation/workload owners.
        sql(&owner,"WITH hint AS MATERIALIZED (SELECT set_config('webdock.organization_allocation',jsonb_build_object('customer_id','200','organization_id','201')::text,true)) INSERT INTO webdock_admin.customers(id,name) SELECT '200','Scope denial fixture' FROM hint",vec![]).await;
        sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('customer-member','201','customer','owner',now())",vec![]).await;
        let auth = database.role("webdock_auth_bootstrap").await;
        let business = database.role("webdock_auth_runtime").await;
        let worker = database.role("webdock_worker_runtime").await;
        let ids = Arc::new(
            webdock_persistence::IdAllocator::connect(
                database.url.as_str(),
                webdock_ids::Identity::platform(0, 2).unwrap(),
            )
            .await
            .unwrap(),
        );
        let keys = || {
            Arc::new(
                SecretKeys::new(BTreeMap::from([(1, SECRET.into())]), 1, Some(SECRET.into()))
                    .unwrap(),
            )
        };
        let identity = Arc::new(
            IdentityService::new(auth.clone(), CookiePolicy::new(false, SECRET).unwrap())
                .with_secret_keys(
                    SecretKeys::new(BTreeMap::from([(1, SECRET.into())]), 1, Some(SECRET.into()))
                        .unwrap(),
                )
                .with_allocator(ids.clone()),
        );
        let issuer = "http://localhost:3125/api/auth";
        let provisioner = OAuthService::new(
            auth.clone(),
            identity.clone(),
            ids.clone(),
            keys(),
            OAuthConfig::new(issuer, "http://localhost:3120/api/mcp", None).unwrap(),
        );
        let client = provisioner
            .provision_client(
                &owner,
                ClientRegistration {
                    name: "Owned HTTP Console".into(),
                    redirect_uris: vec![format!("{ORIGIN}/api/sso/callback")],
                    post_logout_redirect_uris: vec![],
                    scopes: vec!["openid".into(), "profile".into(), "email".into()],
                    auth_method: ClientAuthMethod::ClientSecretPost,
                    allow_refresh: false,
                    skip_consent: true,
                    purpose: ClientPurpose::Portal,
                },
            )
            .await
            .unwrap();
        let oauth = Arc::new(OAuthService::new(
            auth.clone(),
            identity.clone(),
            ids.clone(),
            keys(),
            OAuthConfig::new(
                issuer,
                "http://localhost:3120/api/mcp",
                Some(client.client_id.clone()),
            )
            .unwrap(),
        ));
        oauth.provision_signing_key(&owner).await.unwrap();
        let browser = Arc::new(
            BrowserSessions::new(
                auth.clone(),
                identity.clone(),
                oauth.clone(),
                keys(),
                SECRET,
                BrowserConfig::new(
                    ORIGIN,
                    issuer,
                    client.client_id,
                    client.client_secret.unwrap(),
                )
                .unwrap(),
            )
            .unwrap(),
        );
        let directory = tempfile::tempdir().unwrap();
        let bindings = directory.path().join("bindings.json");
        std::fs::write(&bindings, b"{\"runtimes\":{}}").unwrap();
        std::fs::set_permissions(&bindings, std::fs::Permissions::from_mode(0o600)).unwrap();
        let host = Arc::new(
            HostService::new(
                business,
                worker,
                SECRET.into(),
                ids,
                Arc::new(FileBindings::new(bindings.clone())),
                ed25519_dalek::SigningKey::from_bytes(&[17; 32]),
            )
            .await
            .unwrap()
            .with_session_authority(auth.clone(), identity.clone(), Some(browser.clone()))
            .await
            .unwrap()
            .with_installation(
                Arc::new(Catalog::from_bytes(&serde_json::to_vec(&catalog).unwrap()).unwrap()),
                SECRET,
            )
            .unwrap(),
        );
        let endpoints = Arc::new(PluginEndpoints::new(
            host.clone(),
            RuntimeConnections::new(bindings),
            None,
        ));
        let app = |first_party: bool| {
            build_app_with_domains(
                webdock_core::config::PlatformConfig::from_map(&HashMap::new()).unwrap(),
                auth.clone(),
                Services {
                    identity: Some(identity.clone()),
                    oauth: Some(oauth.clone()),
                    browser: if first_party {
                        Some(browser.clone())
                    } else {
                        None
                    },
                    plugins: Some(endpoints.clone()),
                    ..Default::default()
                },
            )
        };
        let policy = CookiePolicy::new(false, SECRET).unwrap();
        let signed = |token| format!("{}={}", policy.name(), policy.sign(token).unwrap());
        Self {
            owner,
            host,
            local: app(false),
            first_party: app(true),
            operator_cookie: signed("operator-token"),
            customer_cookie: signed("customer-token"),
            scope_cookie: signed("scope-token"),
            browser,
            oauth,
            catalog,
            _directory: directory,
            _database: database,
        }
    }
    pub async fn first_party_cookie(&self, local_cookie: &str) -> String {
        let begin = self.browser.begin(ORIGIN, "/extensions").await.unwrap();
        let location = url::Url::parse(&begin.location).unwrap();
        let query: HashMap<_, _> = location
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mut request = AuthorizationRequest::code(
            query["client_id"].clone(),
            query["redirect_uri"].clone(),
            query["scope"].clone(),
            query["code_challenge"].clone(),
        );
        request.state = Some(query["state"].clone());
        request.nonce = Some(query["nonce"].clone());
        let AuthorizationOutcome::Redirect(callback) =
            self.oauth.authorize(local_cookie, request).await.unwrap()
        else {
            panic!("reviewed fixture Portal client must skip consent");
        };
        let callback = url::Url::parse(&callback).unwrap();
        let code = callback
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        let issued = self
            .browser
            .callback(
                ORIGIN,
                cookie(&begin.set_cookies[0]),
                &code,
                &query["state"],
            )
            .await
            .unwrap();
        cookie(&issued.set_cookies[0]).to_owned()
    }
    pub async fn scope(&self, mode: &str) {
        assert!(["read_only", "owner"].contains(&mode));
        sql(
            &self.owner,
            "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id='scope-session'",
            vec![],
        )
        .await;
        sql(&self.owner,"INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,mode) VALUES('scope-session','operator','200','201',$1)",vec![mode.into()]).await;
    }
    pub async fn enroll(&self) {
        // Offline-equivalent test authority only; no heartbeat, deployment or install rows.
        sql(&self.owner,"INSERT INTO webdock_auth.hosting_agent(cluster_id,generation,credential_hash,revoked) VALUES('42',1,$1,false)",vec!["a".repeat(64).into()]).await;
        sql(&self.owner,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES('fixture','http-agent',1,'42',$1,'unix',0)",vec!["a".repeat(64).into()]).await;
    }
}
