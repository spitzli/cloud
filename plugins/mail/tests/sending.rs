//! Real plugin storage on a disposable local PostgreSQL (`WEBDOCK_MAIL_PLUGIN_TEST_ADMIN_URL`, loopback),
//! an in-test SMTP catcher and an in-test turboSMTP stand-in. No message leaves this process.
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_postgres::NoTls;
use webdock_mail::{Dns, Handler, Plugin, Settings, seal::Sealer, store::Store, transport};
use webdock_plugin_protocol::{ErrorCode, InvocationClaims, Manifest, PluginError};
use webdock_plugin_runtime::ActionHandler;

fn admin_url() -> url::Url {
    let url = url::Url::parse(
        &std::env::var("WEBDOCK_MAIL_PLUGIN_TEST_ADMIN_URL").expect("disposable local server"),
    )
    .unwrap();
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    url
}
async fn admin() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(admin_url().as_str(), NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    client
}

type Seen<T> = Arc<Mutex<Vec<T>>>;
/// Minimal SMTP: records the DATA of every message.
async fn smtp_session(socket: tokio::net::TcpStream, store: Seen<String>) {
    let (read, mut write) = socket.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"220 catcher\r\n").await.unwrap();
    let mut data = None::<String>;
    while let Ok(Some(line)) = lines.next_line().await {
        let reply: &[u8] = match (data.as_mut(), line.as_str()) {
            (Some(_), ".") => {
                store.lock().unwrap().push(data.take().unwrap());
                b"250 queued\r\n"
            }
            (Some(body), _) => {
                body.push_str(&line);
                body.push('\n');
                continue;
            }
            (None, _) => match line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase()
                .as_str()
            {
                "EHLO" | "HELO" => b"250 catcher\r\n",
                "DATA" => {
                    data = Some(String::new());
                    b"354 go\r\n"
                }
                "QUIT" => {
                    write.write_all(b"221 bye\r\n").await.unwrap();
                    return;
                }
                _ => b"250 ok\r\n",
            },
        };
        write.write_all(reply).await.unwrap();
    }
}
async fn smtp_catcher() -> (u16, Seen<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::spawn(smtp_session(socket, Arc::clone(&store)));
        }
    });
    (port, seen)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, String) {
    let mut buffer = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let n = socket.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buffer).to_string();
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let length = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        if body.len() >= length {
            return (head.to_owned(), body[..length].to_owned());
        }
    }
}
/// Answers POST /api/v2/mail/send: 200 with a message id for the right keys, 401 otherwise.
async fn turbo_session(mut socket: tokio::net::TcpStream, store: Seen<(String, Value)>) {
    let (head, body) = read_request(&mut socket).await;
    let key = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("consumerkey: ")
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let (status, reply) = if key.trim() == "good-key" && head.starts_with("POST /api/v2/mail/send ")
    {
        ("200 OK", r#"{"message":"OK","mid":"98765432101234567890"}"#)
    } else {
        ("401 Unauthorized", r#"{"message":"unauthorized"}"#)
    };
    store
        .lock()
        .unwrap()
        .push((head, serde_json::from_str(&body).unwrap_or(Value::Null)));
    socket.write_all(format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}", reply.len()).as_bytes()).await.unwrap();
}
async fn turbo_stand_in() -> (String, Seen<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::spawn(turbo_session(socket, Arc::clone(&store)));
        }
    });
    (origin, seen)
}

struct FakeDns(Arc<Mutex<Vec<(String, String)>>>);
#[tonic::async_trait]
impl Dns for FakeDns {
    async fn txt(&self, name: &str) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
    }
}

struct Fixture {
    handler: Handler,
    storage: String,
    dns: Arc<Mutex<Vec<(String, String)>>>,
    mail: Arc<Mutex<Vec<String>>>,
    turbo: Arc<Mutex<Vec<(String, Value)>>>,
}
async fn fixture(trust: bool) -> Fixture {
    let storage = format!("mail_test_{}", uuid::Uuid::new_v4().simple());
    admin()
        .await
        .batch_execute(&format!("CREATE DATABASE {storage}"))
        .await
        .unwrap();
    let mut url = admin_url();
    url.set_path(&storage);
    let (port, mail) = smtp_catcher().await;
    let (origin, turbo) = turbo_stand_in().await;
    let dns = Arc::new(Mutex::new(Vec::new()));
    let plugin = Plugin::new(
        Store::connect(url.as_str()).await.unwrap(),
        Sealer::new(&"24".repeat(32)).unwrap(),
        transport::Sender::new(&origin),
        Box::new(FakeDns(Arc::clone(&dns))),
        Settings {
            platform: transport::Platform::Smtp {
                host: "127.0.0.1".into(),
                port,
            },
            trust_unverified_domains: trust,
        },
        None,
    );
    Fixture {
        handler: Handler(plugin),
        storage,
        dns,
        mail,
        turbo,
    }
}
impl Fixture {
    async fn call(&self, project: &str, action: &str, input: Value) -> Result<Value, PluginError> {
        let claims = InvocationClaims {
            job: None,
            v: 1,
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: "mail".into(),
            generation: 1,
            action: action.into(),
            project_id: project.into(),
            customer_id: if project == "202" { "202" } else { "101" }.into(),
            iat: 0,
            exp: 0,
        };
        self.handler.invoke(&claims, "", input, json!({})).await
    }
    async fn ok(&self, project: &str, action: &str, input: Value) -> Value {
        self.call(project, action, input)
            .await
            .unwrap_or_else(|e| panic!("{action}: {e:?}"))
    }
    async fn code(&self, project: &str, action: &str, input: Value) -> ErrorCode {
        self.call(project, action, input).await.unwrap_err().code
    }
    async fn cleanup(self) {
        let mut url = admin_url();
        url.set_path(&self.storage);
        let store = Store::connect(url.as_str()).await.unwrap();
        assert!(
            store.engines().await.unwrap().is_empty(),
            "outgoing-only domains, relay and sends must not reserve incoming engines"
        );
        drop(store);
        drop(self.handler);
        admin()
            .await
            .batch_execute(&format!("DROP DATABASE {} WITH (FORCE)", self.storage))
            .await
            .unwrap();
    }
}
fn id(created: &Value) -> String {
    created["resource"]["id"].as_str().unwrap().to_owned()
}
fn send(domain: &str, to: &str) -> Value {
    json!({"resource_id": domain, "from": "hello", "from_name": "Shop", "to": to, "subject": "Order shipped", "text": "Your order is on its way."})
}

#[test]
fn shipped_manifest_is_the_reviewed_contract() {
    webdock_mail::validate_manifest(&Manifest::parse(include_bytes!("../manifest.json")).unwrap())
        .unwrap();
}

#[test]
fn domain_names_are_normalised_and_bounded() {
    assert_eq!(
        webdock_mail::domain_name(" Shop.Example.COM. ").unwrap(),
        "shop.example.com"
    );
    for bad in [
        "localhost",
        "-a.com",
        "a..com",
        "127.0.0.1",
        "a_b.com",
        "ex ample.com",
        &format!("{}.com", "a".repeat(64)),
    ] {
        assert!(webdock_mail::domain_name(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn domain_verification_and_unconfigured_delivery_denial() {
    let f = fixture(false).await;
    let created = f
        .ok(
            "101",
            "domains.create",
            json!({"domain":"Shop.Example.com"}),
        )
        .await;
    assert_eq!(created["resource"]["name"], "shop.example.com");
    let domain = id(&created);
    // An unverified claim blocks nobody: another project may add the same domain, and only
    // one project can ever prove it.
    let rival = id(&f
        .ok(
            "202",
            "domains.create",
            json!({"domain":"shop.example.com"}),
        )
        .await);
    assert_eq!(
        f.code(
            "101",
            "domains.create",
            json!({"domain":"shop.example.com"})
        )
        .await,
        ErrorCode::Conflict,
        "once per project"
    );
    assert_eq!(
        f.code("202", "domains.view", json!({"resource_id": domain}))
            .await,
        ErrorCode::NotFound
    );

    let view = f
        .ok("101", "domains.view", json!({"resource_id": domain}))
        .await;
    assert_eq!(view["verified"], false);
    let record = view["records"][0].clone();
    assert_eq!(record["name"], "_webdock.shop.example.com");
    assert_eq!(
        f.code("101", "mail.send", send(&domain, "a@example.net"))
            .await,
        ErrorCode::CapabilityUnavailable,
        "unconfigured delivery never sends inline"
    );

    f.dns.lock().unwrap().push((
        "_webdock.shop.example.com".into(),
        "webdock-verify=wrong".into(),
    ));
    assert_eq!(
        f.ok("101", "domains.verify", json!({"resource_id": domain}))
            .await["verified"],
        false
    );
    f.dns.lock().unwrap().push((
        "_webdock.shop.example.com".into(),
        record["value"].as_str().unwrap().into(),
    ));
    assert_eq!(
        f.ok("101", "domains.verify", json!({"resource_id": domain}))
            .await["verified"],
        true
    );
    // The rival project's own token is different; even with the right record it cannot verify twice.
    let rival_token = f
        .ok("202", "domains.view", json!({"resource_id": rival}))
        .await["records"][0]["value"]
        .as_str()
        .unwrap()
        .to_owned();
    f.dns
        .lock()
        .unwrap()
        .push(("_webdock.shop.example.com".into(), rival_token));
    assert_eq!(
        f.code("202", "domains.verify", json!({"resource_id": rival}))
            .await,
        ErrorCode::Conflict,
        "first DNS proof wins"
    );

    assert_eq!(
        f.code(
            "101",
            "mail.send",
            send(&domain, "a@example.net, b@example.org")
        )
        .await,
        ErrorCode::CapabilityUnavailable
    );
    assert!(f.mail.lock().unwrap().is_empty());
    assert!(f.turbo.lock().unwrap().is_empty());
    assert!(
        f.ok("101", "domains.view", json!({"resource_id":domain}))
            .await["recent"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.ok("101", "domains.delete", json!({"resource_id": domain}))
        .await;
    assert_eq!(
        f.code("101", "domains.view", json!({"resource_id": domain}))
            .await,
        ErrorCode::NotFound
    );
    // Once the owner removes it, the other project can verify it.
    assert_eq!(
        f.ok("202", "domains.verify", json!({"resource_id": rival}))
            .await["verified"],
        true
    );
    // Deletes are idempotent: the Core already proved ownership.
    f.ok("101", "domains.delete", json!({"resource_id": domain}))
        .await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires a disposable local PostgreSQL server"]
async fn own_turbosmtp_metadata_never_enables_inline_delivery() {
    let f = fixture(true).await;
    let domain = id(&f
        .ok(
            "101",
            "domains.create",
            json!({"domain":"mail.example.com"}),
        )
        .await);
    let relay = id(&f
        .ok(
            "101",
            "relay.connect",
            json!({"consumer_key":"good-key","consumer_secret":"s3cret-value"}),
        )
        .await);
    assert_eq!(
        f.code(
            "101",
            "relay.connect",
            json!({"consumer_key":"x","consumer_secret":"y"})
        )
        .await,
        ErrorCode::Conflict,
        "one account per project"
    );
    let view = f
        .ok("101", "relay.view", json!({"resource_id": relay}))
        .await;
    assert_eq!(
        view,
        json!({"provider":"turboSMTP","region":"EU","keyHint":"…-key","connectedAt":view["connectedAt"]})
    );
    assert!(
        !view.to_string().contains("s3cret"),
        "the secret never comes back"
    );
    assert_eq!(
        f.code("202", "relay.view", json!({"resource_id": relay}))
            .await,
        ErrorCode::NotFound
    );

    assert_eq!(
        f.code("101", "mail.send", send(&domain, "a@example.net"))
            .await,
        ErrorCode::CapabilityUnavailable
    );
    assert!(f.mail.lock().unwrap().is_empty());
    assert!(f.turbo.lock().unwrap().is_empty());
    let view = f
        .ok("101", "domains.view", json!({"resource_id":domain}))
        .await;
    assert!(view["recent"].as_array().unwrap().is_empty());
    assert_eq!(view["transport"], "turbosmtp");
    f.cleanup().await;
}
