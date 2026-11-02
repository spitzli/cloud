#![cfg(feature = "test-support")]
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tokio_rustls::{TlsAcceptor, rustls};
use webdock_mail_runtime::smtp::{SmtpOptions, submit};
#[derive(Clone, Copy)]
enum Mode {
    Accept,
    Drop,
    AuthDenied,
    Flood,
    BeforeDataRejected,
    AfterDataRejected,
}
struct Fixture {
    options: SmtpOptions,
    auth: Arc<AtomicUsize>,
    accepted: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<Vec<String>>,
}
async fn fixture(mode: Mode) -> Fixture {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let options = SmtpOptions::local_fixture(
        listener.local_addr().unwrap().port(),
        hex::encode(Sha256::digest(cert.cert.der())),
        "fixture".into(),
        "fixture-password".into(),
        Duration::from_secs(2),
    )
    .unwrap();
    let auth = Arc::new(AtomicUsize::new(0));
    let accepted = Arc::new(AtomicUsize::new(0));
    let a = auth.clone();
    let s = accepted.clone();
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let Ok(socket) = TlsAcceptor::from(Arc::new(config)).accept(socket).await else {
            return vec![];
        };
        let (read, mut write) = tokio::io::split(socket);
        let mut lines = BufReader::new(read).lines();
        let mut data = false;
        let mut observed = Vec::new();
        if write.write_all(b"220 fixture ESMTP\r\n").await.is_err() {
            return observed;
        }
        while let Ok(Some(line)) = lines.next_line().await {
            if data {
                if line == "." {
                    data = false;
                    if matches!(mode, Mode::Drop) {
                        break;
                    }
                    if matches!(mode, Mode::AfterDataRejected) {
                        let _ = write.write_all(b"450 Rejected\r\n").await;
                        break;
                    }
                    s.fetch_add(1, Ordering::SeqCst);
                    if write.write_all(b"250 Accepted\r\n").await.is_err() {
                        break;
                    }
                } else {
                    observed.push(line);
                }
                continue;
            }
            let reply = if line.starts_with("EHLO") {
                if matches!(mode, Mode::Flood) {
                    let _ = write
                        .write_all(format!("250-{}\r\n", "x".repeat(50_000)).as_bytes())
                        .await;
                    break;
                }
                "250-fixture\r\n250-AUTH PLAIN\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n"
            } else if line.starts_with("AUTH") {
                a.fetch_add(1, Ordering::SeqCst);
                if matches!(mode, Mode::AuthDenied) {
                    "535 Denied\r\n"
                } else {
                    "235 OK\r\n"
                }
            } else if line.starts_with("RCPT") && matches!(mode, Mode::BeforeDataRejected) {
                "450 Rejected\r\n"
            } else if line == "DATA" {
                data = true;
                "354 Continue\r\n"
            } else {
                "250 OK\r\n"
            };
            if write.write_all(reply.as_bytes()).await.is_err() {
                break;
            }
        }
        observed
    });
    Fixture {
        options,
        auth,
        accepted,
        task,
    }
}
const RAW:&[u8]=b"From: sender@example.test\r\nTo: recipient@example.test\r\nSubject: Fixture\r\n\r\n.line starting with a dot\r\n";
#[tokio::test]
async fn private_smtp_pin_before_auth_and_definitive_acceptance() {
    let f = fixture(Mode::Accept).await;
    submit(
        &f.options,
        "sender@example.test",
        &["recipient@example.test".into()],
        RAW,
    )
    .await
    .unwrap();
    assert_eq!(f.auth.load(Ordering::SeqCst), 1);
    assert_eq!(f.accepted.load(Ordering::SeqCst), 1);
    let lines = f.task.await.unwrap();
    assert!(lines.iter().any(|l| l == "..line starting with a dot"));
    let mut f = fixture(Mode::Accept).await;
    f.options.certificate_sha256 = "0".repeat(64);
    let error = submit(
        &f.options,
        "sender@example.test",
        &["recipient@example.test".into()],
        RAW,
    )
    .await
    .unwrap_err();
    assert!(!error.uncertain);
    assert_eq!(f.auth.load(Ordering::SeqCst), 0);
    f.task.await.unwrap();
}
#[tokio::test]
async fn no_retry_after_lost_data_response_and_bounded_malformed_reply() {
    for mode in [Mode::Drop, Mode::AuthDenied, Mode::Flood] {
        let f = fixture(mode).await;
        let error = submit(
            &f.options,
            "sender@example.test",
            &["recipient@example.test".into()],
            RAW,
        )
        .await
        .unwrap_err();
        assert_eq!(error.uncertain, matches!(mode, Mode::Drop));
        if matches!(mode, Mode::AuthDenied) {
            assert_eq!(error.response_code, Some(535));
        }
        assert_eq!(f.accepted.load(Ordering::SeqCst), 0);
        f.task.await.unwrap();
    }
}

#[tokio::test]
async fn explicit_smtp_4xx_remains_definitive_before_and_after_data_without_retry() {
    for mode in [Mode::BeforeDataRejected, Mode::AfterDataRejected] {
        let f = fixture(mode).await;
        let failure = submit(
            &f.options,
            "sender@example.test",
            &["recipient@example.test".into()],
            RAW,
        )
        .await
        .unwrap_err();
        assert!(!failure.uncertain);
        assert_eq!(failure.response_code, Some(450));
        assert_eq!(f.auth.load(Ordering::SeqCst), 1);
        assert_eq!(f.accepted.load(Ordering::SeqCst), 0);
        let observed = f.task.await.unwrap();
        assert_eq!(
            observed.is_empty(),
            matches!(mode, Mode::BeforeDataRejected)
        );
    }
}
