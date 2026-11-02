//! Reuses the offline operator fixture; positive setup never seeds actors, sessions or modules.
use super::*;
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use std::collections::HashMap;
use tower::ServiceExt;
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    plugin_config::RuntimeConnections,
    plugins::PluginEndpoints,
};

const HOSTING: &str = "/api/console/plugins/builtins/hosting/activate";
const DATABASES: &str = "/api/console/plugins/builtins/databases/activate";
#[path = "allowance_configuration.rs"]
mod allowance_configuration;
#[path = "database_reads.rs"]
mod database_reads;
#[path = "native_token_lock.rs"]
mod native_token_lock;
#[path = "recovery_admission.rs"]
mod recovery_admission;

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}
impl Reply {
    fn session(&self) -> String {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter(|v| !v.contains("Max-Age=0"))
            .map(cookie)
            .next()
            .unwrap()
            .to_owned()
    }
    fn location(&self) -> url::Url {
        url::Url::parse(self.headers[header::LOCATION].to_str().unwrap()).unwrap()
    }
}
async fn request(
    app: &Router,
    origin: &str,
    method: &str,
    path: &str,
    session: &str,
    body: Value,
) -> Reply {
    let origin = url::Url::parse(origin).unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", origin.host_str().unwrap())
                .header("origin", origin.origin().ascii_serialization())
                .header("cookie", session)
                .header("content-type", "application/json")
                .header("accept", "text/html")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    if std::env::var("WEBDOCK_ALLOWANCE_TEST").as_deref() == Ok("1")
        && headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with('/'))
    {
        let stage = match path.split('?').next().unwrap_or("") {
            "/api/sso/login" => "pkce-begin",
            "/api/sso/callback" => "pkce-callback",
            "/api/auth/oauth2/authorize" => "pkce-authorize",
            _ => "other",
        };
        eprintln!("native_allowance_relative_redirect stage={stage} status={status}");
    }
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        headers,
        body,
    }
}
async fn activate(app: &Router, session: &str, path: &str, key: &str) -> Reply {
    request(
        app,
        CONSOLE,
        "POST",
        path,
        session,
        json!({"idempotencyKey":key}),
    )
    .await
}

async fn runtime(url: &url::Url) -> DatabaseConnection {
    let mut options = ConnectOptions::new(url.as_str());
    options
        .max_connections(4)
        .min_connections(0)
        .sqlx_logging(false);
    Database::connect(options).await.unwrap()
}

#[test]
fn fixture_runtime_keeps_services_live_while_a_private_cli_blocks() {
    use std::io::Read;
    let probe = |runtime: tokio::runtime::Runtime| {
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let (mut peer, _) = listener.accept().await.unwrap();
                peer.write_all(b"ready").await.unwrap();
            });
            let mut client = std::net::TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(std::time::Duration::from_millis(300)))
                .unwrap();
            let mut response = [0; 5];
            let live = client.read_exact(&mut response).is_ok() && &response == b"ready";
            server.abort();
            let _ = server.await;
            live
        })
    };
    assert!(!probe(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    ));
    assert!(probe(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires coordinated stable native migrations; owns/removes one PostgreSQL container"]
async fn fresh_builtin_setup_uses_real_operator_password_totp_pkce_and_serialized_receipts() {
    assert_eq!(
        std::env::var("WEBDOCK_BUILTIN_DOCKER_TEST").as_deref(),
        Ok("1")
    );
    let fixture = if database_reads::enabled() {
        let input = database_reads::owned_fixture();
        Fixture {
            container: input.container,
            url: input.url,
            passwords: input.passwords,
        }
    } else if recovery_admission::enabled() {
        recovery_admission::fixture().await
    } else {
        Fixture::start().await
    };
    let owner = connect(fixture.url.as_str()).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mounts = docker()
        .args([
            "inspect",
            "--format",
            "{{json .Mounts}}",
            &fixture.container,
        ])
        .output()
        .unwrap();
    assert!(mounts.status.success());
    let mounts: Value = serde_json::from_slice(&mounts.stdout).unwrap();
    for mount in mounts
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["Type"] == "volume")
    {
        let volume = mount["Name"].as_str().unwrap();
        assert!(volume.len() == 64 && volume.bytes().all(|b| b.is_ascii_hexdigit()));
        eprintln!(
            "builtin_fixture container={} volume={} private_directory={}",
            fixture.container,
            volume,
            directory.path().display()
        );
    }
    let config_file = directory.path().join("operator-input.json");
    input(&config_file);
    if std::env::var("WEBDOCK_ALLOWANCE_TEST").as_deref() == Ok("1")
        || recovery_admission::enabled()
    {
        let mut reviewed: Value =
            serde_json::from_slice(&std::fs::read(&config_file).unwrap()).unwrap();
        reviewed["initialMailEnabled"] = json!(true);
        std::fs::write(&config_file, serde_json::to_vec(&reviewed).unwrap()).unwrap();
    }
    let output_file = directory.path().join("operator-output.json");
    let offline = fixture.role("webdock_offline_migrate", "webdock_operator_test");
    if std::env::var("WEBDOCK_NATIVE_TOKEN_LOCK_TEST").as_deref() == Ok("1")
        || std::env::var("WEBDOCK_ALLOWANCE_TEST").as_deref() == Ok("1")
        || recovery_admission::enabled()
        || database_reads::enabled()
    {
        // The same reviewed bootstrap service runs in this test executable, so a
        // concurrent unrelated CLI rebuild cannot change the embedded schema.
        let environment = HashMap::from([
            ("DATABASE_URL_UNPOOLED".into(), offline.as_str().to_owned()),
            ("WEBDOCK_AUTH_SECRET".into(), SECRET.to_owned()),
            ("BETTER_AUTH_URL".into(), AUTH.to_owned()),
            ("WEBDOCK_STUDIO_ORIGIN".into(), STUDIO.to_owned()),
            ("WEBDOCK_CONSOLE_ORIGIN".into(), CONSOLE.to_owned()),
        ]);
        if std::env::var("WEBDOCK_ALLOWANCE_TEST").as_deref() == Ok("1") {
            allowance_configuration::check_initial_mail_conflict(
                &owner,
                &environment,
                &offline,
                &config_file,
                directory.path(),
            )
            .await;
        }
        assert!(
            webdock_control_plane::bootstrap::bootstrap_operator(
                &environment,
                &target(&offline),
                &config_file,
                &output_file,
            )
            .await
            .is_ok(),
            "owned bootstrap service must succeed"
        );
    } else {
        let output = tokio::time::timeout(
            Duration::from_secs(45),
            tokio::process::Command::new(
                std::env::var_os("WEBDOCK_BUILTIN_BOOTSTRAP_BINARY")
                    .expect("owned copied bootstrap binary"),
            )
            .env_clear()
            .env("DATABASE_URL_UNPOOLED", offline.as_str())
            .env("WEBDOCK_AUTH_SECRET", SECRET)
            .env("BETTER_AUTH_URL", AUTH)
            .env("WEBDOCK_STUDIO_ORIGIN", STUDIO)
            .env("WEBDOCK_CONSOLE_ORIGIN", CONSOLE)
            .args(["--target", &target(&offline), "--config"])
            .arg(&config_file)
            .arg("--output")
            .arg(&output_file)
            .kill_on_drop(true)
            .output(),
        )
        .await
        .unwrap()
        .unwrap();
        redacted(&output);
        assert!(output.status.success(), "real offline operator bootstrap");
    }
    recovery_admission::phase(recovery_admission::RecoveryPhase::OfflineBootstrap);
    let secrets: Value = serde_json::from_slice(&std::fs::read(&output_file).unwrap()).unwrap();
    if std::env::var("WEBDOCK_ALLOWANCE_TEST").as_deref() == Ok("1") {
        allowance_configuration::check_bootstrap_replay(
            &owner,
            &offline,
            &config_file,
            directory.path(),
        )
        .await;
    }
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.session"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.plugin_installation"
        )
        .await,
        0
    );
    let role = |name| fixture.role(name, "webdock_operator_test");
    let auth = runtime(&role("webdock_auth_bootstrap")).await;
    let business = runtime(&role("webdock_auth_runtime")).await;
    let worker = runtime(&role("webdock_worker_runtime")).await;
    let ids = Arc::new(
        webdock_persistence::IdAllocator::connect(
            role("webdock_auth_runtime").as_str(),
            webdock_ids::Identity::platform(0, 2).unwrap(),
        )
        .await
        .unwrap(),
    );
    let keys = || {
        webdock_identity::SecretKeys::new(
            BTreeMap::from([(1, SECRET.into())]),
            1,
            Some(SECRET.into()),
        )
        .unwrap()
    };
    let identity = Arc::new(
        webdock_identity::IdentityService::new(
            auth.clone(),
            webdock_identity::CookiePolicy::new(true, SECRET).unwrap(),
        )
        .with_secret_keys(keys())
        .with_allocator(ids.clone()),
    );
    let oauth = Arc::new(webdock_oauth::OAuthService::new(
        auth.clone(),
        identity.clone(),
        ids.clone(),
        Arc::new(keys()),
        webdock_oauth::OAuthConfig::new(
            &format!("{AUTH}/api/auth"),
            &format!("{STUDIO}/api/mcp"),
            Some(secrets["clientID"].as_str().unwrap().to_owned()),
        )
        .unwrap(),
    ));
    let browser = Arc::new(
        webdock_browser_session::BrowserSessions::new(
            auth.clone(),
            identity.clone(),
            oauth.clone(),
            Arc::new(keys()),
            secrets["environment"]["WEBDOCK_SSO_COOKIE_SECRET"]
                .as_str()
                .unwrap(),
            webdock_browser_session::BrowserConfig::new(
                CONSOLE,
                &format!("{AUTH}/api/auth"),
                secrets["clientID"].as_str().unwrap().to_owned(),
                secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
            .unwrap(),
        )
        .unwrap(),
    );
    let bindings = directory.path().join("bindings.json");
    std::fs::write(&bindings, b"{\"runtimes\":{}}").unwrap();
    std::fs::set_permissions(&bindings, std::fs::Permissions::from_mode(0o600)).unwrap();
    let host = Arc::new(
        webdock_plugin_host::HostService::new(
            business.clone(),
            worker.clone(),
            SECRET.into(),
            ids.clone(),
            Arc::new(webdock_plugin_host::FileBindings::new(bindings.clone())),
            ed25519_dalek::SigningKey::from_bytes(&[13; 32]),
        )
        .await
        .unwrap()
        .with_session_authority(auth.clone(), identity.clone(), Some(browser.clone()))
        .await
        .unwrap(),
    );
    let plugins = Arc::new(PluginEndpoints::new(
        host,
        RuntimeConnections::new(bindings),
        None,
    ));
    let hosting = Arc::new(
        webdock_hosting::HostingService::new(
            business.clone(),
            auth.clone(),
            worker.clone(),
            SECRET.into(),
            SECRET.into(),
            ids.clone(),
        )
        .await
        .unwrap(),
    );
    let hosting = recovery_admission::with_signer(hosting, directory.path(), &identity, &browser);
    let databases = Arc::new(
        webdock_control_plane::databases::DatabaseEndpoints::new(
            Arc::new(
                webdock_databases::DatabaseService::new(
                    business.clone(),
                    auth.clone(),
                    worker.clone(),
                    identity.clone(),
                    Some(browser.clone()),
                    ids.clone(),
                    SECRET.into(),
                    SECRET,
                    AUTH,
                )
                .await
                .unwrap(),
            ),
            SECRET,
        )
        .unwrap(),
    );
    let registry_db = connect(role("webdock_registry_runtime").as_str())
        .await
        .unwrap();
    let registry = Arc::new(
        webdock_registry::RegistryService::new(
            registry_db,
            webdock_identity::RlsSigner::new(SECRET, "webdock_registry_runtime").unwrap(),
        )
        .with_allocator(ids.clone()),
    );
    let access = Arc::new(
        webdock_access::AccessService::new(
            business.clone(),
            auth.clone(),
            identity.clone(),
            ids.clone(),
            webdock_identity::RlsSigner::new(SECRET, "webdock_auth_runtime").unwrap(),
            Arc::new(keys()),
            webdock_access::AccessConfig {
                auth_origin: AUTH.parse().unwrap(),
                console_origins: vec![CONSOLE.parse().unwrap()],
                allow_local_http: false,
            },
        )
        .unwrap(),
    );
    let app = |features: &str, load_hosting: bool, load_databases: bool| {
        build_app_with_domains(
            webdock_core::config::PlatformConfig::from_map(&HashMap::from([
                ("WEBDOCK_AUTH_ORIGIN".into(), AUTH.into()),
                ("WEBDOCK_STUDIO_ORIGIN".into(), STUDIO.into()),
                ("WEBDOCK_CONSOLE_ORIGIN".into(), CONSOLE.into()),
                ("WEBDOCK_FEATURES".into(), features.into()),
            ]))
            .unwrap(),
            auth.clone(),
            Services {
                identity: Some(identity.clone()),
                oauth: Some(oauth.clone()),
                browser: Some(browser.clone()),
                plugins: Some(plugins.clone()),
                hosting: load_hosting.then(|| hosting.clone()),
                databases: load_databases.then(|| databases.clone()),
                registry: Some(registry.clone()),
                access: Some(access.clone()),
                ..Default::default()
            },
        )
    };
    let enabled = r#"{"hosting":"enabled","databases":"enabled"}"#;
    let router = app(enabled, true, true);
    let limited = request(
        &router,
        AUTH,
        "POST",
        "/api/auth/sign-in/email",
        "",
        json!({"email":"operator@example.test","password":PASSWORD}),
    )
    .await;
    assert_eq!(limited.status, StatusCode::OK);
    recovery_admission::phase(recovery_admission::RecoveryPhase::PasswordSignIn);
    let limited = limited.session();
    assert!(
        !activate(&router, &limited, HOSTING, "premature")
            .await
            .status
            .is_success()
    );
    let changed = request(&router, AUTH, "POST", "/api/auth/change-password", &limited, json!({"currentPassword":PASSWORD,"newPassword":"changed-operator-password-123","revokeOtherSessions":true})).await;
    assert_eq!(changed.status, StatusCode::OK);
    recovery_admission::phase(recovery_admission::RecoveryPhase::PasswordChanged);
    let changed = changed.session();
    let enrollment = request(
        &router,
        AUTH,
        "POST",
        "/api/auth/two-factor/enable",
        &changed,
        json!({"password":"changed-operator-password-123"}),
    )
    .await;
    assert_eq!(enrollment.status, StatusCode::OK);
    let totp = totp_rs::Totp::from_url(enrollment.body["totpURI"].as_str().unwrap()).unwrap();
    recovery_admission::phase(recovery_admission::RecoveryPhase::TotpEnrolled);
    let ready = request(
        &router,
        AUTH,
        "POST",
        "/api/auth/two-factor/verify-totp",
        &changed,
        json!({"code":totp.generate_current().to_string()}),
    )
    .await;
    assert_eq!(ready.status, StatusCode::OK);
    let native = ready.session();
    recovery_admission::phase(recovery_admission::RecoveryPhase::TotpVerified);
    let begin = request(
        &router,
        CONSOLE,
        "GET",
        "/api/sso/login?returnTo=%2Fprojects",
        "",
        Value::Null,
    )
    .await;
    assert!(
        begin.status.is_redirection(),
        "PKCE begin status {}",
        begin.status
    );
    let authorization = begin.location();
    let authorize_path = format!(
        "{}?{}",
        authorization.path(),
        authorization.query().unwrap()
    );
    assert!(
        authorization
            .query_pairs()
            .any(|(k, v)| k == "code_challenge_method" && v == "S256")
    );
    let granted = request(&router, AUTH, "GET", &authorize_path, &native, Value::Null).await;
    assert!(granted.status.is_redirection());
    let callback = granted.location();
    let callback_path = format!("{}?{}", callback.path(), callback.query().unwrap());
    let issued = request(
        &router,
        CONSOLE,
        "GET",
        &callback_path,
        &begin.session(),
        Value::Null,
    )
    .await;
    assert!(issued.status.is_redirection());
    let console = issued.session();
    let session = request(
        &router,
        CONSOLE,
        "GET",
        "/api/console/session",
        &console,
        Value::Null,
    )
    .await;
    assert_eq!(session.status, StatusCode::OK);
    assert_eq!(session.body["data"]["user"]["role"], "operator");

    recovery_admission::phase(recovery_admission::RecoveryPhase::PkceConsoleOperator);
    if database_reads::enabled() {
        database_reads::foundation(database_reads::Foundation {
            app: &router,
            auth: &auth,
            owner: &owner,
            identity: &identity,
            oauth: &oauth,
            browser: &browser,
            databases: &databases,
            registry: &registry,
            native: &native,
            console: &console,
            private_directory: directory.path(),
        })
        .await;
        return;
    }
    if recovery_admission::enabled() {
        let recovery_directory = directory.keep();
        recovery_admission::check(
            &router,
            &console,
            &fixture,
            &hosting,
            (&auth, &owner),
            &recovery_directory,
            recovery_admission::mail_component::enabled().then(|| {
                recovery_admission::mail_component::FoundationDependencies {
                    mail: webdock_mail::MailDependencies {
                        auth: auth.clone(),
                        business: business.clone(),
                        worker: worker.clone(),
                        identity: identity.clone(),
                        browser: Some(browser.clone()),
                        ids: ids.clone(),
                    },
                    oauth: oauth.clone(),
                    registry: registry.clone(),
                    access: access.clone(),
                }
            }),
        )
        .await;
        return;
    }

    if std::env::var("WEBDOCK_ALLOWANCE_TEST").as_deref() == Ok("1") {
        allowance_configuration::check(allowance_configuration::Context {
            app: &router,
            owner: &owner,
            business: &business,
            ids: &ids,
            secrets: &secrets,
            auth: &auth,
            oauth: &oauth,
            keys: &keys(),
            native: &native,
            console: &console,
            identity: &identity,
            access: &access,
            browser: &browser,
        })
        .await;
        let private_directory = directory.path().to_path_buf();
        directory.close().unwrap();
        assert!(!private_directory.exists());
        let removed = docker()
            .args(["rm", "--force", "--volumes", &fixture.container])
            .output()
            .unwrap();
        assert!(removed.status.success(), "owned allowance fixture cleanup");
        assert!(
            !docker()
                .args(["inspect", &fixture.container])
                .output()
                .unwrap()
                .status
                .success()
        );
        for mount in mounts
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["Type"] == "volume")
        {
            let volume = mount["Name"].as_str().unwrap();
            assert!(
                !docker()
                    .args(["volume", "inspect", volume])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
            eprintln!(
                "native_allowance_cleanup container={} volume={} private_directory={} removed=true",
                fixture.container,
                volume,
                private_directory.display()
            );
        }
        return;
    }

    for origin in [None, Some("https://attacker.example.test")] {
        let mut input = Request::builder()
            .method("POST")
            .uri(HOSTING)
            .header("host", "console.example.test")
            .header("cookie", &console)
            .header("content-type", "application/json");
        if let Some(origin) = origin {
            input = input.header("origin", origin);
        }
        let response = router
            .clone()
            .oneshot(
                input
                    .body(Body::from(r#"{"idempotencyKey":"origin-denial"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    for absent in [app(enabled, false, false), app("{}", true, true)] {
        assert!(
            !activate(&absent, &console, HOSTING, "missing")
                .await
                .status
                .is_success()
        );
        assert!(
            !activate(&absent, &console, DATABASES, "missing")
                .await
                .status
                .is_success()
        );
    }
    assert!(
        !activate(
            &router,
            &console,
            "/api/console/plugins/builtins/mail/activate",
            "mail"
        )
        .await
        .status
        .is_success()
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.plugin_installation"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_auth.plugin_operation"
        )
        .await,
        0
    );

    if std::env::var("WEBDOCK_NATIVE_TOKEN_LOCK_TEST").as_deref() == Ok("1") {
        native_token_lock::check(native_token_lock::Context {
            app: &router,
            owner: &owner,
            auth: &auth,
            business: &business,
            native: &native,
            secrets: &secrets,
            keys: &keys(),
            browser: &browser,
            oauth: &oauth,
        })
        .await;
    }
    // Same-key races and a lost reply retain one effect and the original receipt.
    let lost_reply = Request::builder()
        .method("POST")
        .uri(HOSTING)
        .header("host", "console.example.test")
        .header("origin", CONSOLE)
        .header("cookie", &console)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"idempotencyKey":"hosting-first"}"#))
        .unwrap();
    let (a, b) = tokio::join!(
        router.clone().oneshot(lost_reply),
        activate(&router, &console, HOSTING, "hosting-first")
    );
    let a = a.unwrap();
    assert_eq!(a.status(), StatusCode::OK);
    assert_eq!(b.status, StatusCode::OK);
    assert_eq!(b.body["version"], env!("CARGO_PKG_VERSION"));
    // The first reply is discarded without reading its committed receipt body.
    drop(a);
    let retained = b.body;
    assert_eq!(
        activate(&router, &console, HOSTING, "hosting-first")
            .await
            .body,
        retained
    );
    assert_eq!(
        activate(&router, &console, HOSTING, "different-key")
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        activate(&router, &console, DATABASES, "hosting-first")
            .await
            .status,
        StatusCode::CONFLICT
    );
    let (a, b) = tokio::join!(
        activate(&router, &console, DATABASES, "db-a"),
        activate(&router, &console, DATABASES, "db-b")
    );
    assert!(
        (a.status == StatusCode::OK && b.status == StatusCode::CONFLICT)
            || (b.status == StatusCode::OK && a.status == StatusCode::CONFLICT)
    );
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.plugin_installation WHERE kind='builtin' AND state='enabled' AND desired_state='enabled' AND runtime='absent' AND generation=0 AND current_generation IS NULL AND candidate_generation IS NULL AND image_digest IS NULL").await,2);
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.plugin_operation WHERE status='succeeded' AND phase='activated'").await,2);
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.access_event WHERE action='plugin.builtin.activated'").await,2);
    for table in [
        "plugin_runtime",
        "plugin_configuration",
        "plugin_grant",
        "plugin_feature_policy",
        "plugin_feature_customer",
        "plugin_workload",
        "plugin_install_execution",
        "plugin_release",
    ] {
        assert_eq!(
            scalar(
                &owner,
                &format!("SELECT count(*)::bigint AS n FROM webdock_auth.{table}")
            )
            .await,
            0
        );
    }
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_admin.customers"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_admin.projects"
        )
        .await,
        0
    );

    // Only negative cases alter already legitimately created state in this owned database.
    let principal = identity
        .authenticate_cookie(&native)
        .await
        .unwrap()
        .unwrap();
    let customer = registry
        .save_customer(
            &principal,
            None,
            json!({"name":"Denial scopes only","country":"ch"}),
        )
        .await
        .unwrap();
    sql(
        &owner,
        "UPDATE webdock_auth.plugin_bootstrap SET mode='legacy'",
        vec![],
    )
    .await;
    assert_eq!(
        activate(&router, &console, HOSTING, "hosting-first")
            .await
            .status,
        StatusCode::CONFLICT
    );
    sql(
        &owner,
        "UPDATE webdock_auth.plugin_bootstrap SET mode='fresh'",
        vec![],
    )
    .await;
    sql(&owner,"UPDATE webdock_auth.plugin_installation SET desired_state='disabled',state='disabled' WHERE id='hosting'",vec![]).await;
    assert_eq!(
        activate(&router, &console, HOSTING, "disabled-conflict")
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.plugin_installation WHERE id='hosting' AND kind='builtin' AND desired_state='disabled' AND state='disabled'").await,1);
    sql(&owner,"UPDATE webdock_auth.plugin_installation SET kind='external',version='negative-existing',desired_state='disabled',state='disabled' WHERE id='hosting'",vec![]).await;
    assert_eq!(
        activate(&router, &console, HOSTING, "external-conflict")
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.plugin_installation WHERE id='hosting' AND kind='external' AND version='negative-existing' AND desired_state='disabled' AND state='disabled'").await,1);
    sql(
        &owner,
        "UPDATE webdock_admin.users SET role='reader'",
        vec![],
    )
    .await;
    assert_eq!(
        activate(&router, &console, HOSTING, "hosting-first")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    sql(
        &owner,
        "UPDATE webdock_admin.users SET role='operator'",
        vec![],
    )
    .await;
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET role='user'",
        vec![],
    )
    .await;
    // Negative membership only; never an operator, session, module or setup prerequisite.
    sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") SELECT $1,organization_id,$2,'member',now() FROM webdock_auth.tenant_customer WHERE customer_id=$3",vec![format!("denial-{}",uuid::Uuid::new_v4().simple()).into(),principal.user_id.clone().into(),customer["id"].as_str().unwrap().into()]).await;
    assert_eq!(
        activate(&router, &console, HOSTING, "hosting-first")
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    sql(
        &owner,
        "UPDATE webdock_auth.\"user\" SET role='operator'",
        vec![],
    )
    .await;

    // The real customer exists only for denial contexts, never as setup ownership.
    for mode in ["read_only", "owner"] {
        let preview = request(
            &router,
            CONSOLE,
            "POST",
            "/api/console/tenant-preview/start",
            &console,
            json!({"customerID":customer["id"],"mode":mode}),
        )
        .await;
        assert_eq!(preview.status, StatusCode::OK);
        assert_eq!(
            activate(&router, &console, HOSTING, "hosting-first")
                .await
                .status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &router,
                CONSOLE,
                "POST",
                "/api/console/tenant-preview/exit",
                &console,
                json!({})
            )
            .await
            .status,
            StatusCode::OK
        );
    }
    if std::env::var("WEBDOCK_NATIVE_TOKEN_LOCK_TEST").as_deref() == Ok("1") {
        native_token_lock::refresh_regression(&router, &owner, &native, &oauth).await;
    }
    let sign_out = request(
        &router,
        AUTH,
        "POST",
        "/api/auth/sign-out",
        &native,
        json!({}),
    )
    .await;
    assert_eq!(sign_out.status, StatusCode::OK);
    assert!(
        !activate(&router, &console, HOSTING, "hosting-first")
            .await
            .status
            .is_success()
    );
    sql(
        &owner,
        "UPDATE webdock_auth.native_recovery_epoch SET state='quarantined'",
        vec![],
    )
    .await;
    assert_eq!(
        activate(&router, &console, HOSTING, "quarantined")
            .await
            .status,
        StatusCode::LOCKED
    );
    assert_eq!(scalar(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.plugin_operation WHERE payload->>'action'='activateBuiltin'").await,2);
}
