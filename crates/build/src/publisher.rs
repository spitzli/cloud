//! OCI publishing through the maintained OCI Distribution client. No provider SDK.
use crate::{
    Result,
    artifact::{Archive, ArtifactStore, MAX_OUTPUT, StoredArtifact},
    error,
};
use async_trait::async_trait;
pub use oci_client::secrets::RegistryAuth as RegistryCredential;
use oci_client::{
    Client, Reference,
    client::{ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
    token_cache::RegistryOperation,
};
use serde_json::Value;
use std::{collections::BTreeSet, time::Duration};
#[async_trait]
pub trait PublicationGuard: Send + Sync {
    async fn check(&self) -> Result<()>;
}
pub struct RegistryPublisher {
    registry: String,
    client: Client,
    auth: RegistryAuth,
}
impl RegistryPublisher {
    pub fn new(registry: String, auth: RegistryAuth) -> Result<Self> {
        Self::configured(registry, auth, ClientProtocol::Https)
    }
    fn configured(registry: String, auth: RegistryAuth, protocol: ClientProtocol) -> Result<Self> {
        let reference: Reference = format!("{registry}/validation:bound")
            .parse()
            .map_err(|_| error("invalid_registry"))?;
        if reference.registry() != registry || registry.contains('/') || registry == "docker.io" {
            return Err(error("invalid_registry"));
        }
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = Client::try_from(ClientConfig {
            protocol,
            read_timeout: Some(Duration::from_secs(30)),
            connect_timeout: Some(Duration::from_secs(10)),
            max_concurrent_upload: 1,
            max_concurrent_download: 1,
            ..Default::default()
        })
        .map_err(|_| error("registry_configuration"))?;
        Ok(Self {
            registry,
            client,
            auth,
        })
    }
    #[cfg(feature = "local-registry-test")]
    pub fn loopback_test(address: std::net::SocketAddr) -> Result<Self> {
        Self::loopback_test_authenticated(address, RegistryAuth::Anonymous)
    }
    #[cfg(feature = "local-registry-test")]
    pub fn loopback_test_authenticated(
        address: std::net::SocketAddr,
        auth: RegistryAuth,
    ) -> Result<Self> {
        if !address.ip().is_loopback() {
            return Err(error("loopback_required"));
        }
        Self::configured(address.to_string(), auth, ClientProtocol::Http)
    }
    /// The repository is derived exclusively from admitted customer/project IDs. Every remote write
    /// is guarded; a failure after prepare is ambiguous and must enter durable reconciliation.
    pub async fn publish(
        &self,
        customer: &str,
        project: &str,
        release: &str,
        store: &ArtifactStore,
        artifact: &StoredArtifact,
        guard: &dyn PublicationGuard,
    ) -> Result<String> {
        crate::contracts::id(customer)?;
        crate::contracts::id(project)?;
        crate::contracts::id(release)?;
        if artifact.storage_key().split('/').next() != Some(customer) {
            return Err(error("artifact_scope_mismatch"));
        }
        let bytes = store.read(artifact)?;
        let outer = Archive::read(&bytes, MAX_OUTPUT)?;
        if let Some(bytes) = outer.files().get("provenance.json") {
            let provenance: crate::evidence::Provenance =
                serde_json::from_slice(bytes).map_err(|_| error("invalid_artifact_evidence"))?;
            if provenance.build.project_id != project {
                return Err(error("artifact_evidence_scope_mismatch"));
            }
        }
        let image = Archive::read(
            outer
                .files()
                .get("image.tar")
                .ok_or_else(|| error("artifact_missing"))?,
            MAX_OUTPUT,
        )?;
        let index: Value = serde_json::from_slice(
            image
                .files()
                .get("index.json")
                .ok_or_else(|| error("invalid_oci"))?,
        )
        .map_err(|_| error("invalid_oci"))?;
        let root = index["manifests"][0].clone();
        let repository = format!("{}/customers/{customer}/projects/{project}", self.registry);
        let reference: Reference = format!("{repository}:release-{release}")
            .parse()
            .map_err(|_| error("invalid_repository"))?;
        guard.check().await?;
        tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .auth(&reference, &self.auth, RegistryOperation::Push),
        )
        .await
        .map_err(|_| error("publication_uncertain"))?
        .map_err(|_| error("publication_uncertain"))?;
        let mut pending = vec![(root, false)];
        let mut done = BTreeSet::new();
        while let Some((descriptor, expanded)) = pending.pop() {
            let digest = descriptor["digest"]
                .as_str()
                .ok_or_else(|| error("invalid_oci"))?;
            if done.contains(digest) {
                continue;
            }
            let hash = digest
                .strip_prefix("sha256:")
                .ok_or_else(|| error("invalid_oci"))?;
            let data = image
                .files()
                .get(&format!("blobs/sha256/{hash}"))
                .ok_or_else(|| error("oci_blob_missing"))?;
            let media = descriptor["mediaType"]
                .as_str()
                .ok_or_else(|| error("invalid_oci"))?;
            let manifest =
                media.contains("manifest") || media == "application/vnd.oci.image.index.v1+json";
            if manifest && !expanded {
                let v: Value = serde_json::from_slice(data).map_err(|_| error("invalid_oci"))?;
                pending.push((descriptor.clone(), true));
                if let Some(children) = v["manifests"].as_array() {
                    for child in children {
                        pending.push((child.clone(), false));
                    }
                } else {
                    pending.push((v["config"].clone(), false));
                    for layer in v["layers"].as_array().ok_or_else(|| error("invalid_oci"))? {
                        pending.push((layer.clone(), false));
                    }
                }
                continue;
            }
            guard.check().await?;
            if manifest {
                let target: Reference = if digest == artifact.image_digest() {
                    reference.clone()
                } else {
                    format!("{repository}@{digest}")
                        .parse()
                        .map_err(|_| error("invalid_repository"))?
                };
                let content_type = media.parse().map_err(|_| error("invalid_oci"))?;
                tokio::time::timeout(
                    Duration::from_secs(60),
                    self.client
                        .push_manifest_raw(&target, data.clone(), content_type),
                )
                .await
                .map_err(|_| error("publication_uncertain"))?
                .map_err(|_| error("publication_uncertain"))?;
            } else {
                tokio::time::timeout(
                    Duration::from_secs(60),
                    self.client.push_blob(&reference, data.clone(), digest),
                )
                .await
                .map_err(|_| error("publication_uncertain"))?
                .map_err(|_| error("publication_uncertain"))?;
            }
            guard.check().await?;
            done.insert(digest.to_owned());
        }
        let remote = tokio::time::timeout(
            Duration::from_secs(30),
            self.client.fetch_manifest_digest(&reference, &self.auth),
        )
        .await
        .map_err(|_| error("publication_uncertain"))?
        .map_err(|_| error("publication_uncertain"))?;
        if remote != artifact.image_digest() {
            return Err(error("publication_digest_mismatch"));
        }
        guard.check().await?;
        Ok(format!("{repository}@{remote}"))
    }
}
