use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use webdock_mail_protocol::{
    runtime::{Authority, Handler, PeerIdentity, Runtime, UnixSocket},
    *,
};
struct Live {
    active: AtomicBool,
    attempted: Mutex<HashSet<String>>,
    checked: tokio::sync::Notify,
    lose_begin: AtomicBool,
}
#[async_trait::async_trait]
impl Authority for Live {
    async fn guard(&self, peer: &PeerIdentity, _claims: &LeaseClaims, _token: &str) -> Result<()> {
        self.checked.notify_one();
        if self.active.load(Ordering::SeqCst) && matches!(peer, PeerIdentity::Unix { .. }) {
            Ok(())
        } else {
            Err(Denied)
        }
    }
    async fn begin_attempt(
        &self,
        peer: &PeerIdentity,
        claims: &LeaseClaims,
        token: &str,
    ) -> Result<Value> {
        self.guard(peer, claims, token).await?;
        if !self.attempted.lock().unwrap().insert(claims.id.clone())
            || self.lose_begin.load(Ordering::SeqCst)
        {
            return Err(Denied);
        }
        Ok(json!({"providerSecret":"only-after-attempt"}))
    }
}
struct Sink(AtomicUsize);
#[async_trait::async_trait]
impl Handler for Sink {
    async fn submit(
        &self,
        claims: &LeaseClaims,
        metadata: Value,
        mime: Vec<u8>,
        configuration: Value,
    ) -> wire::SendReceipt {
        assert_eq!(claims.mime_bytes, mime.len() as u64);
        assert_eq!(metadata["from"], "a@example.test");
        assert_eq!(configuration["providerSecret"], "only-after-attempt");
        self.0.fetch_add(1, Ordering::SeqCst);
        wire::SendReceipt {
            state: wire::send_receipt::State::Accepted as i32,
            provider_id: "receipt_1".into(),
        }
    }
}
fn frames(key: &SigningKey, mime: &[u8]) -> Vec<wire::SendFrame> {
    let metadata = br#"{"from":"a@example.test","to":["b@example.test"]}"#.to_vec();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = LeaseClaims {
        v: 1,
        purpose: "mail-send".into(),
        audience: "webdock.mail.runtime.v1".into(),
        id: uuid::Uuid::new_v4().to_string(),
        plugin_id: "mail-fixture".into(),
        generation: 1,
        customer_id: "123".into(),
        job_id: "456".into(),
        source_id: "key:789".into(),
        transport_revision: 1,
        grant_revision: 1,
        configuration_revision: 0,
        descriptor_sha256: "a".repeat(64),
        binding_sha256: "b".repeat(64),
        worker_id: "mail-worker".into(),
        worker_generation: 1,
        worker_credential_sha256: "c".repeat(64),
        metadata_sha256: sha256(&metadata),
        mime_sha256: sha256(mime),
        mime_bytes: mime.len() as u64,
        iat: now,
        exp: now + 60,
    };
    let mut frames = vec![wire::SendFrame {
        offset: 0,
        data: vec![],
        header: Some(wire::SendHeader {
            lease_token: claims.sign(key).unwrap(),
            metadata_json: metadata,
        }),
    }];
    for (index, chunk) in mime.chunks(FRAME_BYTES).enumerate() {
        frames.push(wire::SendFrame {
            offset: (index * FRAME_BYTES) as u64,
            data: chunk.to_vec(),
            header: None,
        });
    }
    frames
}
#[tokio::test]
async fn private_stream_checks_bounds_integrity_live_authority_and_durable_attempt_before_io() {
    let key = SigningKey::from_bytes(&[74; 32]);
    let live = Arc::new(Live {
        active: AtomicBool::new(true),
        attempted: Mutex::new(HashSet::new()),
        checked: tokio::sync::Notify::new(),
        lose_begin: AtomicBool::new(false),
    });
    let sink = Arc::new(Sink(AtomicUsize::new(0)));
    let runtime = Runtime::new(
        "mail-fixture".into(),
        "0.1.0".into(),
        1,
        "a".repeat(64),
        key.verifying_key(),
        live.clone(),
        sink.clone(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("mail.sock");
    let socket = UnixSocket::bind(&path).unwrap();
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(runtime.serve_unix(socket, async {
        let _ = shutdown.await;
    }));
    let channel = webdock_plugin_runtime::unix_channel(&path).await.unwrap();
    let mut client = wire::mail_runtime_client::MailRuntimeClient::new(channel);
    assert_eq!(
        client
            .describe(wire::DescribeRequest {})
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    let mime = vec![b'x'; FRAME_BYTES * 3];
    let valid = frames(&key, &mime);
    let receipt = client
        .submit(tokio_stream::iter(valid.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(receipt.state, wire::send_receipt::State::Accepted as i32);
    assert!(client.submit(tokio_stream::iter(valid)).await.is_err());
    assert_eq!(sink.0.load(Ordering::SeqCst), 1);
    for variation in ["offset", "bytes", "metadata", "oversize", "extra-header"] {
        let mut invalid = frames(&key, &mime);
        match variation {
            "offset" => invalid[1].offset = 1,
            "bytes" => invalid[1].data[0] = b'y',
            "metadata" => invalid[0].header.as_mut().unwrap().metadata_json = b"{}".to_vec(),
            "oversize" => invalid[1].data.push(b'x'),
            _ => invalid[1].header = invalid[0].header.clone(),
        }
        assert!(client.submit(tokio_stream::iter(invalid)).await.is_err());
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
    }
    live.lose_begin.store(true, Ordering::SeqCst);
    let lost = frames(&key, &mime);
    assert!(
        client
            .submit(tokio_stream::iter(lost.clone()))
            .await
            .is_err()
    );
    live.lose_begin.store(false, Ordering::SeqCst);
    assert!(client.submit(tokio_stream::iter(lost)).await.is_err());
    assert_eq!(sink.0.load(Ordering::SeqCst), 1);
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    let mut input = frames(&key, &mime).into_iter();
    tx.send(input.next().unwrap()).await.unwrap();
    // Drain any prior notification, then wait until this stream's header is authorized.
    let _ =
        tokio::time::timeout(std::time::Duration::from_millis(1), live.checked.notified()).await;
    let pending = tokio::spawn(async move {
        client
            .submit(tokio_stream::wrappers::ReceiverStream::new(rx))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), live.checked.notified())
        .await
        .unwrap();
    live.active.store(false, Ordering::SeqCst);
    for frame in input {
        if tx.send(frame).await.is_err() {
            break;
        }
    }
    drop(tx);
    assert!(pending.await.unwrap().is_err());
    assert_eq!(sink.0.load(Ordering::SeqCst), 1);
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(!path.exists());
}
