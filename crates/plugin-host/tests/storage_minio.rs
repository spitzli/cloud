//! Uses the already-running dev MinIO, owns one UUID bucket, and never logs presigned requests.
use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::PathBuf, process::Command, sync::Arc};
use webdock_plugin_host::storage::{StorageAuthority, StorageGateway};
use webdock_plugin_http::storage::StorageBinding;
use webdock_plugin_protocol::{
    Result,
    storage::{Attachment, StorageDirection, StoragePlan},
};
struct Live;
#[async_trait]
impl StorageAuthority for Live {
    async fn check(&self) -> Result<()> {
        Ok(())
    }
}
struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn command(&self, action: &str) -> bool {
        Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/storage_minio.py"
            ))
            .arg(action)
            .arg(&self.path)
            .output()
            .unwrap()
            .status
            .success()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(self.command("cleanup"), "owned MinIO cleanup");
    }
}
#[tokio::test]
#[ignore = "requires the already-running dev MinIO on loopback:55900 and its protected env file"]
async fn owned_minio_binary_roundtrip_verifies_three_mib_and_cleans_exact_bucket() {
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let fixture = Fixture {
        path: dir
            .path()
            .join(format!("{}.json", uuid::Uuid::new_v4().simple())),
    };
    assert!(fixture.command("prepare"), "owned MinIO preparation");
    let v: Value = serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
    let gateway = StorageGateway::loopback_test(dir.path().into()).unwrap();
    let bytes = vec![163u8; 3 * 1024 * 1024 + 17];
    let sha = hex::encode(Sha256::digest(&bytes));
    let mut plan = StoragePlan {
        version: 1,
        binding: "objects".into(),
        direction: StorageDirection::Upload,
        resource_epoch: "active-1".into(),
        resource_id: "fixture".into(),
        url: v["put"].as_str().unwrap().into(),
        headers: Default::default(),
        expires_at: v["expiresAt"].as_i64().unwrap(),
        exact_bytes: bytes.len() as u64,
        expected_sha256: Some(sha.clone()),
        attachment: Attachment {
            filename: "fixture.bin".into(),
            content_type: "application/octet-stream".into(),
        },
    };
    let binding = StorageBinding {
        origin: v["origin"].as_str().unwrap().into(),
        addresses: vec!["127.0.0.1".parse().unwrap()],
        ca: None,
        path_prefix: format!("/{}/", v["bucket"].as_str().unwrap()),
    };
    let staged = gateway
        .stage(
            &plan,
            futures_util::stream::iter(bytes.chunks(65536).map(|b| Ok(b.to_vec()))),
            Arc::new(Live),
        )
        .await
        .unwrap();
    let receipt = gateway
        .upload(
            &plan,
            gateway.target(&plan, binding.clone()).unwrap(),
            staged,
        )
        .await
        .unwrap();
    assert_eq!(receipt.bytes, bytes.len() as u64);
    assert_eq!(receipt.sha256, sha);
    assert!(receipt.etag.is_some());
    plan.direction = StorageDirection::Download;
    plan.url = v["get"].as_str().unwrap().into();
    let mut downloaded = gateway
        .download(
            &plan,
            gateway.target(&plan, binding).unwrap(),
            Arc::new(Live),
        )
        .await
        .unwrap()
        .into_stream();
    let mut hash = Sha256::new();
    let mut count = 0;
    while let Some(chunk) = downloaded.next_chunk().await.unwrap() {
        assert!(chunk.len() <= 65536);
        count += chunk.len();
        hash.update(chunk);
    }
    assert_eq!(count, bytes.len());
    assert_eq!(hex::encode(hash.finalize()), sha);
    drop(downloaded);
    drop(fixture);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
