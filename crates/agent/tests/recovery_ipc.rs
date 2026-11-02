//! Owned kernel-credential IPC probe only; no fake Core admission or recovery-set success.
#![cfg(feature = "docker")]
use std::{os::unix::fs::PermissionsExt, path::Path};
use webdock_agent::storage::{StorageClient, StorageKind, StoragePool};
fn write(path: &Path, value: &impl serde::Serialize) {
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
#[tokio::test]
#[ignore = "requires an exclusively owned root fixture with actual UID switching; no host mounts/socket"]
async fn disposable_recovery_ipc_enforces_kernel_peer_and_preserves_legacy_denial() {
    if let Ok(mode) = std::env::var("WEBDOCK_RECOVERY_IPC_CHILD") {
        let socket = std::env::var("WEBDOCK_RECOVERY_IPC_SOCKET").unwrap();
        if mode == "spoof-root" {
            use tokio::io::AsyncReadExt;
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut byte = [0];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut byte))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                read, 0,
                "client must reject non-Root kernel server before writing a request"
            );
            return;
        }
        let client = StorageClient::new(socket.into());
        let result = client.recovery_pool_identity().await;
        if mode == "info" {
            assert_eq!(rustix::process::geteuid().as_raw(), 65532);
            result.unwrap();
        } else if mode == "denied" {
            assert!(result.is_err());
        } else {
            let packet = webdock_agent::storage::StoragePacket::new(
                "456",
                "123",
                "789",
                1,
                67108864,
                webdock_supervisor::timestamp().unwrap() + 30,
            )
            .unwrap();
            assert!(client.ensure(&packet).await.is_err());
            assert!(client.purge(&packet).await.is_err());
        }
        return;
    }
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_RECOVERY_IPC").as_deref(),
        Ok("yes")
    );
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    assert!(Path::new("/.dockerenv").is_file());
    let root = Path::new("/owned/recovery-ipc-fixture");
    std::fs::create_dir(root).unwrap();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["pool", "verify", "socket", "agent-stage"] {
        std::fs::create_dir(root.join(name)).unwrap();
        std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::os::unix::fs::chown(root.join("agent-stage"), Some(65532), Some(65532)).unwrap();
    let config = root.join("pool.json");
    write(
        &config,
        &serde_json::json!({"root":root.join("pool"),"node":"owned-fixture","clusterID":"789","poolID":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","capacityBytes":1073741824,"reserveBytes":67108864}),
    );
    // Public verification material only; this probe requests info/denials and never mints a grant.
    write(
        &root.join("core-signers"),
        &"fixture-public-trust-not-used-for-admission",
    );
    write(
        &root.join("operator-signers"),
        &"fixture-public-trust-not-used-for-admission",
    );
    let trust = root.join("trust.json");
    write(
        &trust,
        &serde_json::json!({"version":1,"installationID":"owned-ipc-fixture","coreSigners":root.join("core-signers"),"operatorSigners":root.join("operator-signers"),"postgresTlsCertificate":null,"postgresTlsKey":null,"verificationDirectory":root.join("verify"),"agentStagingDirectory":root.join("agent-stage"),"socketDirectory":root.join("socket"),"targets":[{"id":"fresh","sha256":"a".repeat(64),"nonce":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","action":"restore","profile":"mail-installation-v1","capacityBytes":67108864,"volumes":{},"applicationTarget":null}]}),
    );
    StoragePool::initialize(&config).unwrap();
    let socket = root.join("socket/helper.sock");
    let (stop, shutdown) = tokio::sync::oneshot::channel();
    let c = config.clone();
    let s = socket.clone();
    let t = trust.clone();
    let server = tokio::spawn(async move {
        webdock_agent::storage::serve_kind_with_recovery(
            &c,
            &s,
            StorageKind::Apps,
            Some(&t),
            async {
                let _ = shutdown.await;
            },
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !socket.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    for (uid, gid, mode) in [
        (65532, 65532, "info"),
        (65533, 65532, "denied"),
        (65532, 65532, "legacy-denied"),
    ] {
        let status = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "disposable_recovery_ipc_enforces_kernel_peer_and_preserves_legacy_denial",
            ])
            .env("WEBDOCK_RECOVERY_IPC_CHILD", mode)
            .env("WEBDOCK_RECOVERY_IPC_SOCKET", &socket)
            .uid(uid)
            .gid(gid)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "kernel-credential IPC child failed");
    }
    assert!(!root.join("pool/456").exists());
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();
    let denied = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "disposable_recovery_ipc_enforces_kernel_peer_and_preserves_legacy_denial",
        ])
        .env("WEBDOCK_RECOVERY_IPC_CHILD", "denied")
        .env("WEBDOCK_RECOVERY_IPC_SOCKET", &socket)
        .uid(65532)
        .gid(65532)
        .status()
        .await
        .unwrap();
    assert!(denied.success());
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660)).unwrap();
    let spoof_parent = root.join("spoof");
    std::fs::create_dir(&spoof_parent).unwrap();
    std::fs::set_permissions(&spoof_parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::chown(&spoof_parent, Some(65532), Some(65532)).unwrap();
    let spoof_socket = spoof_parent.join("fake.sock");
    let mut spoof = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "disposable_recovery_ipc_enforces_kernel_peer_and_preserves_legacy_denial",
        ])
        .env("WEBDOCK_RECOVERY_IPC_CHILD", "spoof-root")
        .env("WEBDOCK_RECOVERY_IPC_SOCKET", &spoof_socket)
        .uid(65532)
        .gid(65532)
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !spoof_socket.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    std::os::unix::fs::chown(&spoof_parent, Some(0), Some(65532)).unwrap();
    std::fs::set_permissions(&spoof_parent, std::fs::Permissions::from_mode(0o710)).unwrap();
    std::os::unix::fs::chown(&spoof_socket, Some(0), Some(65532)).unwrap();
    std::fs::set_permissions(&spoof_socket, std::fs::Permissions::from_mode(0o660)).unwrap();
    assert!(
        StorageClient::new(spoof_socket)
            .recovery_pool_identity()
            .await
            .is_err()
    );
    assert!(spoof.wait().await.unwrap().success());
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    assert!(!socket.exists());
    std::fs::remove_dir_all(root).unwrap();
}
