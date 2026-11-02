//! Local HTTP provider stand-in only; no provider resource, credential or real mail.
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use webdock_mail::transport::{Platform, Route, Sender};
use webdock_mail_runtime::{
    message::prepare_action_message,
    outbox::{PayloadFormat, SendLease},
};
#[tokio::test]
async fn one_http_attempt_preserves_rejection_and_unknown_classifier() {
    let input = json!({"resource_id":"resource","from":"hello","to":"to@example.test","subject":"owned","text":"body"});
    let compiled = webdock_mail_message::compile_action(input, "example.test").unwrap();
    let message = compiled.request.compile_action().unwrap();
    let prepared = prepare_action_message(&message.raw, &message.sender, &message.to).unwrap();
    let job = SendLease {
        id: "9911".into(),
        customer_id: "91".into(),
        source_id: format!("action:{}", "a".repeat(64)),
        payload_ref: uuid::Uuid::new_v4().to_string(),
        payload_bytes: compiled.payload.len() as i32,
        request_hash: webdock_mail_protocol::sha256(&compiled.payload),
        payload_format: PayloadFormat::Request,
        source_context: json!({}),
        route: json!({}),
        envelope: compiled.envelope,
        lease_token: uuid::Uuid::new_v4().to_string(),
    };
    for (status, body, expected) in [
        (200, r#"{"mid":"98765432101234567890"}"#, "accepted"),
        (429, "{}", "rejected"),
        (401, "{}", "rejected"),
        (500, "{}", "unknown"),
        (200, "{broken", "unknown"),
        (200, r#"{"mid":""}"#, "unknown"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&bytes);
                let Some((head, data)) = text.split_once("\r\n\r\n") else {
                    continue;
                };
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|s| s.parse::<usize>().unwrap())
                    })
                    .unwrap();
                if data.len() >= len {
                    break;
                }
            }
            assert!(bytes.starts_with(b"POST /api/v2/mail/send "));
            socket.write_all(format!("HTTP/1.1 {status} Fixture\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let platform = Platform::TurboSmtp {
            consumer_key: "fixture-key".into(),
            consumer_secret: "fixture-secret".into(),
        };
        let receipt = Sender::new(&origin)
            .submit_prepared(Route::Platform(&platform), &job, &message, &prepared)
            .await;
        assert_eq!(receipt.state(), expected);
        server.await.unwrap();
    }
}
