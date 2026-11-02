#![cfg(feature = "worker")]
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};
use webdock_build::{
    artifact::{Archive, ArtifactStore, MAX_OUTPUT},
    remote::RemoteArtifacts,
};
use webdock_object_storage::Store;

fn store() -> (tempfile::TempDir, ArtifactStore) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(directory.path().into(), MAX_OUTPUT * 2).unwrap();
    (directory, store)
}
fn artifact() -> Vec<u8> {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let config = b"{}".to_vec();
    let hash = hex::encode(Sha256::digest(&config));
    let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{hash}")},"layers":[]})).unwrap();
    let digest = hex::encode(Sha256::digest(&manifest));
    let index = serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{digest}")}]})).unwrap();
    let image = Archive::from_files(BTreeMap::from([
        (
            "oci-layout".into(),
            br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
        ),
        ("index.json".into(), index),
        (format!("blobs/sha256/{hash}"), config),
        (format!("blobs/sha256/{digest}"), manifest),
    ]))
    .unwrap()
    .encode()
    .unwrap();
    Archive::from_files(BTreeMap::from([("image.tar".into(), image)]))
        .unwrap()
        .encode()
        .unwrap()
}

#[tokio::test]
async fn another_worker_recovers_exact_oci_bytes_without_original_local_store() {
    let remote = RemoteArtifacts::new(Store::memory("build-v1", "builds").unwrap());
    let (first_directory, first) = store();
    let bytes = artifact();
    let local = first.put("100", "200", 1, &bytes).unwrap();
    let saved = remote.persist(&first, &local).await.unwrap();
    assert!(saved.remote_storage().is_some());
    let receipt = serde_json::to_vec(saved.remote_storage().unwrap()).unwrap();
    drop(first);
    drop(first_directory);
    let (_directory, second) = store();
    assert!(second.read(&saved).is_err());
    let restored = remote.restore(&second, &saved).await.unwrap();
    assert_eq!(second.read(&restored).unwrap(), bytes);
    assert_eq!(restored.image_digest(), saved.image_digest());
    assert_eq!(
        serde_json::to_vec(restored.remote_storage().unwrap()).unwrap(),
        receipt
    );
    let wrong = RemoteArtifacts::new(Store::memory("other-backend", "builds").unwrap());
    assert!(wrong.restore(&second, &saved).await.is_err());
    assert!(remote.restore(&second, &local).await.is_err());
}

#[tokio::test]
async fn enabling_remote_storage_does_not_migrate_or_remove_local_artifacts() {
    let (_directory, local) = store();
    let bytes = artifact();
    let saved = local.put("100", "200", 1, &bytes).unwrap();
    let _remote = RemoteArtifacts::new(Store::memory("build-v1", "builds").unwrap());
    let recovered = local
        .load(
            saved.storage_key().into(),
            saved.digest().into(),
            saved.size(),
            saved.image_digest().into(),
        )
        .unwrap();
    assert!(recovered.remote_storage().is_none());
    assert_eq!(local.read(&recovered).unwrap(), bytes);
}
