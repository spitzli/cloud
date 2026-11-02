use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webdock_mail::send_control::{HttpSendControl, SendControl};
#[tokio::test]
async fn confidential_control_uses_tls_fixed_path_no_retry_and_redacted_errors() {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let ca = reqwest::Certificate::from_der(cert.cert.der()).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!(
        "https://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let task = tokio::spawn(async move {
        for status in [200, 403, 302] {
            let (socket, _) = listener.accept().await.unwrap();
            let mut stream = tokio_rustls::TlsAcceptor::from(Arc::new(config.clone()))
                .accept(socket)
                .await
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 2048];
            loop {
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                assert!(bytes.len() < 65536);
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with("POST /api/mail/gateway HTTP/1.1\r\n"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer rust-fixture-worker-secret-at-least32")
            );
            let body = if status == 200 {
                r#"{"data":{"ok":true}}"#
            } else {
                r#"{"error":"PRIVATE backend detail"}"#
            };
            let response = format!(
                "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nLocation: https://outside.example.invalid/\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    assert!(
        HttpSendControl::new(
            "http://localhost/",
            "rust-fixture-worker-secret-at-least32",
            &[]
        )
        .is_err()
    );
    let client =
        HttpSendControl::new(&origin, "rust-fixture-worker-secret-at-least32", &[ca]).unwrap();
    assert_eq!(
        client
            .call(serde_json::json!({"action":"send.claim"}))
            .await
            .unwrap(),
        serde_json::json!({"ok":true})
    );
    let denied = client
        .call(serde_json::json!({"action":"send.claim"}))
        .await
        .unwrap_err();
    assert_eq!(denied.status, 403);
    assert!(!denied.to_string().contains("PRIVATE"));
    assert_eq!(
        client
            .call(serde_json::json!({"action":"send.claim"}))
            .await
            .unwrap_err()
            .status,
        503
    );
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}
