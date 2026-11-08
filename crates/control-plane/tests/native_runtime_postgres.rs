//! Actual native binary against an exclusively owned PostgreSQL fixture.
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::{
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_plugin_host::{AdmissionInput, FileBindings, HostService};
use webdock_plugin_runtime::PeerIdentity;
#[allow(
    dead_code,
    reason = "Reuse existing owned PostgreSQL roles and migration fixture"
)]
#[path = "../../plugin-host/tests/support/jobs_fixture.rs"]
mod fixture;
fn peer() -> PeerIdentity {
    PeerIdentity::Unix {
        uid: rustix::process::geteuid().as_raw(),
    }
}
async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn journal(db: &impl ConnectionTrait) -> Value {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY version),'[]') AS journal FROM webdock_api_migrations.rust_journal j")).await.unwrap().unwrap().try_get("","journal").unwrap()
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual binary startup and SIGTERM"]
async fn actual_native_binary_starts_with_restricted_roles_without_schema_changes() {
    actual_core(false).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP write using native ID driver"]
async fn actual_http_write_uses_native_postgres_id_backend() {
    actual_core(true).await;
}

async fn actual_core(native: bool) {
    let fixture = fixture::DatabaseFixture::start().await;
    let owner = fixture.owner().await;
    let before = journal(&owner).await;
    let account_counts = || {
        Statement::from_string(
            DbBackend::Postgres,
            "SELECT jsonb_build_object('users',(SELECT count(*) FROM webdock_auth.\"user\"),'customers',(SELECT count(*) FROM webdock_admin.customers),'projects',(SELECT count(*) FROM webdock_admin.projects)) AS counts",
        )
    };
    let before_accounts: Value = owner
        .query_one_raw(account_counts())
        .await
        .unwrap()
        .unwrap()
        .try_get("", "counts")
        .unwrap();
    let password = uuid::Uuid::new_v4().simple().to_string();
    let mut urls = std::collections::BTreeMap::new();
    for role in [
        "webdock_auth_bootstrap",
        "webdock_auth_runtime",
        "webdock_worker_runtime",
        "webdock_registry_runtime",
    ] {
        owner
            .execute_unprepared(&format!("ALTER ROLE {role} PASSWORD '{password}'"))
            .await
            .unwrap();
        let mut url = fixture.url.clone();
        url.set_username(role).unwrap();
        url.set_password(Some(&password)).unwrap();
        urls.insert(role, url.to_string());
    }
    let reserve = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reserve.local_addr().unwrap().port();
    drop(reserve);
    let private = tempfile::tempdir().unwrap();
    let log = private.path().join("core.log");
    let secret = "owned-native-http-secret-at-least-32bytes";
    let mut command = Command::new(env!("CARGO_BIN_EXE_webdock-control-plane"));
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", private.path())
        .env("NODE_ENV", "development")
        .env("PORT", port.to_string())
        .env("WEBDOCK_BIND_ADDRESS", "127.0.0.1")
        .env("WEBDOCK_DATABASE_URL", &urls["webdock_auth_bootstrap"])
        .env(
            "WEBDOCK_SESSION_DATABASE_URL",
            &urls["webdock_auth_runtime"],
        )
        .env(
            "WEBDOCK_WORKER_DATABASE_URL",
            &urls["webdock_worker_runtime"],
        )
        .env(
            "WEBDOCK_REGISTRY_DATABASE_URL",
            &urls["webdock_registry_runtime"],
        )
        .env("WEBDOCK_AUTH_SECRET", secret)
        .env(
            "DATABASE_RLS_SECRET",
            "owned-plugin-jobs-rls-secret-32bytes",
        )
        .env("WEBDOCK_API_ORIGIN", format!("http://127.0.0.1:{port}"))
        .env("WEBDOCK_AUTH_ORIGIN", "http://localhost:3125")
        .env("WEBDOCK_STUDIO_ORIGIN", "http://localhost:3120")
        .env("WEBDOCK_CONSOLE_ORIGIN", "http://localhost:3121")
        .env("WEBDOCK_FEATURES", "{}")
        .env(
            "WEBDOCK_ID_BACKEND",
            if native { "tokio-postgres" } else { "sea-orm" },
        );
    let output = std::fs::File::create(&log).unwrap();
    command
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::from(output));
    let mut process = Process(command.spawn().unwrap());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "actual Core exited before readiness; owned log {}",
            log.display()
        );
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/readyz"))
            .send()
            .await
            && response.status() == 200
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual Core readiness timeout"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        client
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    if native {
        let response=client.post(format!("http://127.0.0.1:{port}/api/console/waitlist")).header("Host","localhost:3120").header("Origin","http://localhost:3120")
            .json(&json!({"email":"native-id@example.test","name":"Owned native ID test","privacyAccepted":true})).send().await.unwrap();
        assert_eq!(response.status(), 202);
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM webdock_auth.waitlist_entry WHERE email='native-id@example.test') AS entries,(SELECT count(*) FROM pg_catalog.pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap') AS native_connections")).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "entries").unwrap(), 1);
        assert!(row.try_get::<i64>("", "native_connections").unwrap() >= 1);
    }
    assert_eq!(
        before,
        journal(&owner).await,
        "startup must not mutate migration history"
    );
    assert!(
        Command::new("kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "native Core ignored SIGTERM"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(before, journal(&owner).await);
    let after_accounts: Value = owner
        .query_one_raw(account_counts())
        .await
        .unwrap()
        .unwrap()
        .try_get("", "counts")
        .unwrap();
    assert_eq!(
        before_accounts, after_accounts,
        "startup must not seed customer or identity records"
    );
    assert!(!std::fs::read_to_string(&log).unwrap().contains(&password));
    command.env("WEBDOCK_DATABASE_URL", fixture.url.as_str());
    let mut rejected = Process(command.spawn().unwrap());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = rejected.0.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "privileged runtime was not refused"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .contains("Restricted database role required")
    );
}

#[tokio::test]
async fn sigterm_cancels_stalled_database_initialization() {
    let stalled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = stalled.local_addr().unwrap().port();
    let mut process=Process(Command::new(env!("CARGO_BIN_EXE_webdock-control-plane"))
        .env_clear().env("PATH",std::env::var("PATH").unwrap())
        .env("NODE_ENV","development")
        .env("WEBDOCK_DATABASE_URL",format!("postgres://webdock_auth_bootstrap:fixture@127.0.0.1:{port}/stalled?sslmode=disable"))
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
    // Accept but never answer PostgreSQL startup: no schema/data or actual provider is involved.
    let (_socket, _) = tokio::time::timeout(Duration::from_secs(5), stalled.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(
        Command::new("kill")
            .args(["-TERM", &process.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = process.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "SIGTERM was swallowed during database startup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
