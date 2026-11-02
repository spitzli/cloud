//! Real restricted recovery/invitation outboxes -> executable dispatcher -> HTTPS native Mail queue.
#[path = "support/sending.rs"]
mod support;
#[path = "support/tls.rs"]
mod tls;
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use futures_util::FutureExt;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use support::{KEY, bindings, role, seed, sql};
use tls::TlsServer;
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    mail::{MailEndpoints, initialize_sender},
    plugin_config::MailConfig,
};
use webdock_identity::{
    CookiePolicy, DeliveryKind, DeliveryRequest, IdentityService, PasswordAuthentication,
    RlsSigner, SecretKeys,
};
use webdock_mail::{
    MailDependencies, keys::NativeKeys, payload::PrivatePayloadStore, send_api::SendApi,
    send_control::HttpSendControl, submissions::Submissions,
};
use webdock_plugin_host::{FileBindings, HostService};
const AUTH_SECRET: &str = "account-platform-owned-auth-secret-at-least32";
const WORKER_SECRET: &str = "account-platform-mail-worker-secret-at-least32";
const PAYLOAD_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
fn keys() -> SecretKeys {
    SecretKeys::new(
        BTreeMap::from([(1, AUTH_SECRET.into())]),
        1,
        Some(AUTH_SECRET.into()),
    )
    .unwrap()
}
fn private(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn config(dir: &Path, name: &str, value: Value) -> PathBuf {
    let file = dir.join(name);
    private(&file, serde_json::to_vec(&value).unwrap());
    file
}
fn role_url(key: &str, owner: &url::Url) -> String {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), owner.host_str());
    assert_eq!(url.port(), owner.port());
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    url.set_path(owner.path());
    url.into()
}
#[derive(Default)]
struct Receipts {
    lose_once: AtomicBool,
    requests: Mutex<Vec<(String, String)>>,
}
async fn receipt_boundary(
    State(state): State<Arc<Receipts>>,
    request: Request,
    next: Next,
) -> Response {
    let key = request
        .headers()
        .get("idempotency-key")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 8192).await.unwrap();
    state
        .requests
        .lock()
        .unwrap()
        .push((key, webdock_mail_protocol::sha256(&body)));
    let response = next.run(Request::from_parts(parts, Body::from(body))).await;
    if response.status() == StatusCode::ACCEPTED && state.lose_once.swap(false, Ordering::SeqCst) {
        let broken = futures_util::stream::once(async {
            Err::<axum::body::Bytes, _>(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "owned lost admission acknowledgement",
            ))
        });
        return (
            StatusCode::ACCEPTED,
            [("content-type", "application/json")],
            Body::from_stream(broken),
        )
            .into_response();
    }
    response
}
async fn once(path: &Path, success: bool) {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_webdock-account-delivery"))
            .arg(path)
            .arg("--once")
            .env_clear()
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.status.success(), success);
    assert!(output.stdout.len() <= 65536 && output.stderr.len() <= 65536);
    for raw in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(raw);
        for secret in [
            AUTH_SECRET,
            WORKER_SECRET,
            PAYLOAD_KEY,
            &format!("wd_mail_{}", "t".repeat(43)),
        ] {
            assert!(
                !text.contains(secret),
                "account dispatcher leaked configuration"
            );
        }
    }
}
async fn count(owner: &DatabaseConnection, query: &str) -> i64 {
    owner
        .query_one_raw(Statement::from_string(DbBackend::Postgres, query))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
}
async fn recovery(owner: &DatabaseConnection) -> DeliveryRequest {
    let value:String=owner.query_one_raw(Statement::from_string(DbBackend::Postgres,"SELECT payload FROM webdock_auth.identity_delivery_job ORDER BY created_at DESC,id DESC LIMIT 1")).await.unwrap().unwrap().try_get("","payload").unwrap();
    serde_json::from_str(&keys().decrypt(&value).unwrap()).unwrap()
}
#[tokio::test]
#[ignore = "creates and drops an isolated database using the explicitly owned restricted local fixture"]
async fn account_dispatcher_restart_deduplicates_lost_mail_ack_and_delivers_both_outboxes() {
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert!(matches!(url.host_str(), Some("127.0.0.1" | "localhost")));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!(
        "webdock_account_mail_{}_test",
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
    let auth = role("AUTH_DATABASE_URL", url).await;
    let business = role("DATABASE_URL", url).await;
    let worker = role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", url).await;
    let allocator = Arc::new(webdock_persistence::IdAllocator::new(
        business.clone(),
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let base_identity = Arc::new(IdentityService::new(
        auth.clone(),
        CookiePolicy::new(true, AUTH_SECRET).unwrap(),
    ));
    let deps = MailDependencies {
        auth: auth.clone(),
        business: business.clone(),
        worker: worker.clone(),
        identity: base_identity,
        browser: None,
        ids: allocator.clone(),
    };
    let binding = config(dir.path(), "bindings.json", bindings());
    let signing = dir.path().join("signing.hex");
    private(&signing, hex::encode([78u8; 32]));
    let host = Arc::new(
        HostService::new(
            business.clone(),
            worker.clone(),
            KEY.into(),
            allocator.clone(),
            Arc::new(FileBindings::new(binding)),
            ed25519_dalek::SigningKey::from_bytes(&[78; 32]),
        )
        .await
        .unwrap(),
    );
    let sender = initialize_sender(
        deps.clone(),
        host,
        &MailConfig {
            signing_key: signing,
            worker_id: "mail-worker".into(),
            worker_generation: 1,
        },
        &HashMap::from([
            ("DATABASE_RLS_SECRET".into(), KEY.into()),
            ("MAIL_SEND_WORKER_SECRET".into(), WORKER_SECRET.into()),
        ]),
        None,
    )
    .await
    .unwrap();
    let endpoints = Arc::new(
        MailEndpoints::sending_only(
            Arc::new(NativeKeys::new(deps.clone(), KEY, true).await.unwrap()),
            WORKER_SECRET,
        )
        .unwrap()
        .with_queue(Arc::new(Submissions::new(deps, KEY, true).await.unwrap()))
        .with_sender(sender),
    );
    let mut identity = None;
    let db = auth.clone();
    let ids = allocator.clone();
    let core = TlsServer::bind(|origin| {
        let account = Arc::new(
            IdentityService::new(db.clone(), CookiePolicy::new(true, AUTH_SECRET).unwrap())
                .with_secret_keys(keys())
                .with_allocator(ids)
                .with_account_origin(url::Url::parse(origin).unwrap())
                .unwrap(),
        );
        identity = Some(account.clone());
        let configuration = webdock_core::config::PlatformConfig::from_map(&HashMap::from([
            ("WEBDOCK_AUTH_ORIGIN".into(), origin.into()),
            ("WEBDOCK_FEATURES".into(), "{\"mail\":\"enabled\"}".into()),
        ]))
        .unwrap();
        build_app_with_domains(
            configuration,
            db,
            Services {
                identity: Some(account),
                mail: Some(endpoints),
                ..Default::default()
            },
        )
    })
    .await;
    let identity = identity.unwrap();
    let payload_dir = dir.path().join("payloads");
    std::fs::create_dir(&payload_dir).unwrap();
    std::fs::set_permissions(&payload_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = Arc::new(PrivatePayloadStore::open(&payload_dir, PAYLOAD_KEY).unwrap());
    let control = Arc::new(
        HttpSendControl::new(
            &core.origin,
            WORKER_SECRET,
            &[reqwest::Certificate::from_pem(core.ca.as_bytes()).unwrap()],
        )
        .unwrap(),
    );
    let receipts = Arc::new(Receipts {
        lose_once: AtomicBool::new(true),
        ..Default::default()
    });
    let boundary = receipts.clone();
    let payloads = store.clone();
    let api = TlsServer::bind(move |_| {
        SendApi::new(control, payloads)
            .router()
            .layer(middleware::from_fn_with_state(boundary, receipt_boundary))
    })
    .await;
    let authority = dir.path().join("mail.pem");
    private(&authority, &api.ca);
    let configuration = json!({"authDatabaseUrl":role_url("AUTH_DATABASE_URL",url),"businessDatabaseUrl":role_url("DATABASE_URL",url),"authSecret":AUTH_SECRET,"rlsSecret":KEY,"accountOrigin":core.origin,"mail":{"origin":api.origin,"authority":authority,"from":{"email":"sender@example.test","name":"Webdock"},"apiKey":format!("wd_mail_{}","t".repeat(43))}});
    let dispatcher = config(dir.path(), "dispatcher.json", configuration);
    let recipient = identity
        .create_local_account(
            "recipient@example.test",
            "Recipient",
            "initial-password-123",
            false,
        )
        .await
        .unwrap();
    identity
        .queue_account_activation(&recipient, None)
        .await
        .unwrap();
    let activation = recovery(&owner).await;
    assert!(activation.kind == DeliveryKind::Activation);
    once(&dispatcher, false).await;
    assert_eq!(
        count(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_mail.send_job"
        )
        .await,
        1
    );
    assert_eq!(count(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.identity_delivery_job WHERE delivered_at IS NULL AND attempts=1").await,1);
    sql(
        &owner,
        "UPDATE webdock_auth.identity_delivery_job SET next_attempt_at=now()",
        vec![],
    )
    .await;
    once(&dispatcher, true).await;
    assert_eq!(count(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.identity_delivery_job WHERE delivered_at IS NOT NULL AND attempts=2").await,1);
    assert_eq!(
        count(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_mail.send_job"
        )
        .await,
        1
    );
    let sent = owner
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id,payload_ref::text,idempotency_key FROM webdock_mail.send_job",
        ))
        .await
        .unwrap()
        .unwrap();
    let reference: String = sent.try_get("", "payload_ref").unwrap();
    assert_eq!(
        sent.try_get::<String>("", "idempotency_key").unwrap(),
        format!("account:{}", activation.id)
    );
    let message: Value = serde_json::from_slice(&store.get("123", &reference).unwrap()).unwrap();
    assert_eq!(message["subject"], "Set up your Spitzli Cloud account");
    assert!(message["text"].as_str().unwrap().contains(&activation.url));
    let token = url::Url::parse(&activation.url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "token")
        .unwrap()
        .1
        .into_owned();
    identity
        .complete_password_reset(&token, "activated-password-123")
        .await
        .unwrap();
    assert!(
        identity
            .complete_password_reset(&token, "another-password-123")
            .await
            .is_err()
    );
    let actor = identity
        .create_local_account("owner@example.test", "Owner", "owner-password-123", false)
        .await
        .unwrap();
    sql(&owner,"UPDATE webdock_auth.\"user\" SET \"emailVerified\"=true,\"mustChangePassword\"=false WHERE id=$1",vec![actor.clone().into()]).await;
    sql(&owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,'124',$2,'owner',now())",vec![allocator.allocate().await.unwrap().to_string().into(),actor.into()]).await;
    let PasswordAuthentication::Authenticated(session) = identity
        .authenticate_password("owner@example.test", "owner-password-123")
        .await
        .unwrap()
    else {
        panic!("owner signin");
    };
    let access = webdock_access::AccessService::new(
        business.clone(),
        auth.clone(),
        identity.clone(),
        allocator,
        RlsSigner::new(KEY, "webdock_auth_runtime").unwrap(),
        Arc::new(keys()),
        webdock_access::AccessConfig {
            auth_origin: url::Url::parse(&core.origin).unwrap(),
            console_origins: vec![],
            allow_local_http: false,
        },
    )
    .unwrap();
    access
        .manage_tenant(
            session.set_cookie.split(';').next().unwrap(),
            "123",
            webdock_access::TenantCommand::Invite {
                email: "recipient@example.test".into(),
                name: "Recipient".into(),
                role: webdock_access::MemberRole::Member,
            },
        )
        .await
        .unwrap();
    once(&dispatcher, true).await;
    assert_eq!(count(&owner,"SELECT count(*)::bigint AS n FROM webdock_auth.access_delivery_job WHERE state='delivered'").await,1);
    assert_eq!(
        count(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_mail.send_job"
        )
        .await,
        2
    );
    identity
        .request_password_reset("recipient@example.test")
        .await
        .unwrap();
    let reset = recovery(&owner).await;
    assert!(reset.kind == DeliveryKind::Reset);
    once(&dispatcher, true).await;
    sql(
        &owner,
        "UPDATE webdock_auth.identity_delivery_job SET delivered_at=NULL,next_attempt_at=now()",
        vec![],
    )
    .await;
    once(&dispatcher, true).await;
    assert_eq!(
        count(
            &owner,
            "SELECT count(*)::bigint AS n FROM webdock_mail.send_job"
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &owner,
            "SELECT reserved AS n FROM webdock_mail.month_usage WHERE customer_id='123'"
        )
        .await,
        3
    );
    {
        let observed = receipts.requests.lock().unwrap();
        assert_eq!(observed.len(), 5);
        assert_eq!(observed[0], observed[1]);
        assert_eq!(observed[3], observed[4]);
        assert_ne!(observed[0].0, observed[2].0);
    }

    core.assert_healthy();
    api.assert_healthy();
    drop(api);
    drop(core);
    drop(access);
    drop(identity);
    drop(store);
    auth.close().await.unwrap();
    business.close().await.unwrap();
    worker.close().await.unwrap();
    owner.close().await.unwrap();
}
