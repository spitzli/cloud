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
    actual_core(NativeMode::Legacy).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP write using native ID driver"]
async fn actual_http_write_uses_native_postgres_id_backend() {
    actual_core(NativeMode::Ids).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual authenticated HTTP native session read"]
async fn actual_http_session_read_uses_native_postgres() {
    actual_core(NativeMode::SessionReads).await;
}

#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual native HTTP session listing/revocation/logout"]
async fn actual_http_session_management_uses_native_postgres() {
    actual_core(NativeMode::SessionManagement).await;
}
#[tokio::test]
#[ignore = "owned local PostgreSQL18; actual HTTP first-party login persists native nonce"]
async fn actual_http_browser_login_begin_uses_native_flow_storage() {
    actual_core(NativeMode::BrowserFlows).await;
}
#[derive(Clone, Copy)]
enum NativeMode {
    Legacy,
    Ids,
    SessionReads,
    SessionManagement,
    BrowserFlows,
}
async fn actual_core(mode: NativeMode) {
    let native = matches!(mode, NativeMode::Ids);
    let native_session = matches!(mode, NativeMode::SessionReads);
    let native_management = matches!(mode, NativeMode::SessionManagement);
    let native_browser = matches!(mode, NativeMode::BrowserFlows);
    let fixture = fixture::DatabaseFixture::start().await;
    let owner = fixture.owner().await;
    if native_session || native_management {
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('owned-session-user','Owned user','session@example.test',true,'user',false,false); INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('owned-session','owned-core-session-token','owned-session-user',now()+interval '1 hour',now())"#).await.unwrap();
    }
    if native_management {
        owner.execute_unprepared(r#"INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES('foreign-user','Foreign','foreign@example.test',true,'user',false,false);
        INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES('owned-other-session','owned-other-token','owned-session-user',now()+interval '1 hour',now()),('foreign-session','foreign-token','foreign-user',now()+interval '1 hour',now())"#).await.unwrap();
    }
    if native_browser {
        let client_hash = webdock_oauth::legacy_token_hash("owned-core-browser-secret");
        sql(&owner,r#"INSERT INTO webdock_auth."oauthClient"(id,"clientId","clientSecret",name,disabled,"skipConsent","enableEndSession",scopes,"redirectUris","postLogoutRedirectUris","grantTypes","responseTypes","tokenEndpointAuthMethod","requirePKCE","subjectType",metadata,"createdAt","updatedAt") VALUES('103','102',$1,'Owned browser',false,true,true,'["openid","profile","email"]','["http://localhost:3121/api/sso/callback","http://localhost:3120/api/sso/callback"]','["http://localhost:3121/login"]','["authorization_code","refresh_token"]','["code"]','client_secret_post',true,'public','{"webdock_binding":"104"}',now(),now())"#,vec![client_hash.into()]).await;
        owner.execute_unprepared("INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id,enabled) VALUES('104','102','Owned browser',null,true);CREATE TABLE public.owned_browser_http_audit(app text);CREATE FUNCTION public.owned_browser_http_audit() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog AS $$ BEGIN INSERT INTO public.owned_browser_http_audit VALUES(current_setting('application_name'));RETURN NEW;END $$;CREATE TRIGGER owned_browser_http_audit AFTER INSERT ON webdock_auth.identity_browser_flow FOR EACH ROW EXECUTE FUNCTION public.owned_browser_http_audit()").await.unwrap();
    }
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
        )
        .env(
            "WEBDOCK_SESSION_READ_BACKEND",
            if native_session {
                "tokio-postgres"
            } else {
                "sea-orm"
            },
        );
    command.env(
        "WEBDOCK_SESSION_MANAGEMENT_BACKEND",
        if native_management {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    command.env(
        "WEBDOCK_BROWSER_FLOW_BACKEND",
        if native_browser {
            "tokio-postgres"
        } else {
            "sea-orm"
        },
    );
    if native_browser {
        command
            .env("WEBDOCK_SSO_CLIENT_ID", "102")
            .env("WEBDOCK_SSO_CLIENT_SECRET", "owned-core-browser-secret")
            .env("WEBDOCK_SSO_COOKIE_SECRET", secret);
    }
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
    if native_session {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared("LOCK TABLE webdock_auth.session IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let cookie = CookiePolicy::new(false, secret)
            .unwrap()
            .header("owned-core-session-token", 3600)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let request_client = client.clone();
        let request = tokio::spawn(async move {
            request_client
                .get(format!("http://127.0.0.1:{port}/api/auth/get-session"))
                .header("Host", "localhost:3125")
                .header("Cookie", cookie)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap()
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let active=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%s.token=$1%' AND query LIKE '%native_provider_session_live%') AS native_read")).await.unwrap().unwrap().try_get::<bool>("","native_read").unwrap();
            if active {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "actual authenticated HTTP must execute the native session query"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        let response = request.await.unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["user"]["id"], "owned-session-user");
    }
    if native_management {
        let gate = owner.begin().await.unwrap();
        gate.execute_unprepared("LOCK TABLE webdock_auth.session IN ACCESS EXCLUSIVE MODE")
            .await
            .unwrap();
        let auth_cookie = CookiePolicy::new(false, secret)
            .unwrap()
            .header("owned-core-session-token", 3600)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let request_client = client.clone();
        let request_cookie = auth_cookie.clone();
        let request = tokio::spawn(async move {
            request_client
                .get(format!("http://127.0.0.1:{port}/api/auth/list-sessions"))
                .header("Host", "localhost:3125")
                .header("Cookie", request_cookie)
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .unwrap()
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let active=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name='spitzli-native-postgres' AND usename='webdock_auth_bootstrap' AND wait_event_type='Lock' AND query LIKE '%FOR UPDATE OF u,s%') AS native_management")).await.unwrap().unwrap().try_get::<bool>("","native_management").unwrap();
            if active {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "actual management HTTP must execute native canonical row locking"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gate.commit().await.unwrap();
        let response = request.await.unwrap();
        assert_eq!(response.status(), 200);
        let rows: Value = response.json().await.unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r["id"] != "foreign-session"));
        for (path, body) in [
            ("revoke-session-by-id", json!({"id":"foreign-session"})),
            ("revoke-other-sessions", json!({})),
            ("sign-out", json!({})),
        ] {
            let response = client
                .post(format!("http://127.0.0.1:{port}/api/auth/{path}"))
                .header("Host", "localhost:3125")
                .header("Origin", "http://localhost:3125")
                .header("Cookie", auth_cookie.clone())
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        }
        let counts=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT count(*) FILTER(WHERE id='foreign-session') AS foreign_count,count(*) FILTER(WHERE \"userId\"='owned-session-user') AS own_count FROM webdock_auth.session")).await.unwrap().unwrap();
        assert_eq!(counts.try_get::<i64>("", "foreign_count").unwrap(), 1);
        assert_eq!(counts.try_get::<i64>("", "own_count").unwrap(), 0);
        let response = client
            .get(format!("http://127.0.0.1:{port}/api/auth/get-session"))
            .header("Host", "localhost:3125")
            .header("Cookie", auth_cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.json::<Value>().await.unwrap().is_null());
    }
    if native_browser {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client
            .get(format!(
                "http://127.0.0.1:{port}/api/sso/login?returnTo=%2Fprojects"
            ))
            .header("Host", "localhost:3121")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 303);
        let location = response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap();
        let location = url::Url::parse(location).unwrap();
        assert_eq!(
            location.origin().ascii_serialization(),
            "http://localhost:3125"
        );
        let params = location
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params["client_id"], "102");
        assert_eq!(params["code_challenge_method"], "S256");
        assert!(!params["state"].is_empty());
        assert!(!params["nonce"].is_empty());
        assert!(
            response
                .headers()
                .get("set-cookie")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("HttpOnly")
        );
        let row=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT (SELECT count(*) FROM public.owned_browser_http_audit WHERE app='spitzli-native-postgres') AS native_writes,(SELECT payload FROM webdock_auth.identity_browser_flow LIMIT 1) AS payload")).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "native_writes").unwrap(), 1);
        assert!(
            !row.try_get::<String>("", "payload")
                .unwrap()
                .contains(params["state"].as_ref())
        );
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
