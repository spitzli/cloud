//! Complete offline setup in an exclusively owned PostgreSQL container, never shared-role mutation.
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const SECRET: &str = "owned-operator-bootstrap-secret-at-least32";
const PASSWORD: &str = "private-initial-operator-password-123";
const AUTH: &str = "https://auth.example.test";
const STUDIO: &str = "https://studio.example.test";
const CONSOLE: &str = "https://console.example.test";
#[path = "support/plugin_builtins.rs"]
mod plugin_builtins;
fn docker() -> Command {
    let mut command = Command::new("docker");
    command.args(["--host", "unix:///var/run/docker.sock"]);
    command
}
struct Fixture {
    container: String,
    url: url::Url,
    passwords: BTreeMap<String, String>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = docker()
            .args(["rm", "--force", "--volumes", &self.container])
            .output();
    }
}
impl Fixture {
    async fn start() -> Self {
        assert_eq!(
            std::env::var("WEBDOCK_BOOTSTRAP_DOCKER_TEST").as_deref(),
            Ok("1")
        );
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let password = format!("{}{}", nonce, uuid::Uuid::new_v4().simple());
        let created = docker()
            .args([
                "run",
                "--detach",
                "--name",
                &format!("webdock-operator-{}", &nonce[..12]),
                "--label",
                &format!("webdock.rust-operator={nonce}"),
                "--env",
                "POSTGRES_PASSWORD",
                "--env",
                "POSTGRES_DB=webdock_operator_test",
                "--publish",
                "127.0.0.1::5432",
                "postgres:18-alpine",
            ])
            .env("POSTGRES_PASSWORD", &password)
            .output()
            .unwrap();
        assert!(created.status.success(), "owned fixture startup");
        let container = String::from_utf8(created.stdout).unwrap().trim().to_owned();
        assert_eq!(container.len(), 64);
        assert!(container.bytes().all(|b| b.is_ascii_hexdigit()));
        let port = docker()
            .args(["port", &container, "5432/tcp"])
            .output()
            .unwrap();
        assert!(port.status.success());
        let port = String::from_utf8(port.stdout).unwrap();
        let port = port.trim();
        assert!(port.starts_with("127.0.0.1:"));
        let mut url = url::Url::parse(&format!(
            "postgresql://postgres@{port}/webdock_operator_test"
        ))
        .unwrap();
        url.set_password(Some(&password)).unwrap();
        let passwords = webdock_persistence::roles::LOGIN_ROLES
            .iter()
            .copied()
            .chain(["webdock_offline_migrate"])
            .map(|role| {
                (
                    role.into(),
                    format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    ),
                )
            })
            .collect();
        let fixture = Self {
            container,
            url,
            passwords,
        };
        let owner = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(db) = connect(fixture.url.as_str()).await {
                    break db;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        webdock_persistence::roles::provision_roles(
            &owner,
            "webdock_operator_test",
            &fixture.passwords,
        )
        .await
        .unwrap();
        webdock_persistence::migrations::bootstrap(&owner, "webdock_operator_test", SECRET)
            .await
            .unwrap();
        owner.close().await.unwrap();
        fixture
    }
    fn role(&self, role: &str, database: &str) -> url::Url {
        let mut url = self.url.clone();
        url.set_path(&format!("/{database}"));
        url.set_username(role).unwrap();
        url.set_password(Some(&self.passwords[role])).unwrap();
        url
    }
    async fn additional_database(&self, name: &str) -> DatabaseConnection {
        let owner = connect(self.url.as_str()).await.unwrap();
        sql(&owner, &format!("CREATE DATABASE {name}"), vec![]).await;
        owner.close().await.unwrap();
        let mut url = self.url.clone();
        url.set_path(&format!("/{name}"));
        let db = connect(url.as_str()).await.unwrap();
        webdock_persistence::migrations::bootstrap(&db, name, SECRET)
            .await
            .unwrap();
        db
    }
}
async fn connect(url: &str) -> Result<DatabaseConnection, sea_orm::DbErr> {
    let mut options = ConnectOptions::new(url);
    options
        .max_connections(1)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(1))
        .sqlx_logging(false);
    Database::connect(options).await
}
async fn sql(db: &impl ConnectionTrait, query: &str, values: Vec<sea_orm::Value>) {
    if values.is_empty() {
        db.execute_unprepared(query).await.unwrap();
    } else {
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            query,
            values,
        ))
        .await
        .unwrap();
    }
}
async fn scalar(db: &DatabaseConnection, query: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, query))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
fn input(path: &Path) {
    std::fs::write(path,serde_json::to_vec(&json!({"email":"Operator@Example.Test","name":"First operator","password":PASSWORD,"authOrigin":AUTH,"studioOrigin":STUDIO,"consoleOrigin":CONSOLE})).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
async fn command(
    url: &url::Url,
    target: &str,
    config: &Path,
    output: &Path,
) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_webdock-bootstrap-operator"))
            .env_clear()
            .env("DATABASE_URL_UNPOOLED", url.as_str())
            .env("WEBDOCK_AUTH_SECRET", SECRET)
            .env("BETTER_AUTH_URL", AUTH)
            .env("WEBDOCK_STUDIO_ORIGIN", STUDIO)
            .env("WEBDOCK_CONSOLE_ORIGIN", CONSOLE)
            .args(["--target", target, "--config"])
            .arg(config)
            .arg("--output")
            .arg(output)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap()
}
fn target(url: &url::Url) -> String {
    format!(
        "{}:{}{}",
        url.host_str().unwrap(),
        url.port().unwrap_or(5432),
        url.path()
    )
}
fn failed(output: &std::process::Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"operator_bootstrap_failed\n");
}
fn redacted(output: &std::process::Output) {
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains(PASSWORD));
        assert!(!text.contains(SECRET));
    }
}
async fn empty(db: &DatabaseConnection) {
    for table in [
        "webdock_auth.\"user\"",
        "webdock_auth.account",
        "webdock_admin.users",
        "webdock_auth.\"oauthClient\"",
        "webdock_auth.app_binding",
        "webdock_auth.jwks",
    ] {
        assert_eq!(
            scalar(db, &format!("SELECT count(*)::bigint AS n FROM {table}")).await,
            0
        );
    }
}

#[tokio::test]
#[ignore = "creates/removes one exclusively owned Docker PostgreSQL fixture and its own roles"]
async fn offline_bootstrap_is_atomic_private_usable_and_duplicate_safe() {
    let fixture = Fixture::start().await;
    let db = connect(fixture.url.as_str()).await.unwrap();
    let offline = fixture.role("webdock_offline_migrate", "webdock_operator_test");
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("operator.json");
    input(&config);
    let output = directory.path().join("private.json");
    // Original authenticated authority survives PostgreSQL session/role masquerading.
    for statement in [
        "SET ROLE webdock_auth_bootstrap",
        "SET SESSION AUTHORIZATION webdock_auth_bootstrap",
    ] {
        sql(&db, statement, vec![]).await;
        assert!(
            webdock_control_plane::db::require_runtime_role(&db, "webdock_auth_bootstrap")
                .await
                .is_err()
        );
        sql(&db, "RESET SESSION AUTHORIZATION; RESET ROLE", vec![]).await;
    }
    for url in [
        &fixture.url,
        &fixture.role("webdock_auth_bootstrap", "webdock_operator_test"),
    ] {
        failed(&command(url, &target(url), &config, &output).await);
        assert!(!output.exists());
    }
    failed(&command(&offline, "127.0.0.1:1/wrong", &config, &output).await);
    assert!(!output.exists());
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
    failed(&command(&offline, &target(&offline), &config, &output).await);
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&output, "retained private output").unwrap();
    failed(&command(&offline, &target(&offline), &config, &output).await);
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        "retained private output"
    );
    std::fs::remove_file(&output).unwrap();
    empty(&db).await;
    sql(
        &db,
        "UPDATE webdock_api_migrations.baseline SET checksum=repeat('0',64)",
        vec![],
    )
    .await;
    failed(&command(&offline, &target(&offline), &config, &output).await);
    assert!(!output.exists());
    sql(
        &db,
        "UPDATE webdock_api_migrations.baseline SET checksum=$1",
        vec![
            hex::encode(sha2::Sha256::digest(include_bytes!(
                "../../persistence/schema/baseline.sql"
            )))
            .into(),
        ],
    )
    .await;
    sql(
        &db,
        "UPDATE webdock_api_migrations.rust_journal SET success=false WHERE version=1",
        vec![],
    )
    .await;
    failed(&command(&offline, &target(&offline), &config, &output).await);
    assert!(!output.exists());
    sql(
        &db,
        "UPDATE webdock_api_migrations.rust_journal SET success=true WHERE version=1",
        vec![],
    )
    .await;
    for (seed, cleanup) in [
        (
            "INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"mustChangePassword\") VALUES('collision','Existing','operator@example.test',false,'user',true)",
            "DELETE FROM webdock_auth.\"user\" WHERE id='collision'",
        ),
        (
            "INSERT INTO webdock_admin.users(id,name,email,role) VALUES('91','Existing','operator@example.test','reader')",
            "DELETE FROM webdock_admin.users WHERE id='91'",
        ),
        (
            "INSERT INTO webdock_auth.\"oauthClient\"(id,\"clientId\",\"redirectUris\") VALUES('92','92','[]')",
            "DELETE FROM webdock_auth.\"oauthClient\" WHERE id='92'",
        ),
        (
            "INSERT INTO webdock_auth.access_event(id,actor_id,action,outcome) VALUES('93','prior','operator.bootstrap','succeeded')",
            "DELETE FROM webdock_auth.access_event WHERE id='93'",
        ),
    ] {
        sql(&db, seed, vec![]).await;
        failed(&command(&offline, &target(&offline), &config, &output).await);
        assert!(!output.exists());
        sql(&db, cleanup, vec![]).await;
    }
    sql(&db,"CREATE FUNCTION public.fail_initial_key() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned failure before commit'; END $$; CREATE TRIGGER owned_failure AFTER INSERT ON webdock_auth.jwks FOR EACH ROW EXECUTE FUNCTION public.fail_initial_key()",vec![]).await;
    failed(&command(&offline, &target(&offline), &config, &output).await);
    assert!(!output.exists());
    empty(&db).await;
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint AS n FROM webdock_auth.access_event"
        )
        .await,
        0
    );
    sql(
        &db,
        "DROP TRIGGER owned_failure ON webdock_auth.jwks; DROP FUNCTION public.fail_initial_key()",
        vec![],
    )
    .await;
    let second = directory.path().join("second.json");
    let reviewed = target(&offline);
    let (first_result, second_result) = tokio::join!(
        command(&offline, &reviewed, &config, &output),
        command(&offline, &reviewed, &config, &second)
    );
    assert_ne!(
        first_result.status.success(),
        second_result.status.success()
    );
    redacted(&first_result);
    redacted(&second_result);
    let (winner, summary, loser) = if first_result.status.success() {
        (&output, &first_result, &second)
    } else {
        (&second, &second_result, &output)
    };
    assert!(!loser.exists());
    assert_eq!(winner.metadata().unwrap().mode() & 0o777, 0o600);
    assert!(summary.stderr.is_empty());
    let summary: Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(summary["securitySetupRequired"], true);
    let secrets: Value = serde_json::from_slice(&std::fs::read(winner).unwrap()).unwrap();
    assert_eq!(secrets["status"], "verify-command-result");
    assert_eq!(secrets["operatorID"], summary["operatorID"]);
    assert_eq!(secrets["clientID"], summary["clientID"]);
    assert_eq!(scalar(&db,"SELECT count(*)::bigint AS n FROM webdock_auth.access_event WHERE action IN ('operator.bootstrap','console.bootstrap')").await,2);
    assert_eq!(scalar(&db,"SELECT count(*)::bigint AS n FROM webdock_auth.\"user\" WHERE role='operator' AND \"emailVerified\" AND \"mustChangePassword\" AND NOT \"twoFactorEnabled\"").await,1);
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint AS n FROM webdock_auth.session"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint AS n FROM webdock_auth.\"twoFactor\""
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint AS n FROM webdock_auth.identity_delivery_job"
        )
        .await,
        0
    );
    let no_overwrite = std::fs::read(winner).unwrap();
    failed(&command(&offline, &reviewed, &config, winner).await);
    assert_eq!(std::fs::read(winner).unwrap(), no_overwrite);
    usable(&fixture, &db, &secrets).await;
    lost_commit_ack(&fixture, directory.path(), &config).await;
    db.close().await.unwrap();
}
use sha2::Digest;
fn cookie(value: &str) -> &str {
    value.split(';').next().unwrap()
}
async fn usable(fixture: &Fixture, owner: &DatabaseConnection, secrets: &Value) {
    let auth = connect(
        fixture
            .role("webdock_auth_bootstrap", "webdock_operator_test")
            .as_str(),
    )
    .await
    .unwrap();
    let registry_db = connect(
        fixture
            .role("webdock_registry_runtime", "webdock_operator_test")
            .as_str(),
    )
    .await
    .unwrap();
    let allocator = Arc::new(
        webdock_persistence::IdAllocator::connect(
            fixture
                .role("webdock_auth_runtime", "webdock_operator_test")
                .as_str(),
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
        .with_allocator(allocator.clone()),
    );
    let registry = webdock_registry::RegistryService::new(
        registry_db.clone(),
        webdock_identity::RlsSigner::new(SECRET, "webdock_registry_runtime").unwrap(),
    )
    .with_allocator(allocator.clone());
    let webdock_identity::PasswordAuthentication::SetupRequired {
        session: Some(limited),
        ..
    } = identity
        .authenticate_password("operator@example.test", PASSWORD)
        .await
        .unwrap()
    else {
        panic!("limited setup required");
    };
    assert!(!limited.principal.can_access());
    assert!(
        registry
            .list(&limited.principal, json!({"collection":"customers"}))
            .await
            .is_err()
    );
    let changed = identity
        .change_password(
            cookie(&limited.set_cookie),
            PASSWORD,
            "changed-operator-password-123",
        )
        .await
        .unwrap();
    assert!(!changed.principal.can_access());
    let enrollment = identity
        .begin_totp_enrollment(cookie(&changed.set_cookie), "changed-operator-password-123")
        .await
        .unwrap();
    let totp = totp_rs::Totp::from_url(&enrollment.totp_uri).unwrap();
    let ready = identity
        .confirm_totp_enrollment(
            cookie(&changed.set_cookie),
            &totp.generate_current().to_string(),
        )
        .await
        .unwrap();
    assert!(ready.principal.is_operator());
    let customer = registry
        .save_customer(
            &ready.principal,
            None,
            json!({"name":"Fresh install customer","country":"ch"}),
        )
        .await
        .unwrap();
    let project = registry
        .save_project(
            &ready.principal,
            None,
            json!({"name":"Fresh project","customer":customer["id"]}),
        )
        .await
        .unwrap();
    assert!(project["id"].is_string());
    let mapping = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT auth_subject FROM webdock_admin.users WHERE id=$1",
            [secrets["portalUserID"].as_str().unwrap().into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        mapping.try_get::<String>("", "auth_subject").unwrap(),
        ready.principal.user_id
    );
    let client_id = secrets["clientID"].as_str().unwrap().to_owned();
    let client_secret = secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
        .as_str()
        .unwrap()
        .to_owned();
    let oauth = Arc::new(webdock_oauth::OAuthService::new(
        auth.clone(),
        identity.clone(),
        allocator,
        Arc::new(keys()),
        webdock_oauth::OAuthConfig::new(
            &format!("{AUTH}/api/auth"),
            &format!("{STUDIO}/api/mcp"),
            Some(client_id.clone()),
        )
        .unwrap(),
    ));
    let browser = webdock_browser_session::BrowserSessions::new(
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
            client_id,
            client_secret,
        )
        .unwrap()
        .with_additional_origins(&[STUDIO.into()])
        .unwrap(),
    )
    .unwrap();
    for origin in [CONSOLE, STUDIO] {
        let begin = browser.begin(origin, "/projects").await.unwrap();
        let location = url::Url::parse(&begin.location).unwrap();
        let query = location
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let mut request = webdock_oauth::AuthorizationRequest::code(
            query["client_id"].to_string(),
            query["redirect_uri"].to_string(),
            query["scope"].to_string(),
            query["code_challenge"].to_string(),
        );
        request.state = Some(query["state"].to_string());
        request.nonce = Some(query["nonce"].to_string());
        let webdock_oauth::AuthorizationOutcome::Redirect(callback) = oauth
            .authorize(cookie(&ready.set_cookie), request)
            .await
            .unwrap()
        else {
            panic!("Portal authorization should skip consent only for reviewed first-party client");
        };
        let callback = url::Url::parse(&callback).unwrap();
        let code = callback
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        let session = browser
            .callback(
                origin,
                cookie(&begin.set_cookies[0]),
                &code,
                &query["state"],
            )
            .await
            .unwrap();
        assert_eq!(session.location, format!("{origin}/projects"));
        assert_eq!(
            browser
                .authenticate(origin, cookie(&session.set_cookies[0]))
                .await
                .unwrap()
                .unwrap()
                .user_id,
            ready.principal.user_id
        );
    }
    assert!(
        oauth.jwks().await.unwrap()["keys"]
            .as_array()
            .is_some_and(|keys| keys.len() == 1)
    );
    drop(browser);
    drop(oauth);
    drop(registry);
    drop(identity);
    auth.close().await.unwrap();
    registry_db.close().await.unwrap();
}
struct Proxy {
    port: u16,
    cut: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let kind = reader.read_u8().await?;
    let length = reader.read_u32().await?;
    if !(4..=16 * 1024 * 1024).contains(&length) {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut frame = vec![kind];
    frame.extend_from_slice(&length.to_be_bytes());
    frame.resize(length as usize + 1, 0);
    reader.read_exact(&mut frame[5..]).await?;
    Ok(frame)
}
async fn forward_requests(
    mut read: tokio::net::tcp::OwnedReadHalf,
    mut write: tokio::net::tcp::OwnedWriteHalf,
    marked: Arc<AtomicBool>,
) -> std::io::Result<()> {
    loop {
        let packet = frame(&mut read).await?;
        let insertion = b"INSERT INTO webdock_auth.\"user\"";
        if packet
            .windows(insertion.len())
            .any(|window| window == insertion)
        {
            marked.store(true, Ordering::SeqCst);
        }
        write.write_all(&packet).await?;
    }
}
async fn proxy_session(
    mut downstream: tokio::net::TcpStream,
    address: std::net::SocketAddr,
    cut: Arc<AtomicBool>,
) -> std::io::Result<()> {
    let mut upstream = tokio::net::TcpStream::connect(address).await?;
    let length = downstream.read_u32().await?;
    if !(8..=16384).contains(&length) {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    let mut startup = vec![0; length as usize];
    startup[..4].copy_from_slice(&length.to_be_bytes());
    downstream.read_exact(&mut startup[4..]).await?;
    upstream.write_all(&startup).await?;
    let (client_read, mut client_write) = downstream.into_split();
    let (mut server_read, server_write) = upstream.into_split();
    let main = Arc::new(AtomicBool::new(false));
    let requests = forward_requests(client_read, server_write, main.clone());
    let responses = async move {
        loop {
            let packet = frame(&mut server_read).await?;
            if packet[0] == b'C' && packet[5..] == *b"COMMIT\0" && main.load(Ordering::SeqCst) {
                cut.store(true, Ordering::SeqCst);
                return Ok::<(), std::io::Error>(());
            }
            client_write.write_all(&packet).await?;
        }
    };
    tokio::select! {result=requests=>result,result=responses=>result}
}
impl Proxy {
    async fn start(upstream: std::net::SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let cut = Arc::new(AtomicBool::new(false));
        let observed = cut.clone();
        let task = tokio::spawn(async move {
            let mut sessions = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let (downstream,_)=accepted.unwrap();let cut=observed.clone();
                        sessions.spawn(async move {let _=proxy_session(downstream,upstream,cut).await;});
                    },
                    finished=sessions.join_next(),if !sessions.is_empty()=>{assert!(finished.unwrap().is_ok(),"bootstrap proxy session panicked");}
                }
            }
        });
        Self { port, cut, task }
    }
}
async fn lost_commit_ack(fixture: &Fixture, dir: &Path, input: &Path) {
    let db = fixture.additional_database("webdock_uncertain_test").await;
    let proxy = Proxy::start(
        format!("127.0.0.1:{}", fixture.url.port().unwrap())
            .parse()
            .unwrap(),
    )
    .await;
    let mut offline = fixture.role("webdock_offline_migrate", "webdock_uncertain_test");
    offline.set_port(Some(proxy.port)).unwrap();
    offline.query_pairs_mut().append_pair("sslmode", "disable");
    let output = dir.join("uncertain.json");
    let result = command(&offline, &target(&offline), input, &output).await;
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr).starts_with("operator_bootstrap_commit_uncertain")
    );
    redacted(&result);
    assert!(proxy.cut.load(Ordering::SeqCst));
    assert!(!proxy.task.is_finished(), "bootstrap proxy listener failed");
    assert!(output.exists());
    assert_eq!(output.metadata().unwrap().mode() & 0o777, 0o600);
    let secrets: Value = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint AS n FROM webdock_auth.\"user\" WHERE role='operator'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT count(*)::bigint AS n FROM webdock_auth.\"oauthClient\""
        )
        .await,
        1
    );
    assert_eq!(
        scalar(&db, "SELECT count(*)::bigint AS n FROM webdock_auth.jwks").await,
        1
    );
    let actual: String = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id FROM webdock_auth.\"user\" WHERE role='operator'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
    assert_eq!(secrets["operatorID"], actual);
    let retry = dir.join("uncertain-retry.json");
    let direct = fixture.role("webdock_offline_migrate", "webdock_uncertain_test");
    failed(&command(&direct, &target(&direct), input, &retry).await);
    assert!(!retry.exists());
    drop(proxy);
    db.close().await.unwrap();
}
