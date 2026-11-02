use serde_json::json;
use webdock_mail_protocol::native_control::{NativeControlClient, decode_command};
#[test]
fn strict_control_json_and_identity_limits() {
    assert!(decode_command(br#"{"action":"send.claim"}"#).is_ok());
    for raw in [
        b"null".as_slice(),
        b"[]",
        b"{} {}",
        br#"{"action":"a","action":"b"}"#,
        br#"{"x":{"a":1,"a":2}}"#,
    ] {
        assert!(decode_command(raw).is_err());
    }
    let mut max = vec![b' '; 163840];
    max[..2].copy_from_slice(b"{}");
    assert!(decode_command(&max).is_ok());
    max.push(b' ');
    assert!(decode_command(&max).is_err());
}
#[tokio::test]
async fn client_rejects_invalid_generation_and_nonobject_before_network() {
    let channel = tonic::transport::Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
    assert!(NativeControlClient::new(channel.clone(), 0).is_err());
    assert!(NativeControlClient::new(channel.clone(), u64::MAX).is_err());
    let client = NativeControlClient::new(channel, 1).unwrap();
    assert_eq!(client.call(json!([])).await.unwrap_err().status, 400);
}

struct Slow {
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl webdock_mail_protocol::native_control::Control for Slow {
    async fn call(
        &self,
        peer: &webdock_plugin_runtime::PeerIdentity,
        _generation: u64,
        _command: serde_json::Value,
    ) -> Result<serde_json::Value, webdock_mail_protocol::native_control::ControlError> {
        assert!(matches!(
            peer,
            webdock_plugin_runtime::PeerIdentity::Unix { .. }
        ));
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        Ok(serde_json::json!({"id":"123","state":"queued"}))
    }
}
#[tokio::test]
async fn actual_unix_client_deadline_is_bounded_and_never_retries() {
    use std::sync::{Arc, atomic::Ordering};
    let control = Arc::new(Slow {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let dir = tempfile::tempdir().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = dir.path().join("control.sock");
    let mut socket = webdock_plugin_runtime::UnixSocket::bind(&path).unwrap();
    let incoming = socket.incoming().unwrap();
    let service = webdock_mail_protocol::native_control::ControlServer::service(control.clone());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming_shutdown(incoming, async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
        drop(socket);
    });
    let client = NativeControlClient::new(
        webdock_plugin_runtime::unix_channel(&path).await.unwrap(),
        1,
    )
    .unwrap();
    assert_eq!(
        client
            .call(json!({"padding":"x".repeat(163840)}))
            .await
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(
        control.calls.load(Ordering::SeqCst),
        0,
        "oversized command never reaches backend"
    );
    let started = std::time::Instant::now();
    assert_eq!(
        client
            .call(serde_json::json!({"action":"send.claim"}))
            .await
            .unwrap_err()
            .status,
        503
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(14)
            && started.elapsed() < std::time::Duration::from_secs(18)
    );
    assert_eq!(control.calls.load(Ordering::SeqCst), 1);
    drop(client);
    stop.send(()).unwrap();
    task.await.unwrap();
}
