use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub struct Fixture {
    pub origin: String,
    ca: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    pub async fn start(
        handler: impl Fn(String, Value) -> (u16, String) + Send + Sync + 'static,
    ) -> Self {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let ca = cert.cert.pem();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
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
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let mut stream = acceptor.accept(socket).await.unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 8192];
                let end = loop {
                    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                        break end + 4;
                    }
                    assert!(bytes.len() < 65536);
                };
                let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                assert!(headers.starts_with("POST /api/v2/mail/send HTTP/1.1\r\n"));
                let lower = headers.to_ascii_lowercase();
                let length: usize = lower
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert!(length < 24_000_000);
                while bytes.len() - end < length {
                    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let body: Value = serde_json::from_slice(&bytes[end..end + length]).unwrap();
                let (status, body) = handler(lower, body);
                let reply = format!(
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nLocation: https://outside.invalid/\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
                let _ = stream.shutdown().await;
            }
        });
        Self { origin, ca, task }
    }
    pub fn configuration(&self, key: &str, secret: &str) -> Value {
        let binding = webdock_plugin_http::HttpBinding {
            origin: self.origin.clone(),
            addresses: vec!["127.0.0.1".parse().unwrap()],
            headers: BTreeMap::from([
                ("consumerKey".into(), key.into()),
                ("consumerSecret".into(), secret.into()),
            ]),
            rules: vec![webdock_plugin_http::HttpRule {
                method: "POST".into(),
                path: "/api/v2/mail/send".into(),
                query: vec![],
            }],
            ca: Some(self.ca.clone()),
            website_binding_id: None,
        };
        serde_json::json!({"binding":binding})
    }
}
