#![cfg(feature = "local-registry-test")]
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use webdock_build::{
    artifact::ArtifactStore,
    publisher::{PublicationGuard, RegistryPublisher},
};
struct Guard {
    allowed: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl PublicationGuard for Guard {
    async fn check(&self) -> webdock_build::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.allowed.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(webdock_build::Error {
                code: "lease_revoked",
                logs: String::new(),
            })
        }
    }
}
#[path = "support/registry.rs"]
mod registry;
#[tokio::test]
#[ignore = "explicit isolated local registry with cached pinned image and real VM artifact"]
async fn publishes_verified_oci_bytes_and_rechecks_authority() {
    let registry = registry::LocalRegistry::start().await;
    let address = registry.address;
    let publisher = RegistryPublisher::loopback_test(address).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), 1024 * 1024 * 1024).unwrap();
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime-test/build-vm/verified-artifact.tar"),
    )
    .unwrap();
    let artifact = store.put("987661", "987662", 1, &bytes).unwrap();
    let guard = Guard {
        allowed: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
    };
    let image = publisher
        .publish("987661", "987663", "987664", &store, &artifact, &guard)
        .await
        .unwrap();
    assert_eq!(
        image,
        format!(
            "{address}/customers/987661/projects/987663@{}",
            artifact.image_digest()
        )
    );
    assert!(guard.calls.load(Ordering::SeqCst) > 5);
    let loaded = store
        .load(
            artifact.storage_key().into(),
            artifact.digest().into(),
            artifact.size(),
            artifact.image_digest().into(),
        )
        .unwrap();
    assert_eq!(
        publisher
            .publish("987661", "987663", "987664", &store, &loaded, &guard)
            .await
            .unwrap(),
        image
    );
    guard.allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        publisher
            .publish("987661", "987663", "987665", &store, &artifact, &guard)
            .await
            .unwrap_err()
            .code,
        "lease_revoked"
    );
    assert!(
        publisher
            .publish("987666", "987663", "987665", &store, &artifact, &guard)
            .await
            .is_err()
    );
    drop(registry);
}
