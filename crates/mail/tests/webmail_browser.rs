#![cfg(feature = "test-support")]
//! Built Console → Core HTTPS launch over restricted PostgreSQL → HTTPS Mail gateway → real
//! Community Edition JMAP engine. Everything is owned, loopback-only and removed afterwards.
#[allow(dead_code, reason = "fixture module shared by several test binaries")]
#[path = "support/engine.rs"]
mod engine;
#[allow(dead_code, reason = "fixture module shared by several test binaries")]
#[path = "support/tls.rs"]
mod tls;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::FutureExt;
use sea_orm::{
    ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap, os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration,
};
use tls::TlsServer;
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    mail::MailEndpoints,
    ui::UiAssets,
};
use webdock_identity::{CookiePolicy, IdentityService, RlsSigner, SecretKeys};
use webdock_mail::{
    MailDependencies,
    gateway::{Control, ControlFailure, Gateway, GatewayOptions},
    gateway_control::HttpControl,
    incoming::{IncomingConfig, NativeIncoming},
    jmap::{EmailQuery, JmapClient, MailDraft},
    secrets::MailCipher,
    webmail::{WebmailConfig, WebmailService},
};
const KEY: &str = "webmail-browser-owned-fixture-context-at-least32";
const GATEWAY_SECRET: &str = "webmail-browser-owned-gateway-secret-at-least32";
const SEEDED: &str = "Seeded native fixture";
const SAVED: &str = "Browser saved draft";

async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        text,
        values,
    ))
    .await
    .unwrap();
}
async fn role(key: &str, owner: &url::Url) -> DatabaseConnection {
    let mut url = url::Url::parse(&std::env::var(key).unwrap()).unwrap();
    assert_eq!(url.host_str(), owner.host_str());
    assert_eq!(url.port(), owner.port());
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    url.set_path(owner.path());
    Database::connect(url.as_str()).await.unwrap()
}
/// The gateway and Core each need the other's origin before either can start serving.
struct Deferred(std::sync::OnceLock<HttpControl>);
#[async_trait::async_trait]
impl Control for Deferred {
    async fn call(&self, input: Value) -> Result<Value, ControlFailure> {
        self.0.get().ok_or(ControlFailure)?.call(input).await
    }
    async fn send(&self, input: Value) -> Result<Value, ControlFailure> {
        self.0.get().ok_or(ControlFailure)?.send(input).await
    }
}

#[tokio::test]
#[ignore = "requires the owned local PostgreSQL fixture, Docker, the pinned engine image and a built Console"]
async fn console_opens_a_granted_mailbox_through_the_real_gateway_and_engine() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let dist = root.join("apps/console/dist");
    assert!(
        dist.join("index.html").is_file(),
        "Build the Console before acceptance"
    );
    let mut url =
        url::Url::parse(&std::env::var("WEBDOCK_TEST_MIGRATION_DATABASE_URL").unwrap()).unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/webdock_rls_test");
    assert!(url.query().is_none());
    let admin = Database::connect(url.as_str()).await.unwrap();
    let name = format!(
        "webdock_webmail_browser_{}_test",
        uuid::Uuid::new_v4().simple()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    url.set_path(&format!("/{name}"));
    let owner = Database::connect(url.as_str()).await.unwrap();
    let engine = engine::Engine::provision(&["alice"]).await;
    let result = std::panic::AssertUnwindSafe(run(&owner, &url, &name, &engine, &root, &dist))
        .catch_unwind()
        .await;
    drop(engine);
    owner.close().await.unwrap();
    admin
        .execute_unprepared(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
    admin.close().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn run(
    owner: &DatabaseConnection,
    url: &url::Url,
    name: &str,
    engine: &engine::Engine,
    root: &Path,
    dist: &Path,
) {
    let account = engine.accounts[0].clone();
    let authorization = engine::basic("alice@customer.test", engine::PASSWORD);
    let mailbox =
        JmapClient::local_fixture(&engine.origin, &authorization, &account, None).unwrap();
    let identity = mailbox.identities().await.unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let draft = |subject: &str| MailDraft {
        identity_id: identity.clone(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: subject.into(),
        text: "Seeded body for the native browser proof.".into(),
        in_reply_to: None,
        references: None,
        attachments: vec![],
    };
    mailbox.create_draft(draft(SEEDED), None).await.unwrap();

    webdock_persistence::migrations::bootstrap(owner, name, KEY)
        .await
        .unwrap();
    sql(owner,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('alice','Alice','alice@example.invalid',true,'user',false,false)",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('alice-session','alice-token','alice',now()+interval '1 hour',now())",vec![]).await;
    let seed = owner.begin().await.unwrap();
    sql(&seed,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await;
    sql(
        &seed,
        "INSERT INTO webdock_admin.customers(id,name) VALUES('123','Webmail browser fixture')",
        vec![],
    )
    .await;
    seed.commit().await.unwrap();
    sql(owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") VALUES('member-alice','124','alice','member',now())",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}')",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','test-only')",vec![]).await;
    sql(owner,"INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES('123','local-fixture','200',true,'ready',1,1)",vec![]).await;
    sql(owner,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status) VALUES('301','123','customer.test','test-only','ownership_verified')",vec![]).await;
    sql(owner,"INSERT INTO webdock_mail.domain(id,customer_id,sender_domain_id,name,provider_id,state) VALUES('302','123','301','customer.test','domain1','ready')",vec![]).await;
    sql(owner,"INSERT INTO webdock_mail.mailbox(id,customer_id,domain_id,local_part,name,quota_bytes,provider_id,state) VALUES('303','123','302','alice','Alice mailbox',10485760,$1,'ready')",vec![account.clone().into()]).await;
    let encryption = STANDARD.encode([31u8; 32]);
    let sealed = MailCipher::new(&encryption)
        .unwrap()
        .seal(
            &json!({"username":"alice@customer.test","password":engine::PASSWORD}),
            "identity:123:304",
        )
        .unwrap();
    sql(owner,"INSERT INTO webdock_mail.identity(id,customer_id,subject,provider_id,encrypted_credentials,state) VALUES('304','123','alice','principal1',$1,'ready')",vec![sealed.into()]).await;
    sql(owner,"INSERT INTO webdock_mail.mailbox_grant(customer_id,mailbox_id,subject,enabled,revision,applied_revision,member_id) VALUES('123','303','alice',true,1,1,'member-alice')",vec![]).await;

    let auth = role("AUTH_DATABASE_URL", url).await;
    let business = role("DATABASE_URL", url).await;
    let worker = role("WEBDOCK_PLUGIN_WORKER_DATABASE_URL", url).await;
    let cookies = CookiePolicy::new(true, "webmail-browser-cookie-fixture-key-atleast32").unwrap();
    let cookie = (
        cookies.name().to_owned(),
        cookies.sign("alice-token").unwrap(),
    );
    let identity = Arc::new(IdentityService::new(auth.clone(), cookies));
    let ids = Arc::new(webdock_persistence::IdAllocator::new(
        role("DATABASE_URL", url).await,
        webdock_ids::Identity::platform(0, 2).unwrap(),
    ));
    let dependencies = MailDependencies {
        auth: auth.clone(),
        business: business.clone(),
        worker,
        identity: identity.clone(),
        browser: None,
        ids: ids.clone(),
    };

    let core_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    // Machine control routes are admitted only on the separate auth origin, as in production.
    let auth_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let core_origin = TlsServer::origin(&core_listener);
    let auth_origin = TlsServer::origin(&auth_listener);
    let gateway_origin = TlsServer::origin(&gateway_listener);
    let webmail = WebmailService::new(
        dependencies.clone(),
        WebmailConfig {
            enabled: true,
            gateway_origin: gateway_origin.clone(),
            managed_cluster: "200".into(),
            rls_secret: KEY.into(),
            encryption_key: encryption,
        },
    )
    .await
    .unwrap();
    let incoming = NativeIncoming::new(
        dependencies,
        IncomingConfig {
            enabled: true,
            mailboxes_enabled: true,
            sending_enabled: false,
            managed_cluster: "200".into(),
            rls_secret: KEY.into(),
        },
    )
    .await
    .unwrap();
    let access = webdock_access::AccessService::new(
        business,
        auth.clone(),
        identity.clone(),
        ids,
        RlsSigner::new(KEY, "webdock_auth_runtime").unwrap(),
        Arc::new(
            SecretKeys::new(
                std::collections::BTreeMap::from([(1, KEY.into())]),
                1,
                Some(KEY.into()),
            )
            .unwrap(),
        ),
        webdock_access::AccessConfig {
            auth_origin: auth_origin.parse().unwrap(),
            console_origins: vec![core_origin.parse().unwrap()],
            allow_local_http: false,
        },
    )
    .unwrap();
    let config = webdock_core::config::PlatformConfig::from_map(&HashMap::from([
        ("WEBDOCK_STUDIO_ORIGIN".into(), core_origin.clone()),
        ("WEBDOCK_AUTH_ORIGIN".into(), auth_origin.clone()),
        ("WEBDOCK_FEATURES".into(), r#"{"mail":"enabled"}"#.into()),
    ]))
    .unwrap();
    let router = build_app_with_domains(
        config,
        auth,
        Services {
            identity: Some(identity),
            mail: Some(Arc::new(
                MailEndpoints::new(Arc::new(webmail), GATEWAY_SECRET)
                    .unwrap()
                    .with_incoming(Arc::new(incoming)),
            )),
            access: Some(Arc::new(access)),
            // The deployment declares the advertised gateway explicitly, like any connect origin.
            ui: Some(Arc::new(
                UiAssets::new(dist, &[], std::slice::from_ref(&gateway_origin), false).unwrap(),
            )),
            ..Default::default()
        },
    );
    let core = TlsServer::serve(core_listener, router.clone());
    let auth_server = TlsServer::serve(auth_listener, router);

    let control = Arc::new(Deferred(std::sync::OnceLock::new()));
    let mut options = GatewayOptions::new(vec![core_origin.clone()], control.clone()).unwrap();
    options.test_runtime_origin = Some(engine.origin.trim_end_matches('/').to_owned());
    let gateway = TlsServer::serve(gateway_listener, Gateway::new(options).unwrap().router());
    // Sending is disabled for this tenant; the internal send origin is never contacted.
    let ca = reqwest::Certificate::from_pem(auth_server.ca.as_bytes()).unwrap();
    assert!(
        control
            .0
            .set(
                HttpControl::new(&auth_origin, "http://127.0.0.1:9", GATEWAY_SECRET, &[ca])
                    .unwrap()
            )
            .is_ok()
    );

    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let descriptor = dir.path().join("webmail.json");
    std::fs::write(
        &descriptor,
        json!({
            "origin": core_origin, "gatewayOrigin": gateway_origin,
            "cookie": {"name": cookie.0, "value": cookie.1},
            "customerID": "123", "customerName": "Webmail browser fixture",
            "mailboxName": "Alice mailbox", "address": "alice@customer.test",
            "seededSubject": SEEDED, "savedSubject": SAVED,
        })
        .to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&descriptor, std::fs::Permissions::from_mode(0o600)).unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new("node")
            .arg(root.join("apps/console/tests/webmail-native-browser.mjs"))
            .env("WEBDOCK_WEBMAIL_BROWSER_FIXTURE", &descriptor)
            .current_dir(root)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("Webmail browser deadline")
    .expect("Webmail browser process");
    core.assert_healthy();
    auth_server.assert_healthy();
    gateway.assert_healthy();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));

    // The browser's draft exists in the real engine, and the write journal recorded exactly it.
    let listed = mailbox
        .emails(EmailQuery {
            text: Some(SAVED.into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let saved = listed["list"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|email| email["subject"] == SAVED)
        .count();
    assert_eq!(saved, 1, "exactly one saved browser draft");
    let journal = owner
        .query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT count(*) FILTER (WHERE action='draft' AND state='succeeded')::bigint AS drafts, count(*)::bigint AS writes FROM webdock_mail.webmail_write WHERE customer_id='123' AND mailbox_id='303'"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(journal.try_get::<i64>("", "drafts").unwrap(), 1);
    assert_eq!(journal.try_get::<i64>("", "writes").unwrap(), 1);
    let launches = owner
        .query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT count(*)::bigint AS n FROM webdock_mail.webmail_launch WHERE customer_id='123' AND consumed_at IS NULL"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        launches.try_get::<i64>("", "n").unwrap(),
        0,
        "every issued launch code was consumed"
    );
}
