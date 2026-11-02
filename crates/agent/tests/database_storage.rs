use chrono::{Duration, Utc};
use webdock_agent::storage::database::DatabaseStoragePacket;
fn packet() -> DatabaseStoragePacket {
    serde_json::from_value(serde_json::json!({"version":1,"installationID":"456","clusterID":"789","revision":1,"node":"fixture-node","poolID":"123e4567-e89b-42d3-a456-426614174000","volumeBytes":67108864,"leaseUntil":Utc::now()+Duration::seconds(120)})).unwrap()
}
#[test]
fn database_storage_preserves_exact_physical_installation_contract() {
    let value = packet();
    value.validate().unwrap();
    let pv = value
        .persistent_volume(std::path::Path::new("/owned/456/mount/data"))
        .unwrap();
    assert_eq!(pv["metadata"]["name"], "wd-db-456");
    assert_eq!(
        pv["spec"]["claimRef"],
        serde_json::json!({"namespace":"wd-db-456","name":"postgres-1"})
    );
    assert_eq!(pv["spec"]["persistentVolumeReclaimPolicy"], "Retain");
    assert_eq!(pv["spec"]["local"]["path"], "/owned/456/mount/data");
    let mut bad = packet();
    bad.lease_until = Utc::now() + Duration::seconds(301);
    assert!(bad.validate().is_err());
    bad.lease_until = Utc::now() - Duration::seconds(1);
    assert!(bad.validate().is_err());
    bad = packet();
    bad.revision = 2147483647;
    assert!(bad.validate().is_err());
    bad = packet();
    bad.version = 2;
    assert!(bad.validate().is_err());
    bad = packet();
    bad.node = "../node".into();
    assert!(bad.validate().is_err());
    let mut json = serde_json::to_value(packet()).unwrap();
    json["path"] = "/etc".into();
    assert!(serde_json::from_value::<DatabaseStoragePacket>(json).is_err());
}

#[tokio::test]
#[ignore = "requires explicitly owned privileged disposable container; no host mounts"]
async fn disposable_database_pool_keeps_uid_content_identity_and_separate_rpc() {
    use std::{
        os::unix::fs::{MetadataExt, PermissionsExt},
        path::Path,
    };
    use webdock_agent::storage::{
        StorageClient, StorageKind, StoragePool, VolumeState, database::DatabaseStorageClient,
    };
    assert_eq!(
        std::env::var("WEBDOCK_DISPOSABLE_STORAGE_TEST").as_deref(),
        Ok("yes")
    );
    assert!(Path::new("/.dockerenv").exists());
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let base = tempfile::Builder::new()
        .prefix("webdock-db-storage-owned-")
        .tempdir()
        .unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = base.path().join("volumes");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = base.path().join("config.json");
    let mut request = packet();
    request.pool_id = uuid::Uuid::new_v4().to_string();
    std::fs::write(&config,serde_json::to_vec(&serde_json::json!({"root":root,"node":request.node,"clusterID":request.cluster_id,"poolID":request.pool_id,"capacityBytes":268435456,"reserveBytes":67108864})).unwrap()).unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    StoragePool::initialize(&config).unwrap();
    drop(StoragePool::open_kind(&config, StorageKind::Database).unwrap());
    assert!(StoragePool::open(&config).is_err());
    let socket = base.path().join("database.sock");
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let server_config = config.clone();
    let server_socket = socket.clone();
    let server = tokio::spawn(async move {
        webdock_agent::storage::serve_kind(
            &server_config,
            &server_socket,
            StorageKind::Database,
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    let test_root = root.clone();
    let test = tokio::spawn(async move {
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            StorageClient::new(socket.clone())
                .information()
                .await
                .is_err()
        );
        let client = DatabaseStorageClient::new(socket);
        let nodes = vec!["other-node".into(), request.node.clone()];
        assert!(client.capability(&nodes).await.unwrap().ready);
        let first = client.ensure(&request).await.unwrap();
        let path = Path::new(
            first.persistent_volume["spec"]["local"]["path"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(path, test_root.join("456/mount/data"));
        let metadata = std::fs::metadata(path).unwrap();
        assert_eq!((metadata.uid(), metadata.gid()), (26, 26));
        std::fs::write(path.join("proof"), b"retained-postgres-data").unwrap();
        let original = std::fs::read(test_root.join("_pool/state.json")).unwrap();
        let capability = client.capability(&nodes).await.unwrap();
        assert!(capability.ready);
        assert_eq!(capability.allocated_bytes, 67108864);
        assert_eq!(capability.allocations.len(), 1);
        assert_eq!(
            capability.allocations[0].installation_id,
            request.installation_id
        );
        client.retained(&request).await.unwrap();
        assert_eq!(
            client.state(&request).await.unwrap().state,
            VolumeState::Ready
        );
        let mut wrong = request.clone();
        wrong.pool_id = uuid::Uuid::new_v4().to_string();
        assert!(client.ensure(&wrong).await.is_err());
        wrong = request.clone();
        wrong.volume_bytes *= 2;
        assert!(client.ensure(&wrong).await.is_err());
        std::os::unix::fs::chown(path, Some(65532), Some(65532)).unwrap();
        assert!(!client.capability(&nodes).await.unwrap().ready);
        assert!(client.ensure(&request).await.is_err());
        std::os::unix::fs::chown(path, Some(26), Some(26)).unwrap();
        assert!(
            std::process::Command::new("/usr/bin/umount")
                .arg(test_root.join("456/mount"))
                .status()
                .unwrap()
                .success()
        );
        assert!(!client.capability(&nodes).await.unwrap().ready);
        request.revision = 2;
        client.ensure(&request).await.unwrap();
        assert_eq!(
            std::fs::read(path.join("proof")).unwrap(),
            b"retained-postgres-data"
        );
        assert_eq!(
            std::fs::read(test_root.join("_pool/state.json")).unwrap(),
            original
        );
        client.purge(&request).await.unwrap();
        client.purge(&request).await.unwrap();
        assert_eq!(
            client.state(&request).await.unwrap().state,
            VolumeState::Purged
        );
        assert_eq!(client.capability(&nodes).await.unwrap().allocated_bytes, 0);
        assert!(client.ensure(&request).await.is_err());
    })
    .await;
    let _ = stop_tx.send(());
    server.await.unwrap().unwrap();
    let mount = root.join("456/mount");
    if mount.exists() {
        let _ = std::process::Command::new("/usr/bin/umount")
            .arg(&mount)
            .output();
    }
    let image = root.join("456/volume.ext4");
    if image.exists() {
        let output = std::process::Command::new("/usr/sbin/losetup")
            .args(["--json", "--list", "--associated"])
            .arg(&image)
            .output()
            .unwrap();
        assert!(output.status.success());
        let loops: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        for entry in loops["loopdevices"].as_array().unwrap() {
            assert_eq!(entry["back-file"], image.to_str().unwrap());
            let device = entry["name"].as_str().unwrap();
            assert!(
                device
                    .strip_prefix("/dev/loop")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            );
            assert!(
                std::process::Command::new("/usr/sbin/losetup")
                    .args(["--detach", device])
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    test.unwrap();
}

#[test]
fn database_capacity_rejects_foreign_nodes_duplicate_allocations_and_unbounded_sizes() {
    use webdock_agent::storage::database::DatabaseStorageCapability;
    let capability: DatabaseStorageCapability = serde_json::from_value(serde_json::json!({"node":"fixture-node","poolID":"123e4567-e89b-42d3-a456-426614174000","capacityBytes":268435456,"ready":true,"availableBytes":134217728,"allocatedBytes":67108864,"allocations":[{"installationID":"456","volumeBytes":67108864}]})).unwrap();
    let nodes = vec!["fixture-node".into()];
    capability.validate(&nodes).unwrap();
    assert!(capability.validate(&["foreign".into()]).is_err());
    let mut bad = capability.clone();
    bad.allocations.push(bad.allocations[0].clone());
    assert!(bad.validate(&nodes).is_err());
    let mut bad = capability.clone();
    bad.capacity_bytes = u64::MAX;
    assert!(bad.validate(&nodes).is_err());
    let mut bad = capability;
    bad.allocations[0].volume_bytes = 1;
    assert!(bad.validate(&nodes).is_err());
}
