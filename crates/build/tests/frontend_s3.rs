#![cfg(feature = "local-registry-test")]
//! Private consumer/OCI reuse, without bootstrapping Core or changing shared SQL.
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, Ordering},
};
use webdock_build::{
    artifact::{Archive, ArtifactStore, MAX_OUTPUT},
    publisher::{PublicationGuard, RegistryPublisher},
    remote::RemoteArtifacts,
};
#[path = "support/registry.rs"]
mod registry;
struct Guard(AtomicBool);
#[async_trait::async_trait]
impl PublicationGuard for Guard {
    async fn check(&self) -> webdock_build::Result<()> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(webdock_build::Error {
                code: "lease_revoked",
                logs: String::new(),
            })
        }
    }
}
fn local() -> (tempfile::TempDir, ArtifactStore) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), MAX_OUTPUT * 2).unwrap();
    (dir, store)
}
#[tokio::test]
#[ignore = "explicit owned MinIO bucket/user, new real frontend VM output and temporary registry"]
async fn retained_frontend_survives_original_worker_loss_and_reuses_original_oci() {
    #[derive(serde::Deserialize)]
    struct Fixture {
        config: webdock_object_storage::S3Config,
        credentials: webdock_object_storage::Credentials,
    }
    let config_path = std::path::PathBuf::from(
        std::env::var_os("WEBDOCK_S3_TEST_CONFIG").expect("owned private S3 config"),
    );
    let metadata = config_path.symlink_metadata().unwrap();
    assert!(metadata.is_file() && metadata.permissions().mode() & 0o077 == 0);
    let config: Fixture = serde_json::from_slice(&std::fs::read(config_path).unwrap()).unwrap();
    let remote = RemoteArtifacts::new(
        webdock_object_storage::Store::loopback_test(config.config, config.credentials).unwrap(),
    );
    let vm = std::path::PathBuf::from(
        std::env::var_os("WEBDOCK_FRONTEND_VM_ROOT").expect("owned VM output"),
    );
    let bytes = std::fs::read(vm.join("verified-frontend-e2.tar")).unwrap();
    let envelope = Archive::read(&bytes, MAX_OUTPUT).unwrap();
    let evidence = webdock_build::evidence::validate(&envelope, "100", "200", 1, None)
        .unwrap()
        .unwrap();
    assert_eq!(evidence.version, 2);
    let (producer_directory, producer) = local();
    let saved = producer.put("100", "200", 1, &bytes).unwrap();
    let saved = remote.persist(&producer, &saved).await.unwrap();
    assert_eq!(
        saved.remote_storage().unwrap().sha256,
        saved.digest().strip_prefix("sha256:").unwrap()
    );
    drop(producer);
    drop(producer_directory);
    let (_consumer_directory, consumer) = local();
    assert!(consumer.read(&saved).is_err());
    let restored = remote.restore(&consumer, &saved).await.unwrap();
    assert_eq!(consumer.read(&restored).unwrap(), bytes);
    let output =
        webdock_build::frontend::retained(&consumer, &restored, &evidence.build, None).unwrap();
    assert_eq!(output.files()["index.html"], b"<p>owned frontend</p>");
    assert_eq!(output.files()["assets/main.js"], b"console.log(42)");
    let mut wrong = evidence.build.clone();
    wrong.project_id = "301".into();
    assert!(webdock_build::frontend::retained(&consumer, &restored, &wrong, None).is_err());
    let mut corrupted = envelope.files().clone();
    corrupted.get_mut("frontend.tar").unwrap()[512] ^= 1;
    let corrupted = Archive::from_files(corrupted).unwrap().encode().unwrap();
    assert!(consumer.put("100", "200", 1, &corrupted).is_err());

    let registry = registry::LocalRegistry::start().await;
    let publisher = RegistryPublisher::loopback_test(registry.address).unwrap();
    let guard = Guard(AtomicBool::new(true));
    let first = publisher
        .publish("100", "300", "400", &consumer, &restored, &guard)
        .await
        .unwrap();
    let image = Archive::read(&envelope.files()["image.tar"], MAX_OUTPUT).unwrap();
    let index: serde_json::Value = serde_json::from_slice(&image.files()["index.json"]).unwrap();
    let hash = index["manifests"][0]["digest"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap();
    let original_manifest = &image.files()[&format!("blobs/sha256/{hash}")];
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let pull = || {
        client
            .get(format!(
                "http://{}/v2/customers/100/projects/300/manifests/{}",
                registry.address,
                restored.image_digest()
            ))
            .header("Accept", "application/vnd.oci.image.manifest.v1+json")
    };
    assert_eq!(
        pull()
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        original_manifest
    );
    for (name, expected) in image
        .files()
        .iter()
        .filter(|(name, _)| name.starts_with("blobs/sha256/"))
    {
        let digest = format!("sha256:{}", name.strip_prefix("blobs/sha256/").unwrap());
        // Manifest blobs use the manifest route; regular config/layer blobs use the blob route.
        if name == &format!("blobs/sha256/{hash}") {
            continue;
        }
        let actual = client
            .get(format!(
                "http://{}/v2/customers/100/projects/300/blobs/{digest}",
                registry.address
            ))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(actual.as_ref(), expected);
    }
    let rollback = publisher
        .publish("100", "300", "401", &consumer, &restored, &guard)
        .await
        .unwrap();
    assert_eq!(first, rollback);
    assert_eq!(
        pull()
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        original_manifest
    );
    guard.0.store(false, Ordering::SeqCst);
    assert_eq!(
        publisher
            .publish("100", "300", "402", &consumer, &restored, &guard)
            .await
            .unwrap_err()
            .code,
        "lease_revoked"
    );
    assert!(
        publisher
            .publish("101", "300", "402", &consumer, &restored, &guard)
            .await
            .is_err()
    );
    assert!(
        publisher
            .publish("100", "301", "402", &consumer, &restored, &guard)
            .await
            .is_err()
    );
    println!(
        "Verified fresh MinIO retention, original local store absent, compatible consumer hydration, exact HTML/JS, unchanged OCI push/pull/republication and private authority denials."
    );
}
