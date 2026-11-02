//! Trusted worker transfers. S3 receipts do not replace Core completion or release authority.
use crate::{
    Result,
    artifact::{ArtifactStore, MAX_OUTPUT, StoredArtifact},
    error, io,
};
use std::io::Read;
use webdock_object_storage::Store;

#[derive(Clone)]
pub struct RemoteArtifacts {
    store: Store,
}
impl RemoteArtifacts {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    pub fn binding(&self) -> &str {
        self.store.binding()
    }
    pub fn id(&self) -> &str {
        self.store.id()
    }
    fn scope(artifact: &StoredArtifact) -> Result<String> {
        let parts: Vec<_> = artifact.key.split('/').collect();
        if parts.len() != 4 || artifact.size == 0 || artifact.size > MAX_OUTPUT {
            return Err(error("invalid_artifact"));
        }
        for part in &parts[..3] {
            crate::contracts::id(part)?;
        }
        artifact.format_version()?;
        Ok(parts[..3].join("/"))
    }
    pub async fn persist(
        &self,
        local: &ArtifactStore,
        artifact: &StoredArtifact,
    ) -> Result<StoredArtifact> {
        let scope = Self::scope(artifact)?;
        let bytes = local.read(artifact)?;
        let remote = self
            .store
            .upload(&scope, bytes.as_slice(), MAX_OUTPUT)
            .await
            .map_err(|_| error("artifact_upload_failed"))?;
        let mut result = artifact.clone();
        result.remote = Some(remote);
        self.validate(&result)?;
        Ok(result)
    }
    fn validate(&self, artifact: &StoredArtifact) -> Result<String> {
        let scope = Self::scope(artifact)?;
        let receipt = artifact
            .remote
            .as_ref()
            .ok_or_else(|| error("artifact_backend_mismatch"))?;
        self.store
            .validate_receipt(&scope, receipt, MAX_OUTPUT)
            .map_err(|_| error("artifact_backend_mismatch"))?;
        if artifact.digest != format!("sha256:{}", receipt.sha256)
            || artifact.size != receipt.size_bytes
        {
            return Err(error("artifact_digest_mismatch"));
        }
        Ok(scope)
    }
    /// Download only to fresh private staging; validate the full OCI graph before local publication.
    pub async fn restore(
        &self,
        local: &ArtifactStore,
        artifact: &StoredArtifact,
    ) -> Result<StoredArtifact> {
        let scope = self.validate(artifact)?;
        let mut staging = tempfile::tempfile().map_err(io)?;
        let mut target = tokio::fs::File::from_std(staging.try_clone().map_err(io)?);
        self.store
            .download(
                &scope,
                artifact.remote.as_ref().unwrap(),
                &mut target,
                MAX_OUTPUT,
            )
            .await
            .map_err(|_| error("artifact_download_failed"))?;
        target.sync_all().await.map_err(io)?;
        drop(target);
        use std::io::Seek;
        staging.rewind().map_err(io)?;
        let mut bytes = Vec::new();
        staging
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        let parts: Vec<_> = scope.split('/').collect();
        let generation = parts[2]
            .parse::<i32>()
            .map_err(|_| error("invalid_generation"))?;
        let mut restored = local.put(parts[0], parts[1], generation, &bytes)?;
        if restored.key != artifact.key
            || restored.digest != artifact.digest
            || restored.size != artifact.size
            || restored.image_digest != artifact.image_digest
        {
            return Err(error("artifact_digest_mismatch"));
        }
        restored.remote = artifact.remote.clone();
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact::Archive,
        evidence::{BuildEvidence, MachineIdentity},
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    fn legacy() -> Vec<u8> {
        let config = b"{}";
        let hash = hex::encode(Sha256::digest(config));
        let manifest = serde_json::to_vec(&json!({"schemaVersion":2,"config":{"mediaType":"application/vnd.oci.image.config.v1+json","size":2,"digest":format!("sha256:{hash}")},"layers":[]})).unwrap();
        let manifest_hash = hex::encode(Sha256::digest(&manifest));
        let index = serde_json::to_vec(&json!({"schemaVersion":2,"manifests":[{"mediaType":"application/vnd.oci.image.manifest.v1+json","size":manifest.len(),"digest":format!("sha256:{manifest_hash}")}]})).unwrap();
        let image = Archive::from_files(BTreeMap::from([
            (
                "oci-layout".into(),
                br#"{"imageLayoutVersion":"1.0.0"}"#.to_vec(),
            ),
            ("index.json".into(), index),
            (format!("blobs/sha256/{hash}"), config.to_vec()),
            (format!("blobs/sha256/{manifest_hash}"), manifest),
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
    async fn restore_rejects_valid_legacy_and_e1_content_relabelled_by_suffix() {
        let source = Archive::from_files(BTreeMap::from([(
            "Dockerfile".into(),
            b"FROM scratch\n".to_vec(),
        )]))
        .unwrap()
        .encode()
        .unwrap();
        let e1 = crate::evidence::assemble(
            &legacy(),
            &source,
            &BuildEvidence {
                customer_id: "100".into(),
                project_id: "300".into(),
                build_id: "200".into(),
                generation: 1,
                source_commit: "a".repeat(40),
                recipe: "dockerfile".into(),
                root_directory: ".".into(),
                machine: MachineIdentity {
                    image_sha256: "b".repeat(64),
                    kernel_sha256: None,
                    initrd_sha256: None,
                },
            },
            None,
        )
        .unwrap();
        let remote = RemoteArtifacts::new(Store::memory("suffix-test", "builds").unwrap());
        for (bytes, suffix) in [(legacy(), "e1.tar"), (e1, "tar")] {
            let directory = tempfile::tempdir().unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let local = ArtifactStore::open(directory.path().into(), MAX_OUTPUT).unwrap();
            let artifact = local.put("100", "200", 1, &bytes).unwrap();
            let mut remote_artifact = remote.persist(&local, &artifact).await.unwrap();
            remote_artifact.key =
                format!("100/200/1/{}.{suffix}", hex::encode(Sha256::digest(&bytes)));
            assert!(
                matches!(remote.restore(&local, &remote_artifact).await, Err(e) if e.code == "artifact_digest_mismatch")
            );
        }
    }
}
