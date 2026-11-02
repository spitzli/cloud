use std::{collections::BTreeMap, net::IpAddr, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webdock_plugin_http::{
    HttpBinding, HttpRule, MAIL_REQUEST_BYTES, bind_target, send_json, send_mail,
};
fn binding(port: u16, ca: String) -> HttpBinding {
    HttpBinding {
        origin: format!("https://provider.invalid:{port}"),
        addresses: vec!["127.0.0.1".parse::<IpAddr>().unwrap()],
        headers: BTreeMap::from([("authorization".into(), "Bearer fixture-tenant-only".into())]),
        rules: vec![HttpRule {
            method: "POST".into(),
            path: "/send".into(),
            query: vec!["mode".into()],
        }],
        ca: Some(ca),
        website_binding_id: None,
    }
}
#[test]
fn target_cannot_escape_exact_named_binding() {
    let b = binding(443, String::new());
    for path in [
        "//evil.invalid/send",
        "https://evil.invalid/send",
        "/send/../send",
        "/%73end",
        "/send?other=1",
        "/send?mode=a&mode=b",
    ] {
        assert!(bind_target("POST", path, b.clone()).is_err(), "{path}");
    }
    assert!(bind_target("GET", "/send", b.clone()).is_err());
    assert!(bind_target("POST", "/send?mode=a", b).is_ok());
}
async fn serve_once(response: &'static [u8]) -> (HttpBinding, tokio::task::JoinHandle<Vec<u8>>) {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["provider.invalid".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let tls = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let b = binding(listener.local_addr().unwrap().port(), cert.pem());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio_rustls::TlsAcceptor::from(Arc::new(tls))
            .accept(stream)
            .await
            .unwrap();
        let mut input = vec![];
        let mut buffer = [0; 8192];
        loop {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            input.extend_from_slice(&buffer[..n]);
            if let Some(end) = input.windows(4).position(|x| x == b"\r\n\r\n") {
                let header = std::str::from_utf8(&input[..end]).unwrap();
                let size = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(|n| n.parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if input.len() >= end + 4 + size {
                    break;
                }
            }
        }
        stream.write_all(response).await.unwrap();
        stream.shutdown().await.unwrap();
        input
    });
    (b, server)
}
#[tokio::test]
async fn bulk_lane_is_pinned_authenticated_bounded_and_never_redirects() {
    let (b, server) =
        serve_once(b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await;
    let response = send_mail(
        bind_target("POST", "/send?mode=raw", b).unwrap(),
        "message/rfc822",
        b"From: fixture@example.invalid\r\n\r\nhello".to_vec(),
    )
    .await
    .unwrap();
    assert_eq!(response.status, 202);
    let request = server.await.unwrap();
    assert!(
        std::str::from_utf8(&request)
            .unwrap()
            .contains("authorization: Bearer fixture-tenant-only")
    );
    let (b,server)=serve_once(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: https://other.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    assert_eq!(
        send_mail(
            bind_target("POST", "/send", b).unwrap(),
            "application/json",
            b"{}".to_vec()
        )
        .await
        .unwrap()
        .status,
        307
    );
    server.await.unwrap();
    let (b, server) =
        serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 65537\r\nConnection: close\r\n\r\n").await;
    assert!(
        send_mail(
            bind_target("POST", "/send", b).unwrap(),
            "application/json",
            b"{}".to_vec()
        )
        .await
        .is_err()
    );
    server.await.unwrap();
    assert!(
        send_mail(
            bind_target("POST", "/send", binding(1, String::new())).unwrap(),
            "application/json",
            vec![0; MAIL_REQUEST_BYTES + 1]
        )
        .await
        .is_err()
    );
    assert!(
        send_json(
            bind_target("POST", "/send", binding(1, String::new())).unwrap(),
            Some(vec![0; 1024 * 1024 + 1])
        )
        .await
        .is_err()
    );
}
