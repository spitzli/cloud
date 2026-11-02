use serde_json::json;
use std::{collections::BTreeMap, net::IpAddr, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webdock_plugin_host::http::{
    HttpBinding, HttpRule, HttpTransport, PinnedTransport, bind_request,
};
async fn endpoint(
    status: u16,
    declared_size: Option<usize>,
) -> (HttpBinding, tokio::task::JoinHandle<()>) {
    let generated =
        rcgen::generate_simple_self_signed(vec!["provider.invalid".to_owned()]).unwrap();
    let tls = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![generated.cert.der().clone()],
        tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(
            generated.signing_key.serialize_der(),
        )
        .into(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let Ok(mut stream) = acceptor.accept(stream).await else {
            return;
        };
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).await.unwrap() > 0);
        let body = "{\"pinned\":true}";
        let _=stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nLocation: https://never-follow.invalid/private\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",declared_size.unwrap_or(body.len())).as_bytes()).await;
    });
    (
        HttpBinding {
            origin: format!("https://provider.invalid:{port}"),
            addresses: vec!["127.0.0.1".parse::<IpAddr>().unwrap()],
            headers: BTreeMap::new(),
            rules: vec![HttpRule {
                method: "GET".into(),
                path: "/v1/items".into(),
                query: vec![],
            }],
            ca: Some(generated.cert.pem()),
            website_binding_id: None,
        },
        task,
    )
}
#[tokio::test]
async fn pins_addresses_validates_tls_does_not_follow_redirects_and_bounds_response() {
    let input = json!({"binding":"provider","method":"GET","path":"/v1/items"});
    let (binding, task) = endpoint(200, None).await;
    let response = PinnedTransport
        .send(bind_request(&input, binding).unwrap())
        .await
        .unwrap();
    assert_eq!(response.body, json!({"pinned":true}));
    task.await.unwrap();
    let (binding, task) = endpoint(302, None).await;
    assert_eq!(
        PinnedTransport
            .send(bind_request(&input, binding).unwrap())
            .await
            .unwrap()
            .status,
        302
    );
    task.await.unwrap();
    let (binding, task) = endpoint(200, Some(1024 * 1024 + 1)).await;
    assert!(
        PinnedTransport
            .send(bind_request(&input, binding).unwrap())
            .await
            .is_err()
    );
    task.await.unwrap();
    let (mut binding, task) = endpoint(200, None).await;
    binding.ca = None;
    assert!(
        PinnedTransport
            .send(bind_request(&input, binding).unwrap())
            .await
            .is_err()
    );
    task.await.unwrap();
}
