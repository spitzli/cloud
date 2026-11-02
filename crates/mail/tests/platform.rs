//! Fresh restricted PostgreSQL -> production Core HTTPS -> API/worker/relay subprocesses -> TLS.
#[path = "support/sending.rs"]
mod support;
use axum::{
    Router,
    body::Body,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use futures_util::FutureExt;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use support::{KEY, PLUGIN, bindings, role, seed, sql};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    mail::{MailEndpoints, initialize_sender},
    plugin_config::MailConfig,
};
use webdock_mail::{
    MailDependencies, keys::NativeKeys, payload::PrivatePayloadStore, submissions::Submissions,
};
use webdock_mail_protocol::sha256;
use webdock_plugin_host::{FileBindings, HostService};
const WORKER_SECRET: &str = "mail-platform-owned-worker-secret-at-least32";
const PAYLOAD_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

#[path = "support/tls.rs"]
mod tls;
use tls::TlsServer;
#[derive(Default)]
struct Provider {
    requests: Mutex<Vec<Value>>,
    lose_reply: AtomicBool,
}
async fn receive(
    State(provider): State<Arc<Provider>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers["consumerkey"], "fixture-provider-key");
    assert_eq!(headers["consumersecret"], "fixture-provider-secret");
    assert!(headers.get("authorization").is_none());
    assert_eq!(body["from"], "sender@example.test");
    assert_eq!(body["to"], "target@example.test");
    assert_eq!(body["bcc"], "hidden@example.test");
    let mime = body["mime_raw"].as_str().unwrap();
    assert!(
        !mime
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("bcc:"))
    );
    assert!(mime.contains("No external delivery"));
    assert!(
        body["reference_id"]
            .as_str()
            .is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit()))
    );
    provider.requests.lock().unwrap().push(body);
    if provider.lose_reply.load(Ordering::SeqCst) {
        let broken = futures_util::stream::once(async {
            Err::<axum::body::Bytes, _>(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "owned lost receipt",
            ))
        });
        return (StatusCode::OK, Body::from_stream(broken)).into_response();
    }
    Json(json!({"mid":"98765432109876543210"})).into_response()
}
fn private(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn json_file(dir: &Path, name: &str, value: Value) -> PathBuf {
    let path = dir.join(name);
    private(&path, serde_json::to_vec(&value).unwrap());
    path
}
fn child(binary: &Path, args: &[&Path]) -> Child {
    let mut command = Command::new(binary);
    for arg in args {
        command.arg(arg);
    }
    command
        .env_clear()
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}
async fn stop(child: &mut Child) {
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .take(65537)
        .read_to_end(&mut stderr)
        .await
        .unwrap();
    assert!(stderr.len() <= 65536, "bounded subprocess diagnostics");
    let diagnostics = String::from_utf8_lossy(&stderr);
    for secret in [
        WORKER_SECRET,
        PAYLOAD_KEY,
        "fixture-provider-secret",
        "fixture-provider-key",
    ] {
        assert!(
            !diagnostics.contains(secret),
            "private credential leaked to subprocess stderr"
        );
    }
}
async fn socket_ready(path: &Path, child: &mut Child) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "relay exited during startup"
            );
            if webdock_plugin_runtime::unix_channel(path).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
async fn api_ready(origin: &str, child: &mut Child) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "Mail API exited during startup"
            );
            if client
                .get(format!("{origin}/healthz"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
async fn row(owner: &DatabaseConnection, id: &str) -> Value {
    let row = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_jsonb(j) AS value FROM webdock_mail.send_job j WHERE id=$1",
            [id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "value").unwrap()
}
async fn outcome(owner: &DatabaseConnection, id: &str, state: &str, worker: &mut Child) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                worker.try_wait().unwrap().is_none(),
                "worker exited during dispatch"
            );
            let value = row(owner, id).await;
            if value["state"] == state {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("expected durable Mail outcome")
}
async fn submit(client: &reqwest::Client, api: &str, key: &str) -> Value {
    let response=client.post(format!("{api}/v1/emails")).bearer_auth(format!("wd_mail_{}","t".repeat(43))).header("idempotency-key",key).json(&json!({"from":"sender@example.test","to":"target@example.test","bcc":"hidden@example.test","subject":"Owned TLS fixture","text":"No external delivery"})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body: Value = response.json().await.unwrap();
    assert!(!body.to_string().contains("fixture-provider-secret"));
    body
}
async fn count(owner: &DatabaseConnection, sql: &str) -> i64 {
    owner
        .query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}

#[tokio::test]
#[ignore = "build standalone relay local-provider-test, then run with an explicitly owned local RLS fixture"]
async fn native_processes_use_restricted_core_and_one_pinned_provider_attempt() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(url.host_str(), Some("127.0.0.1" | "localhost")));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!(
        "webdock_mail_platform_{}_test",
        uuid::Uuid::new_v4().simple()
    );
    sql(&admin, &format!("CREATE DATABASE {name}"), vec![]).await;
    url.set_path(&format!("/{name}"));
    let result = std::panic::AssertUnwindSafe(run(&url, &name))
        .catch_unwind()
        .await;
    sql(
        &admin,
        &format!("DROP DATABASE {name} WITH (FORCE)"),
        vec![],
    )
    .await;
    admin.close().await.unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
async fn run(url: &url::Url, name: &str) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let owner = Database::connect(url.as_str()).await.unwrap();
    webdock_persistence::migrations::bootstrap(&owner, name, KEY)
        .await
        .unwrap();
    seed(&owner).await;
    let provider = Arc::new(Provider::default());
    let state = provider.clone();
    let upstream = TlsServer::bind(move |_| {
        Router::new()
            .route("/api/v2/mail/send", post(receive))
            .with_state(state)
    })
    .await;
    let mut binding = bindings();
    binding["runtimes"][PLUGIN]["customers"]["123"]["send"]["origin"] = json!(upstream.origin);
    binding["runtimes"][PLUGIN]["customers"]["123"]["send"]["addresses"] = json!(["127.0.0.1"]);
    binding["runtimes"][PLUGIN]["customers"]["123"]["send"]["ca"] = json!(upstream.ca);
    let bindings_path = json_file(dir.path(), "bindings.json", binding);
    let signing_path = dir.path().join("signing.hex");
    private(&signing_path, hex::encode([78u8; 32]));
    let key = ed25519_dalek::SigningKey::from_bytes(&[78; 32]);
    let auth = role("AUTH_DATABASE_URL", url).await;
    let business = role("DATABASE_URL", url).await;
    let worker_db = role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", url).await;
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        business.clone(),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let identity = Arc::new(webdock_identity::IdentityService::new(
        auth.clone(),
        webdock_identity::CookiePolicy::new(false, "owned-platform-cookie-secret-at-least32")
            .unwrap(),
    ));
    let deps = MailDependencies {
        auth: auth.clone(),
        business: business.clone(),
        worker: worker_db.clone(),
        identity: identity.clone(),
        browser: None,
        ids: ids.clone(),
    };
    let host = Arc::new(
        HostService::new(
            business.clone(),
            worker_db.clone(),
            KEY.into(),
            ids,
            Arc::new(FileBindings::new(bindings_path)),
            key.clone(),
        )
        .await
        .unwrap(),
    );
    let environment = HashMap::from([
        ("DATABASE_RLS_SECRET".into(), KEY.into()),
        ("MAIL_SEND_WORKER_SECRET".into(), WORKER_SECRET.into()),
    ]);
    let sender = initialize_sender(
        deps.clone(),
        host,
        &MailConfig {
            signing_key: signing_path,
            worker_id: "mail-worker".into(),
            worker_generation: 1,
        },
        &environment,
        None,
    )
    .await
    .unwrap();
    let keys = Arc::new(NativeKeys::new(deps.clone(), KEY, true).await.unwrap());
    let queue = Arc::new(Submissions::new(deps, KEY, true).await.unwrap());
    let endpoints = Arc::new(
        MailEndpoints::sending_only(keys, WORKER_SECRET)
            .unwrap()
            .with_queue(queue)
            .with_sender(sender.clone()),
    );
    let db = auth.clone();
    let core = TlsServer::bind(move |origin| {
        let configuration = webdock_core::config::PlatformConfig::from_map(&HashMap::from([
            ("WEBDOCK_AUTH_ORIGIN".into(), origin.into()),
            ("WEBDOCK_FEATURES".into(), "{\"mail\":\"enabled\"}".into()),
        ]))
        .unwrap();
        build_app_with_domains(
            configuration,
            db,
            Services {
                identity: Some(identity),
                mail: Some(endpoints),
                ..Default::default()
            },
        )
    })
    .await;
    let core_ca = dir.path().join("core.pem");
    private(&core_ca, &core.ca);
    let callback = dir.path().join("authority.sock");
    let mut socket = webdock_plugin_runtime::UnixSocket::bind(&callback).unwrap();
    let incoming = socket.incoming().unwrap();
    let (finish_callback, callback_done) = tokio::sync::oneshot::channel();
    let authority = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(sender.service())
            .serve_with_incoming_shutdown(incoming, async {
                let _ = callback_done.await;
            })
            .await
            .unwrap();
        drop(socket);
    });
    let descriptor: String = owner
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT manifest_sha256 FROM webdock_auth.plugin_release WHERE plugin_id=$1",
            [PLUGIN.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "manifest_sha256")
        .unwrap();
    let plugin_path = dir.path().join("relay.sock");
    let uid = rustix::process::geteuid().as_raw();
    let relay_config = json_file(
        dir.path(),
        "relay.json",
        json!({"pluginID":PLUGIN,"version":"1.0.0","generation":1,"descriptorSha256":descriptor,"issuerPublicKeyHex":hex::encode(key.verifying_key().as_bytes()),"worker":{"kind":"unix","uid":uid},"corePeer":{"kind":"unix","uid":uid},"listen":{"kind":"unix","path":plugin_path},"core":{"kind":"unix","path":callback},"testOrigin":upstream.origin}),
    );
    let relay_binary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/mail-relay/target/debug/webdock-mail-relay");
    assert!(
        relay_binary.exists(),
        "Build relay with --features local-provider-test first"
    );
    let mut relay = child(&relay_binary, &[&relay_config]);
    socket_ready(&plugin_path, &mut relay).await;
    let payload_dir = dir.path().join("payloads");
    std::fs::create_dir(&payload_dir).unwrap();
    std::fs::set_permissions(&payload_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let api_origin = format!("http://127.0.0.1:{port}");
    let api_config = json_file(
        dir.path(),
        "api.json",
        json!({"port":port,"controlOrigin":core.origin,"controlSecret":WORKER_SECRET,"controlAuthority":core_ca,"payloadDirectory":payload_dir,"payloadKey":PAYLOAD_KEY}),
    );
    let mut api = child(
        Path::new(env!("CARGO_BIN_EXE_webdock-mail-api")),
        &[Path::new("--config"), &api_config],
    );
    api_ready(&api_origin, &mut api).await;
    let worker_config = json_file(
        dir.path(),
        "worker.json",
        json!({"controlOrigin":core.origin,"controlSecret":WORKER_SECRET,"controlAuthority":core_ca,"payloadDirectory":payload_dir,"payloadKey":PAYLOAD_KEY,"issuerPublicKeyHex":hex::encode(key.verifying_key().as_bytes()),"providers":[{"binding":{"pluginID":PLUGIN,"generation":1,"descriptorSha256":descriptor,"workerID":"mail-worker","workerGeneration":1,"workerCredentialSha256":sha256(WORKER_SECRET.as_bytes())},"channel":{"kind":"unix","path":plugin_path}}]}),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .add_root_certificate(reqwest::Certificate::from_pem(core.ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let untrusted = reqwest::Client::builder()
        .no_proxy()
        .tls_built_in_root_certs(false)
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    assert!(
        untrusted
            .get(format!("{}healthz", core.origin))
            .send()
            .await
            .is_err(),
        "untrusted Core certificate must fail before credentials"
    );
    let denied = client
        .post(format!("{}api/mail/gateway", core.origin))
        .bearer_auth("unauthorized-machine-secret-at-least32")
        .json(&json!({"action":"send.claim"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let denied = client
        .post(format!("{api_origin}/internal/webmail"))
        .bearer_auth(WORKER_SECRET)
        .json(&json!({"action":"lookup","token":"x".repeat(43),"emailID":"draft1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let first = submit(&client, &api_origin, "accepted-message").await;
    let first_id = first["id"].as_str().unwrap();
    assert_eq!(first["state"], "queued");
    let mut worker = child(
        Path::new(env!("CARGO_BIN_EXE_webdock-mail-worker")),
        &[&worker_config],
    );
    let accepted = outcome(&owner, first_id, "accepted", &mut worker).await;
    assert_eq!(accepted["provider_id"], "98765432109876543210");
    assert_eq!(
        submit(&client, &api_origin, "accepted-message").await["id"],
        first_id
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(count(&owner,"SELECT count(*)::bigint AS n FROM webdock_mail.provider_lease WHERE attempted_at IS NOT NULL").await,1);
    assert_eq!(
        count(
            &owner,
            "SELECT accepted AS n FROM webdock_mail.month_usage WHERE customer_id='123'"
        )
        .await,
        2
    );
    provider.lose_reply.store(true, Ordering::SeqCst);
    let second = submit(&client, &api_origin, "lost-provider-receipt").await;
    let second_id = second["id"].as_str().unwrap();
    let unknown = outcome(&owner, second_id, "unknown", &mut worker).await;
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    let payload = PrivatePayloadStore::open(&payload_dir, PAYLOAD_KEY).unwrap();
    assert!(
        payload
            .get("123", unknown["payload_ref"].as_str().unwrap())
            .is_ok()
    );
    assert_eq!(
        count(
            &owner,
            "SELECT reserved AS n FROM webdock_mail.month_usage WHERE customer_id='123'"
        )
        .await,
        2
    );
    stop(&mut worker).await;
    let replay = submit(&client, &api_origin, "lost-provider-receipt").await;
    assert_eq!(replay["id"], second_id);
    assert_eq!(replay["state"], "unknown");
    let third = submit(&client, &api_origin, "revoked-before-handoff").await;
    let third_id = third["id"].as_str().unwrap();
    sql(
        &owner,
        "UPDATE webdock_mail.api_key SET revoked_at=now() WHERE id='456'",
        vec![],
    )
    .await;
    let mut worker = child(
        Path::new(env!("CARGO_BIN_EXE_webdock-mail-worker")),
        &[&worker_config],
    );
    outcome(&owner, third_id, "cancelled", &mut worker).await;
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(count(&owner,"SELECT count(*)::bigint AS n FROM webdock_mail.provider_lease WHERE attempted_at IS NOT NULL").await,2);
    assert_eq!(
        count(
            &owner,
            "SELECT reserved AS n FROM webdock_mail.month_usage WHERE customer_id='123'"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_mail.service WHERE customer_id='123'"
        )
        .await,
        0
    );
    stop(&mut worker).await;
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["reference_id"], first_id);
        assert_eq!(requests[1]["reference_id"], second_id);
    }
    assert!(
        !payload_dir
            .join(accepted["payload_ref"].as_str().unwrap())
            .exists()
    );
    assert!(
        payload
            .get("123", unknown["payload_ref"].as_str().unwrap())
            .is_ok()
    );
    core.assert_healthy();
    upstream.assert_healthy();
    stop(&mut api).await;
    stop(&mut relay).await;
    finish_callback.send(()).unwrap();
    authority.await.unwrap();
    assert!(!plugin_path.exists());
    assert!(!callback.exists());
    drop(core);
    drop(upstream);
    drop(payload);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker_db.close().await.unwrap();
    owner.close().await.unwrap();
}
