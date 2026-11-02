use async_trait::async_trait;
use futures_util::stream;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use webdock_plugin_host::storage::{StorageAuthority, StorageGateway};
use webdock_plugin_protocol::{
    PluginError, Result,
    storage::{Attachment, StorageDirection, StoragePlan},
};
struct Live(AtomicBool);
#[async_trait]
impl StorageAuthority for Live {
    async fn check(&self) -> Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(PluginError::denied())
        }
    }
}
fn plan() -> StoragePlan {
    StoragePlan {
        version: 1,
        binding: "objects".into(),
        direction: StorageDirection::Upload,
        resource_epoch: "active-1".into(),
        resource_id: "owned".into(),
        url: "http://127.0.0.1/private/key".into(),
        headers: Default::default(),
        expires_at: chrono::Utc::now().timestamp() + 300,
        exact_bytes: 3,
        expected_sha256: Some(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        ),
        attachment: Attachment {
            filename: "file.bin".into(),
            content_type: "application/octet-stream".into(),
        },
    }
}
#[tokio::test]
async fn staging_enforces_actual_size_hash_revocation_and_private_anonymous_files() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let gateway = StorageGateway::new(dir.path().into()).unwrap();
    let live = Arc::new(Live(AtomicBool::new(true)));
    let staged = gateway
        .stage(&plan(), stream::iter([Ok(b"abc".to_vec())]), live.clone())
        .await
        .unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    assert!(
        gateway
            .stage(&plan(), stream::iter([Ok(b"abd".to_vec())]), live.clone())
            .await
            .is_err()
    );
    assert!(
        gateway
            .stage(&plan(), stream::iter([Ok(b"abcd".to_vec())]), live.clone())
            .await
            .is_err()
    );
    assert!(
        gateway
            .stage(&plan(), stream::iter([Ok(b"ab".to_vec())]), live.clone())
            .await
            .is_err()
    );
    live.0.store(false, Ordering::SeqCst);
    assert!(staged.into_stream().next_chunk().await.is_err());
    assert!(
        gateway
            .stage(&plan(), stream::iter([Ok(b"abc".to_vec())]), live)
            .await
            .is_err()
    );
}

async fn serve_once(
    response: Vec<u8>,
    early: bool,
) -> (String, tokio::task::JoinHandle<(usize, String)>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut data = Vec::new();
        let mut buffer = [0; 65536];
        loop {
            let n = socket.read(&mut buffer).await.unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..n]);
            if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                let header = std::str::from_utf8(&data[..end]).unwrap().to_owned();
                let size = header
                    .lines()
                    .find_map(|h| {
                        h.strip_prefix("content-length: ")
                            .and_then(|s| s.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if early || data.len() >= end + 4 + size {
                    let _ = socket.write_all(&response).await;
                    return (data.len() - end - 4, header);
                }
            }
        }
        (0, String::new())
    });
    (origin, task)
}
fn binding(origin: &str) -> webdock_plugin_http::storage::StorageBinding {
    webdock_plugin_http::storage::StorageBinding {
        origin: origin.into(),
        addresses: vec!["127.0.0.1".parse().unwrap()],
        ca: None,
        path_prefix: "/private/".into(),
    }
}
fn gateway() -> (tempfile::TempDir, StorageGateway) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let g = StorageGateway::loopback_test(dir.path().into()).unwrap();
    (dir, g)
}
#[tokio::test]
async fn actual_http_transfers_large_binary_with_hash_and_opaque_receipt() {
    use sha2::{Digest, Sha256};
    let (_dir, gateway) = gateway();
    let live = Arc::new(Live(AtomicBool::new(true)));
    let bytes = vec![37u8; 3 * 1024 * 1024 + 1];
    let mut p = plan();
    p.exact_bytes = bytes.len() as u64;
    p.expected_sha256 = Some(hex::encode(Sha256::digest(&bytes)));
    let (origin,server)=serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"0123456789abcdef0123456789abcdef-2\"\r\nConnection: close\r\n\r\n".to_vec(),false).await;
    p.url = format!("{origin}/private/key?X-Amz-Signature=private");
    use base64::Engine;
    p.url
        .push_str("&X-Amz-SignedHeaders=host%3Bif-none-match%3Bx-amz-checksum-sha256");
    p.headers.insert("if-none-match".into(), "*".into());
    p.headers.insert(
        "x-amz-checksum-sha256".into(),
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&bytes)),
    );
    let staged = gateway
        .stage(
            &p,
            stream::iter(bytes.chunks(65536).map(|b| Ok(b.to_vec()))),
            live.clone(),
        )
        .await
        .unwrap();
    let receipt = gateway
        .upload(&p, gateway.target(&p, binding(&origin)).unwrap(), staged)
        .await
        .unwrap();
    assert_eq!(receipt.bytes, p.exact_bytes);
    assert_ne!(receipt.etag.as_deref(), Some(receipt.sha256.as_str()));
    let (count, headers) = server.await.unwrap();
    assert_eq!(count, p.exact_bytes as usize);
    assert!(headers.contains("if-none-match: *"));
    assert!(headers.contains("x-amz-checksum-sha256:"));
    assert!(!headers.contains("cookie:"));
    assert!(!headers.contains("authorization:"));
    p.direction = StorageDirection::Download;
    p.headers.clear();
    p.headers
        .insert("if-match".into(), "\"0123456789abcdef-2\"".into());
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .into_bytes();
    response.extend_from_slice(&bytes);
    let (origin, server) = serve_once(response, false).await;
    p.url = format!("{origin}/private/key?X-Amz-SignedHeaders=host%3Bif-match");
    let staged = gateway
        .download(&p, gateway.target(&p, binding(&origin)).unwrap(), live)
        .await
        .unwrap();
    let mut stream = staged.into_stream();
    let mut observed = Sha256::new();
    let mut count = 0;
    while let Some(chunk) = stream.next_chunk().await.unwrap() {
        assert!(chunk.len() <= 65536);
        count += chunk.len();
        observed.update(chunk);
    }
    assert_eq!(count, bytes.len());
    assert_eq!(hex::encode(observed.finalize()), p.expected_sha256.unwrap());
    let (_, headers) = server.await.unwrap();
    assert!(headers.contains("if-match: \"0123456789abcdef-2\""));
}
#[tokio::test]
async fn redirects_truncation_wrong_hash_and_provider_diagnostics_never_release_bytes() {
    let (_dir, gateway) = gateway();
    let live = Arc::new(Live(AtomicBool::new(true)));
    let mut p = plan();
    p.direction = StorageDirection::Download;
    for response in [
  b"HTTP/1.1 307 Redirect\r\nContent-Length: 0\r\nLocation: http://169.254.169.254/?private-signature\r\nConnection: close\r\n\r\n".to_vec(),
  b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nab".to_vec(),
  b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabd".to_vec(),
  b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd".to_vec(),
 ] {
  let (origin,server)=serve_once(response,false).await;p.url=format!("{origin}/private/key?X-Amz-Signature=private-signature");
  let error=gateway.download(&p,gateway.target(&p,binding(&origin)).unwrap(),live.clone()).await.err().unwrap();
  assert!(!error.message.contains(&origin));assert!(!error.message.contains("private-signature"));server.await.unwrap();
 }
}
#[tokio::test]
async fn staging_capacity_is_held_until_delivery_and_unknown_checksums_are_only_measured() {
    let (_dir, gateway) = gateway();
    let live = Arc::new(Live(AtomicBool::new(true)));
    let mut held = vec![];
    let mut p = plan();
    p.direction = StorageDirection::Download;
    p.expected_sha256 = None;
    for _ in 0..4 {
        held.push(
            gateway
                .stage(&p, stream::iter([Ok(b"abc".to_vec())]), live.clone())
                .await
                .unwrap(),
        );
    }
    assert!(
        gateway
            .stage(&p, stream::iter([Ok(b"abc".to_vec())]), live.clone())
            .await
            .is_err()
    );
    held.pop();
    assert!(
        gateway
            .stage(&p, stream::iter([Ok(b"abc".to_vec())]), live)
            .await
            .is_ok()
    );
}
#[tokio::test]
async fn authority_loss_interrupts_a_pending_body_without_waiting_for_input() {
    let (_dir, gateway) = gateway();
    let live = Arc::new(Live(AtomicBool::new(true)));
    let source = live.clone();
    let task = tokio::spawn(async move {
        gateway
            .stage(&plan(), stream::pending::<Result<Vec<u8>>>(), source)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    live.0.store(false, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}
#[tokio::test]
async fn binary_stream_can_outlive_the_ordinary_json_deadline_with_live_authority() {
    let (_dir, gateway) = gateway();
    let live = Arc::new(Live(AtomicBool::new(true)));
    let input = stream::once(async {
        tokio::time::sleep(std::time::Duration::from_secs(46)).await;
        Ok(b"abc".to_vec())
    });
    let staged = gateway.stage(&plan(), input, live).await.unwrap();
    assert_eq!(staged.bytes, 3);
}

#[test]
fn producer_checksum_and_content_headers_are_bound_to_the_verified_plan() {
    let (_dir, gateway) = gateway();
    let mut p = plan();
    p.headers
        .insert("x-amz-checksum-sha256".into(), "wrong-digest".into());
    assert!(
        gateway.target(&p, binding("http://127.0.0.1")).is_err(),
        "signed checksum must match the staged bytes"
    );
    p.headers.clear();
    p.headers.insert("content-type".into(), "text/plain".into());
    assert!(gateway.target(&p, binding("http://127.0.0.1")).is_err());
    p.headers.clear();
    p.headers
        .insert("x-amz-content-sha256".into(), "0".repeat(64));
    assert!(gateway.target(&p, binding("http://127.0.0.1")).is_err());
    p.headers.clear();
    p.url =
        "http://127.0.0.1/private/key?x-amz-checksum-sha256=wrong&X-Amz-SignedHeaders=host".into();
    assert!(gateway.target(&p, binding("http://127.0.0.1")).is_err());
    p.url = "http://127.0.0.1/private/key?X-Amz-SignedHeaders=host".into();
    p.headers.insert("if-none-match".into(), "*".into());
    assert!(
        gateway.target(&p, binding("http://127.0.0.1")).is_err(),
        "conditional header must actually be signed"
    );
}
