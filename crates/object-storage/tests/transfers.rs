#![cfg(feature = "local-test")]
use webdock_object_storage::{Credentials, ObjectReceipt, S3Config, Store};
#[path = "support/faults.rs"]
mod faults;

fn config() -> S3Config {
    S3Config {
        id: "build-eu-v1".into(),
        endpoint: "https://s3.eu-central-2.wasabisys.com".into(),
        region: "eu-central-2".into(),
        bucket: "webdock-builds-test".into(),
        namespace: "builds".into(),
    }
}

#[test]
fn explicit_configuration_rejects_untrusted_origins_and_paths() {
    for endpoint in [
        "http://s3.eu-central-2.wasabisys.com",
        "http://127.0.0.1:55900",
        "https://user:password@example.test",
        "https://example.test/?secret=hidden",
        "https://example.test/prefix",
        "https://example.test/#fragment",
    ] {
        let mut c = config();
        c.endpoint = endpoint.into();
        assert!(Store::s3(c, Credentials::new("key".into(), "secret".into()).unwrap()).is_err());
    }
    for namespace in [
        "",
        "../builds",
        "/builds",
        "builds//x",
        "builds%2Fx",
        "builds\\x",
    ] {
        let mut c = config();
        c.namespace = namespace.into();
        assert!(Store::s3(c, Credentials::new("key".into(), "secret".into()).unwrap()).is_err());
    }
    assert!(Credentials::new(String::new(), "secret".into()).is_err());
    let credentials = Credentials::new("private-key-id".into(), "private-secret".into()).unwrap();
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("private-key-id") && !debug.contains("private-secret"));
    let store = Store::s3(config(), credentials).unwrap();
    assert!(!format!("{store:?}").contains("private-secret"));
}

#[tokio::test]
async fn backend_binding_cannot_be_reassigned_to_a_different_namespace() {
    let original = Store::memory("fixture-v1", "builds").unwrap();
    let receipt = original.upload("100/200/1", &b"x"[..], 10).await.unwrap();
    let replacement = Store::memory("fixture-v1", "exports").unwrap();
    let mut forged = receipt;
    forged.object_key = forged.object_key.replacen("builds/", "exports/", 1);
    assert!(
        replacement
            .validate_receipt("100/200/1", &forged, 10)
            .is_err()
    );
}

#[tokio::test]
async fn receipt_survives_client_restart_and_binds_bytes_backend_and_scope() {
    let store = Store::memory("fixture-v1", "builds").unwrap();
    let data = b"a verified private artifact";
    let receipt = store
        .upload("100/200/1", &data[..], data.len() as u64)
        .await
        .unwrap();
    assert_eq!(receipt.size_bytes, data.len() as u64);
    assert_eq!(
        receipt.sha256,
        "7c52a586497d0f309e703e4eb4041f61648bb4a4111513f29f1542160a1afd10"
    );
    let serialized = serde_json::to_vec(&receipt).unwrap();
    let receipt: ObjectReceipt = serde_json::from_slice(&serialized).unwrap();
    let mut downloaded = Vec::new();
    store
        .download("100/200/1", &receipt, &mut downloaded, 1024)
        .await
        .unwrap();
    assert_eq!(downloaded, data);
    assert!(
        store
            .download("101/200/1", &receipt, &mut Vec::new(), 1024)
            .await
            .is_err()
    );
    let mut changed = receipt.clone();
    changed.backend_id = "different-backend".into();
    assert!(
        store
            .download("100/200/1", &changed, &mut Vec::new(), 1024)
            .await
            .is_err()
    );
    changed = receipt.clone();
    changed.sha256 = "00".repeat(32);
    assert!(
        store
            .download("100/200/1", &changed, &mut Vec::new(), 1024)
            .await
            .is_err()
    );
    changed = receipt.clone();
    changed.size_bytes += 1;
    assert!(
        store
            .download("100/200/1", &changed, &mut Vec::new(), 1024)
            .await
            .is_err()
    );
    assert!(
        store
            .download("100/200/1", &receipt, &mut Vec::new(), 1)
            .await
            .is_err()
    );
    let other = store.upload("100/200/1", &data[..], 1024).await.unwrap();
    assert_ne!(receipt.object_key, other.object_key);
}

#[tokio::test]
async fn oversized_or_interrupted_stream_never_returns_a_receipt() {
    let store = Store::memory("fixture-v1", "builds").unwrap();
    assert!(
        store
            .upload("100/200/1", &b"oversized"[..], 3)
            .await
            .is_err()
    );
    struct Broken;
    impl tokio::io::AsyncRead for Broken {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("fixture stream failure")))
        }
    }
    assert!(store.upload("100/200/1", Broken, 1024).await.is_err());
    let (reader, mut writer) = tokio::io::duplex(8);
    use tokio::io::AsyncWriteExt;
    let producer = tokio::spawn(async move {
        writer.write_all(b"first").await.unwrap();
        drop(writer);
    });
    // A closed stream is valid EOF: expected size is validated separately by product receipts.
    let receipt = store.upload("100/200/1", reader, 1024).await.unwrap();
    producer.await.unwrap();
    assert_eq!(receipt.size_bytes, 5);
    for scope in ["../escape", "100//1", "/100", "100/%2e"] {
        assert!(store.upload(scope, &b"x"[..], 10).await.is_err());
    }
}

#[tokio::test]
async fn empty_object_has_a_verified_receipt() {
    let store = Store::memory("fixture-v1", "builds").unwrap();
    let receipt = store
        .upload("100/200/1", tokio::io::empty(), 1024)
        .await
        .unwrap();
    assert_eq!(receipt.size_bytes, 0);
    store
        .download("100/200/1", &receipt, &mut Vec::new(), 1024)
        .await
        .unwrap();
}

#[tokio::test]
async fn combined_key_is_rejected_before_reading_or_uploading() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Observed(Arc<AtomicBool>);
    impl tokio::io::AsyncRead for Observed {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            self.0.store(true, Ordering::SeqCst);
            std::task::Poll::Ready(Ok(()))
        }
    }
    let store = Store::memory("fixture-v1", &"a".repeat(480)).unwrap();
    let touched = Arc::new(AtomicBool::new(false));
    assert!(
        store
            .upload("100/200/1", Observed(touched.clone()), 1024)
            .await
            .is_err()
    );
    assert!(
        !touched.load(Ordering::SeqCst),
        "invalid combined key reached the source/remote transfer"
    );
}

#[tokio::test]
#[ignore = "run through tools/tests/object-storage.py with explicitly owned MinIO resources"]
async fn real_minio_multipart_restart_integrity_and_policy_isolation() {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Fixture {
        config: S3Config,
        credentials: Credentials,
        denied_bucket: String,
    }
    let input =
        std::env::var_os("WEBDOCK_S3_TEST_CONFIG").expect("private fixture config required");
    let bytes = std::fs::read(input).unwrap();
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    let store = Store::loopback_test(fixture.config.clone(), fixture.credentials).unwrap();
    let data = vec![0x5au8; 17 * 1024 * 1024 + 13];
    let receipt = store
        .upload("100/200/1", &data[..], data.len() as u64)
        .await
        .unwrap();
    drop(store);
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    let store = Store::loopback_test(fixture.config.clone(), fixture.credentials).unwrap();
    let mut recovered = Vec::new();
    store
        .download("100/200/1", &receipt, &mut recovered, data.len() as u64)
        .await
        .unwrap();
    assert_eq!(recovered, data);
    assert!(
        store
            .download("101/200/1", &receipt, &mut Vec::new(), data.len() as u64)
            .await
            .is_err()
    );
    let mut damaged = receipt.clone();
    damaged.sha256 = "00".repeat(32);
    assert!(
        store
            .download("100/200/1", &damaged, &mut Vec::new(), data.len() as u64)
            .await
            .is_err()
    );
    assert!(
        store
            .upload("100/200/1", &data[..], 8 * 1024 * 1024 + 1)
            .await
            .is_err()
    );
    let empty = store
        .upload("100/200/1", tokio::io::empty(), 1024)
        .await
        .unwrap();
    assert_eq!(empty.size_bytes, 0);
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    let mut denied = fixture.config;
    denied.bucket = fixture.denied_bucket;
    let denied = Store::loopback_test(denied, fixture.credentials).unwrap();
    assert!(
        denied
            .upload("100/200/1", &b"denied"[..], 1024)
            .await
            .is_err()
    );
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    let bad = Store::loopback_test(
        fixture.config,
        Credentials::new(
            "invalid-fixture-key".into(),
            "invalid-fixture-secret".into(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(bad.upload("100/200/1", &b"denied"[..], 1024).await.is_err());
    let fixture: Fixture = serde_json::from_slice(&bytes).unwrap();
    let proxy = faults::FaultProxy::start(fixture.config.endpoint.clone()).await;
    let mut config = fixture.config;
    config.endpoint = proxy.endpoint.clone();
    let reconciled = Store::loopback_test(config, fixture.credentials).unwrap();
    let receipt = reconciled
        .upload("100/200/1", &b"lost completion response"[..], 1024)
        .await
        .unwrap();
    assert!(proxy.lost.load(std::sync::atomic::Ordering::SeqCst));
    let mut restored = Vec::new();
    reconciled
        .download("100/200/1", &receipt, &mut restored, 1024)
        .await
        .unwrap();
    assert_eq!(restored, b"lost completion response");
    proxy
        .reject_complete
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        reconciled
            .upload("100/200/1", &b"must not complete"[..], 1024)
            .await
            .is_err()
    );
    proxy
        .reject_complete
        .store(false, std::sync::atomic::Ordering::SeqCst);
    proxy
        .truncate_reads
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        reconciled
            .download("100/200/1", &receipt, &mut Vec::new(), 1024)
            .await
            .is_err()
    );
}
