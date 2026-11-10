#[path = "support/native_pg.rs"]
mod fixture;
use spitzli_persistence::postgres::{Limits, Pool};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "owned local PostgreSQL18 with an owned TLS proxy"]
async fn native_tls_checks_actual_hostname_and_never_falls_back_to_plaintext() {
    let f = fixture::Fixture::start().await;
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let root = tempfile::tempdir().unwrap();
    let ca = root.path().join("ca.pem");
    std::fs::write(&ca, certificate.cert.pem()).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der());
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![certificate.cert.der().clone()], key.into())
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let backend = url::Url::parse(&f.runtime_url).unwrap();
    let backend_port = backend.port().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let mut request = [0u8; 8];
                if stream.read_exact(&mut request).await.is_err() {
                    return;
                }
                assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                stream.write_all(b"S").await.unwrap();
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut upstream = tokio::net::TcpStream::connect(("127.0.0.1", backend_port))
                    .await
                    .unwrap();
                let _ = tokio::io::copy_bidirectional(&mut tls, &mut upstream).await;
            });
        }
    });
    let mut url = url::Url::parse(&f.runtime_url).unwrap();
    url.set_host(Some("localhost")).unwrap();
    url.set_port(Some(port)).unwrap();
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("sslmode", "verify-full")
        .append_pair("sslrootcert", ca.to_str().unwrap());
    let pool = Pool::connect(
        url.as_str(),
        "webdock_auth_runtime",
        true,
        Limits::default(),
    )
    .await
    .unwrap();
    let mut c = pool.acquire().await.unwrap();
    let tx = c.transaction().await.unwrap();
    assert_eq!(
        tx.query_one("SELECT current_user::text", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "webdock_auth_runtime"
    );
    tx.rollback().await.unwrap();
    url.set_host(Some("127.0.0.1")).unwrap();
    assert!(
        Pool::connect(
            url.as_str(),
            "webdock_auth_runtime",
            true,
            Limits::default()
        )
        .await
        .is_err(),
        "a trusted certificate for another hostname must fail"
    );
    let rejecting = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rejecting_port = rejecting.local_addr().unwrap().port();
    let downgrade = tokio::spawn(async move {
        let (mut s, _) = rejecting.accept().await.unwrap();
        let mut b = [0u8; 8];
        s.read_exact(&mut b).await.unwrap();
        assert_eq!(b, [0, 0, 0, 8, 4, 210, 22, 47]);
        s.write_all(b"N").await.unwrap();
        let mut data = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut data))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0, "no startup/password bytes may follow a TLS refusal");
    });
    url.set_port(Some(rejecting_port)).unwrap();
    assert!(
        Pool::connect(
            url.as_str(),
            "webdock_auth_runtime",
            true,
            Limits::default()
        )
        .await
        .is_err()
    );
    downgrade.await.unwrap();
    task.abort();
}
