#![cfg(feature = "local-registry-test")]
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, Ordering},
};
use webdock_build::{
    artifact::{Archive, ArtifactStore, MAX_OUTPUT},
    publisher::{PublicationGuard, RegistryCredential, RegistryPublisher},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    address: std::net::SocketAddr,
    username: String,
    password: String,
    container_id: String,
    run_id: String,
}
fn fixture() -> Fixture {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let path = std::path::PathBuf::from(
        std::env::var_os("WEBDOCK_REGISTRY_S3_TEST_CONFIG")
            .expect("explicit private registry fixture"),
    );
    let metadata = path.symlink_metadata().unwrap();
    use std::os::unix::fs::MetadataExt;
    assert!(metadata.is_file() && metadata.nlink() == 1 && metadata.mode() & 0o077 == 0);
    let result: Fixture = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(result.address.ip().is_loopback());
    result
}
fn source() -> (
    tempfile::TempDir,
    ArtifactStore,
    webdock_build::artifact::StoredArtifact,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = ArtifactStore::open(dir.path().into(), MAX_OUTPUT).unwrap();
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.runtime-test/build-vm/verified-artifact.tar"),
    )
    .unwrap();
    let artifact = store.put("100", "200", 1, &bytes).unwrap();
    (dir, store, artifact)
}
fn newer_artifact(store: &ArtifactStore) -> webdock_build::artifact::StoredArtifact {
    use serde_json::json;
    use std::collections::BTreeMap;
    let config =
        br#"{"architecture":"amd64","os":"linux","rootfs":{"type":"layers","diff_ids":[]}}"#;
    let config_hash = hex::encode(Sha256::digest(config));
    let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":config.len(),"digest":format!("sha256:{config_hash}")},"layers":[]})).unwrap();
    let hash = hex::encode(Sha256::digest(&manifest));
    let index = serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{hash}")}]})).unwrap();
    let image = Archive::from_files(BTreeMap::from([
        (
            "oci-layout".into(),
            br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
        ),
        ("index.json".into(), index),
        (format!("blobs/sha256/{hash}"), manifest),
        (format!("blobs/sha256/{config_hash}"), config.to_vec()),
    ]))
    .unwrap()
    .encode()
    .unwrap();
    let bytes = Archive::from_files(BTreeMap::from([("image.tar".into(), image)]))
        .unwrap()
        .encode()
        .unwrap();
    store.put("100", "201", 1, &bytes).unwrap()
}
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
async fn verify_pull(
    config: &Fixture,
    store: &ArtifactStore,
    artifact: &webdock_build::artifact::StoredArtifact,
    newer: &webdock_build::artifact::StoredArtifact,
) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{}/v2/customers/100/projects/300", config.address);
    let path = format!("{base}/manifests/{}", artifact.image_digest());
    assert_eq!(
        client.get(&path).send().await.unwrap().status().as_u16(),
        401
    );
    assert_eq!(
        client
            .get(&path)
            .basic_auth(&config.username, Some("wrong-fixture-password"))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    for (tag, expected) in [("release-400", artifact), ("release-500", newer)] {
        let response = client.get(format!("{base}/manifests/{tag}")).header("Accept", "application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json").basic_auth(&config.username, Some(&config.password)).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.headers().get("location").is_none());
        let bytes = response.bytes().await.unwrap();
        assert_eq!(
            format!("sha256:{}", hex::encode(Sha256::digest(&bytes))),
            expected.image_digest()
        );
    }
    for artifact in [artifact, newer] {
        let outer = Archive::read(&store.read(artifact).unwrap(), MAX_OUTPUT).unwrap();
        let image = Archive::read(&outer.files()["image.tar"], MAX_OUTPUT).unwrap();
        for (path, expected) in image.files() {
            let Some(hash) = path.strip_prefix("blobs/sha256/") else {
                continue;
            };
            let json = serde_json::from_slice::<serde_json::Value>(expected).ok();
            let manifest = json.as_ref().is_some_and(|v| {
                v["schemaVersion"] == 2 && (v["layers"].is_array() || v["manifests"].is_array())
            });
            let kind = if manifest { "manifests" } else { "blobs" };
            let response = client.get(format!("{base}/{kind}/sha256:{hash}")).header("Accept", "application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json").basic_auth(&config.username, Some(&config.password)).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 200);
            assert!(response.headers().get("location").is_none());
            assert_eq!(
                response.bytes().await.unwrap().as_ref(),
                expected.as_slice()
            );
        }
    }
    let denied = client
        .delete(format!("{base}/manifests/{}", artifact.image_digest()))
        .basic_auth(&config.username, Some(&config.password))
        .send()
        .await
        .unwrap();
    assert_eq!(
        denied.status().as_u16(),
        405,
        "client deletion must remain disabled"
    );
}
fn restart(config: &Fixture) {
    let inspect = std::process::Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.Id}} {{index .Config.Labels \"webdock.registry-storage\"}}",
            &config.container_id,
        ])
        .output()
        .unwrap();
    assert!(inspect.status.success());
    assert_eq!(
        String::from_utf8(inspect.stdout).unwrap().trim(),
        format!("{} {}", config.container_id, config.run_id)
    );
    assert!(
        std::process::Command::new("docker")
            .args(["restart", "--timeout", "5", &config.container_id])
            .output()
            .unwrap()
            .status
            .success()
    );
}
async fn wait_ready(config: &Fixture) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .unwrap();
    for _ in 0..100 {
        if client
            .get(format!("http://{}/v2/", config.address))
            .send()
            .await
            .is_ok_and(|r| matches!(r.status().as_u16(), 401 | 503))
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("owned registry did not become reachable");
}

#[tokio::test]
#[ignore = "owned authenticated S3 registry; run tools/tests/registry-storage.py"]
async fn registry_s3_publish_pull_and_restart() {
    let config = fixture();
    wait_ready(&config).await;
    let (_directory, store, artifact) = source();
    let newer = newer_artifact(&store);
    assert_ne!(artifact.image_digest(), newer.image_digest());
    let publisher = RegistryPublisher::loopback_test_authenticated(
        config.address,
        RegistryCredential::Basic(config.username.clone(), config.password.clone()),
    )
    .unwrap();
    let guard = Guard(AtomicBool::new(true));
    for (release, artifact) in [("400", &artifact), ("500", &newer)] {
        let image = publisher
            .publish("100", "300", release, &store, artifact, &guard)
            .await
            .unwrap();
        assert!(image.ends_with(artifact.image_digest()));
    }
    verify_pull(&config, &store, &artifact, &newer).await;
    restart(&config);
    wait_ready(&config).await;
    verify_pull(&config, &store, &artifact, &newer).await;
    guard.0.store(false, Ordering::SeqCst);
    assert!(
        publisher
            .publish("100", "300", "600", &store, &artifact, &guard)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "owned S3 user explicitly disabled by registry-storage.py"]
async fn registry_s3_backend_revocation_fails_closed() {
    let config = fixture();
    restart(&config);
    wait_ready(&config).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .get(format!(
            "http://{}/v2/customers/100/projects/300/manifests/release-400",
            config.address
        ))
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .basic_auth(config.username, Some(config.password))
        .send()
        .await
        .unwrap();
    assert!(!response.status().is_success());
}

#[tokio::test]
#[ignore = "owned S3 user re-enabled by registry-storage.py"]
async fn registry_s3_backend_recovery_preserves_releases() {
    let config = fixture();
    restart(&config);
    wait_ready(&config).await;
    let (_directory, store, artifact) = source();
    let newer = newer_artifact(&store);
    verify_pull(&config, &store, &artifact, &newer).await;
}
