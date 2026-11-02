#[path = "support/tls.rs"]
mod tls;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};
use webdock_identity::{Delivery, DeliveryKind, DeliveryRequest};
use webdock_mail::{
    account_delivery::{MailConfiguration, MailDelivery},
    message::Sender,
};
#[derive(Default)]
struct Mail {
    mode: AtomicU8,
    requests: Mutex<Vec<(HeaderMap, Value)>>,
}
async fn receive(
    State(mail): State<Arc<Mail>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    mail.requests.lock().unwrap().push((headers, body));
    let mode = mail.mode.load(Ordering::SeqCst);
    match mode {
        3 => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "https://outside.invalid/credential-sink")],
            Json(json!({"id":"789","state":"queued"})),
        )
            .into_response(),
        4 => (
            StatusCode::ACCEPTED,
            Json(json!({"id":"789","state":"invented"})),
        )
            .into_response(),
        5 => (
            StatusCode::ACCEPTED,
            Json(json!({"id":"000789","state":"queued"})),
        )
            .into_response(),
        6 => (
            StatusCode::ACCEPTED,
            Json(json!({"id":"789","state":"queued","private":"untrusted-extra"})),
        )
            .into_response(),
        7 => (
            StatusCode::ACCEPTED,
            Json(json!({"id":"789","state":"queued","padding":"x".repeat(5000)})),
        )
            .into_response(),
        8 => (
            StatusCode::ACCEPTED,
            Json(json!({"id":"789","state":"rejected"})),
        )
            .into_response(),
        9 => (StatusCode::OK, Json(json!({"id":"789","state":"queued"}))).into_response(),
        _ => (
            StatusCode::ACCEPTED,
            Json(json!({"id":"789","state":match mode {1=>"accepted",2=>"unknown",_=>"queued"}})),
        )
            .into_response(),
    }
}
fn private(path: &Path, value: &str) {
    std::fs::write(path, value).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn request() -> DeliveryRequest {
    DeliveryRequest {
        id: "123".into(),
        email: "recipient@example.test".into(),
        kind: DeliveryKind::Reset,
        url: format!(
            "https://auth.example.test/reset-password?token={}",
            "a".repeat(64)
        ),
    }
}
fn config(origin: &str, ca: &Path) -> MailConfiguration {
    MailConfiguration {
        origin: origin.into(),
        authority: Some(ca.into()),
        from: Sender {
            email: "sender@example.test".into(),
            name: Some("Webdock".into()),
        },
        api_key: format!("wd_mail_{}", "k".repeat(43)),
    }
}
#[tokio::test]
async fn durable_receipts_are_typed_and_replays_keep_exact_payload_and_idempotency() {
    let mail = Arc::new(Mail::default());
    let state = mail.clone();
    let server = tls::TlsServer::bind(move |_| {
        Router::new()
            .route("/v1/emails", post(receive))
            .with_state(state)
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    private(&ca, &server.ca);
    let sender =
        MailDelivery::new(config(&server.origin, &ca), "https://auth.example.test").unwrap();
    let request = request();
    for mode in 0..=2 {
        mail.mode.store(mode, Ordering::SeqCst);
        sender.deliver(&request).await.unwrap();
    }
    let requests = mail.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (headers, body) in requests.iter() {
        assert_eq!(
            headers["authorization"],
            format!("Bearer wd_mail_{}", "k".repeat(43))
        );
        assert_eq!(headers["idempotency-key"], "account:123");
        assert_eq!(*body, requests[0].1);
        assert_eq!(body["from"], "Webdock <sender@example.test>");
        assert_eq!(body["subject"], "Reset your Spitzli Cloud password");
        assert_eq!(
            body["text"],
            format!(
                "Reset your Spitzli Cloud password using this link:\n{}\nIf you did not request this, you can ignore this message.",
                request.url
            )
        );
    }
    drop(requests);
    server.assert_healthy();
}
#[tokio::test]
async fn untrusted_ca_redirect_and_malformed_ack_fail_without_retry_or_secret_in_error() {
    let mail = Arc::new(Mail::default());
    let state = mail.clone();
    let server = tls::TlsServer::bind(move |_| {
        Router::new()
            .route("/v1/emails", post(receive))
            .with_state(state)
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    private(&ca, &server.ca);
    let wrong = dir.path().join("wrong.pem");
    private(
        &wrong,
        &rcgen::generate_simple_self_signed(vec!["localhost".into()])
            .unwrap()
            .cert
            .pem(),
    );
    let sender =
        MailDelivery::new(config(&server.origin, &wrong), "https://auth.example.test").unwrap();
    assert!(sender.deliver(&request()).await.is_err());
    assert!(mail.requests.lock().unwrap().is_empty());
    let sender =
        MailDelivery::new(config(&server.origin, &ca), "https://auth.example.test").unwrap();
    for mode in 3..=9 {
        mail.mode.store(mode, Ordering::SeqCst);
        let error = sender.deliver(&request()).await.unwrap_err();
        assert_eq!(error.to_string(), "account delivery is unavailable");
    }
    assert_eq!(mail.requests.lock().unwrap().len(), 7);
    server.assert_healthy();
}
#[tokio::test]
async fn only_canonical_kind_bound_account_links_can_enter_mail() {
    let mail = Arc::new(Mail::default());
    let state = mail.clone();
    let server = tls::TlsServer::bind(move |_| {
        Router::new()
            .route("/v1/emails", post(receive))
            .with_state(state)
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("ca.pem");
    private(&ca, &server.ca);
    let sender =
        MailDelivery::new(config(&server.origin, &ca), "https://auth.example.test").unwrap();
    for link in [
        "https://outside.invalid/reset-password?token=abc",
        "https://auth.example.test/sites",
        "https://auth.example.test/reset-password?token=abc",
        "https://auth.example.test/reset-password?token=abc&token=def",
        "https://auth.example.test/reset-password?token=abc#secret",
    ] {
        let mut input = request();
        input.url = link.into();
        assert!(sender.deliver(&input).await.is_err());
    }
    assert!(mail.requests.lock().unwrap().is_empty());
    let mut activation = request();
    activation.kind = DeliveryKind::Activation;
    sender.deliver(&activation).await.unwrap();
    let mut invitation = request();
    invitation.kind = DeliveryKind::Invitation;
    invitation.id = "124".into();
    invitation.url = "https://auth.example.test/invitation?id=456".into();
    sender.deliver(&invitation).await.unwrap();
    let mut website = request();
    website.kind = DeliveryKind::WebsiteAccess;
    website.id = "125".into();
    website.url = "https://auth.example.test/sites".into();
    sender.deliver(&website).await.unwrap();
    {
        let requests = mail.requests.lock().unwrap();
        assert_eq!(
            requests[0].1["subject"],
            "Set up your Spitzli Cloud account"
        );
        assert_eq!(requests[1].1["subject"], "Invitation to Spitzli Cloud");
        assert_eq!(
            requests[2].1["subject"],
            "Your Spitzli Cloud website access"
        );
    }
    for (index, origin) in [
        "http://localhost:3125",
        "http://127.0.0.1:3125",
        "http://[::1]:3125",
    ]
    .into_iter()
    .enumerate()
    {
        let sender = MailDelivery::new(config(&server.origin, &ca), origin).unwrap();
        let mut local = request();
        local.id = (126 + index).to_string();
        local.url = format!("{origin}/reset-password?token={}", "a".repeat(64));
        sender.deliver(&local).await.unwrap();
    }
    assert!(
        MailDelivery::new(
            config("http://localhost:3139", &ca),
            "https://auth.example.test"
        )
        .is_err()
    );
    assert!(MailDelivery::new(config(&server.origin, &ca), "http://outside.invalid").is_err());
    server.assert_healthy();
}
/// Minimal in-process SMTP catch-all: accepts one message and returns its DATA section.
async fn catch_one(listener: tokio::net::TcpListener) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    write.write_all(b"220 fixture\r\n").await.unwrap();
    let mut data = String::new();
    let mut in_data = false;
    while let Some(line) = lines.next_line().await.unwrap() {
        if in_data {
            if line == "." {
                in_data = false;
                write.write_all(b"250 queued\r\n").await.unwrap();
            } else {
                data.push_str(&line);
                data.push('\n');
            }
            continue;
        }
        let reply: &[u8] = match line.split(' ').next().unwrap_or("").to_uppercase().as_str() {
            "EHLO" => b"250 fixture\r\n",
            "DATA" => {
                in_data = true;
                b"354 go\r\n"
            }
            "QUIT" => {
                write.write_all(b"221 bye\r\n").await.unwrap();
                break;
            }
            _ => b"250 ok\r\n",
        };
        write.write_all(reply).await.unwrap();
    }
    data
}
#[tokio::test]
async fn loopback_smtp_carries_the_activation_link_with_its_invitation() {
    assert!(
        MailDelivery::loopback(
            "192.0.2.1:25".parse().unwrap(),
            "webdock@example.test",
            "http://localhost:3125"
        )
        .is_err(),
        "only a loopback catch-all is accepted"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let caught = tokio::spawn(catch_one(listener));
    let sender =
        MailDelivery::loopback(address, "webdock@example.test", "http://localhost:3125").unwrap();
    let mut activation = request();
    activation.kind = DeliveryKind::Activation;
    activation.url = format!(
        "http://localhost:3125/reset-password?token={}&invitation=456",
        "a".repeat(64)
    );
    sender.deliver(&activation).await.unwrap();
    let message = caught.await.unwrap();
    assert!(message.contains("To: <recipient@example.test>"));
    assert!(message.contains("Subject: Set up your Spitzli Cloud account"));
    assert!(message.contains(&activation.url));
    // Only activations may continue into an invitation; resets stay single-purpose.
    let mut reset = activation.clone();
    reset.kind = DeliveryKind::Reset;
    assert!(sender.deliver(&reset).await.is_err());
}
