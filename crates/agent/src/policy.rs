//! Reviewed artifacts and resource-scoped mounts. No request supplies a host path.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use webdock_plugin_protocol::Manifest;
use webdock_supervisor::*;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePolicyConfig {
    #[serde(default)]
    pub artifact_directory: Option<PathBuf>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub mount_root: Option<PathBuf>,
    #[serde(default)]
    pub runtime_uid: Option<u32>,
    #[serde(default)]
    pub grants: Vec<MountGrant>,
    #[serde(default)]
    pub registry_config: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MountGrant {
    pub name: String,
    pub revision: u64,
    pub installation: String,
    pub project_id: String,
    pub resource_id: String,
    pub kind: GrantKind,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GrantKind {
    SocketDirectory {},
    Secret { sha256: String },
}
#[derive(Clone)]
pub struct ResolvedMount {
    pub source: PathBuf,
    pub target: String,
    pub read_only: bool,
}
#[derive(Clone, Default)]
pub struct RuntimePolicy {
    config: RuntimePolicyConfig,
    artifacts: BTreeMap<String, Manifest>,
}
impl RuntimePolicy {
    pub fn registry(&self) -> Result<Option<crate::registry::RegistryConfig>> {
        self.config
            .registry_config
            .as_ref()
            .map(|path| crate::registry::RegistryConfig::load(path))
            .transpose()
    }
    pub fn load(config: RuntimePolicyConfig) -> Result<Self> {
        if let Some(path) = &config.registry_config {
            crate::registry::RegistryConfig::load(path)?;
        }
        if config.artifacts.len() > 32 || config.grants.len() > 256 {
            return Err(ExecutionError::Invalid("runtime policy limit"));
        }
        let mut artifacts = BTreeMap::new();
        for hash in &config.artifacts {
            if !valid_sha256(hash) || artifacts.contains_key(hash) {
                return Err(ExecutionError::Invalid("artifact digest"));
            }
            let directory = config
                .artifact_directory
                .as_ref()
                .ok_or(ExecutionError::Denied)?;
            crate::config::private_directory(directory).map_err(|_| ExecutionError::Denied)?;
            let bytes =
                crate::config::read_private(&directory.join(format!("{hash}.json")), 262144)
                    .map_err(|_| ExecutionError::Denied)?;
            if format!("{:x}", Sha256::digest(&bytes)) != *hash {
                return Err(ExecutionError::Ownership);
            }
            artifacts.insert(
                hash.clone(),
                Manifest::parse(&bytes)
                    .map_err(|_| ExecutionError::Invalid("reviewed descriptor"))?,
            );
        }
        let mut keys = std::collections::HashSet::new();
        for grant in &config.grants {
            if !valid_mount_name(&grant.name)
                || grant.revision == 0
                || grant.installation.is_empty()
                || grant.installation.len() > 24
                || !grant
                    .installation
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || webdock_core::id::RecordId::parse(&grant.project_id).is_err()
                || webdock_core::id::RecordId::parse(&grant.resource_id).is_err()
                || !keys.insert((
                    &grant.installation,
                    &grant.project_id,
                    &grant.resource_id,
                    &grant.name,
                ))
            {
                return Err(ExecutionError::Invalid("mount grant"));
            }
            if let GrantKind::Secret { sha256 } = &grant.kind
                && !valid_sha256(sha256)
            {
                return Err(ExecutionError::Invalid("secret integrity digest"));
            }
        }
        Ok(Self { config, artifacts })
    }
    pub fn admit(&self, spec: &RuntimeSpec) -> Result<()> {
        if let Some(registry) = self.registry()? {
            registry.select(spec.project_id.as_str(), &spec.image)?;
        }
        spec.validate()?;
        if let Some(binding) = &spec.plugin {
            let manifest = self
                .artifacts
                .get(&binding.manifest_sha256)
                .ok_or(ExecutionError::Denied)?;
            if manifest.id() != binding.plugin_id
                || manifest.version() != binding.version
                || manifest.as_json()["image"] != spec.image
            {
                return Err(ExecutionError::Ownership);
            }
            manifest
                .require_action_only_lifecycle()
                .map_err(|_| ExecutionError::Unsupported("plugin lifecycle"))?;
        }
        for reference in &spec.mounts {
            self.grant(spec, reference)?;
        }
        Ok(())
    }
    pub fn grant<'a>(
        &'a self,
        spec: &RuntimeSpec,
        reference: &MountReference,
    ) -> Result<&'a MountGrant> {
        let grant = self
            .config
            .grants
            .iter()
            .find(|g| {
                g.installation == spec.installation
                    && g.project_id == spec.project_id.as_str()
                    && g.resource_id == spec.resource_id.as_str()
                    && g.name == reference.name
                    && g.revision == reference.revision
            })
            .ok_or(ExecutionError::Denied)?;
        if let (Some(binding), GrantKind::Secret { .. }) = (&spec.plugin, &grant.kind) {
            let manifest = self
                .artifacts
                .get(&binding.manifest_sha256)
                .ok_or(ExecutionError::Denied)?
                .as_json();
            if !manifest["secrets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == &grant.name)
                || !manifest["capabilities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == "plugin.secrets.read")
            {
                return Err(ExecutionError::Denied);
            }
        }
        Ok(grant)
    }
    pub fn uid(&self, spec: &RuntimeSpec) -> Result<u32> {
        if spec.mounts.is_empty() {
            return Ok(65532);
        }
        self.config
            .runtime_uid
            .filter(|uid| *uid > 0 && i32::try_from(*uid).is_ok())
            .ok_or(ExecutionError::Denied)
    }
    pub fn local_mounts(&self, spec: &RuntimeSpec) -> Result<Vec<ResolvedMount>> {
        self.admit(spec)?;
        let mut result = Vec::new();
        for reference in &spec.mounts {
            let grant = self.grant(spec, reference)?;
            let uid = self.uid(spec)?;
            let root = self
                .config
                .mount_root
                .as_ref()
                .ok_or(ExecutionError::Denied)?;
            private_owned_directory(root, uid)?;
            let project = root
                .join(&spec.installation)
                .join(spec.project_id.as_str())
                .join(spec.resource_id.as_str());
            for path in [
                root.join(&spec.installation),
                root.join(&spec.installation).join(spec.project_id.as_str()),
                project.clone(),
            ] {
                private_owned_directory(&path, uid)?;
            }
            match &grant.kind {
                GrantKind::SocketDirectory {} => {
                    let parent = project.join("sockets");
                    let source = parent.join(&grant.name);
                    private_owned_directory(&parent, uid)?;
                    private_owned_directory(&source, uid)?;
                    let mut count = 0;
                    for entry in std::fs::read_dir(&source).map_err(|_| ExecutionError::Denied)? {
                        let entry = entry.map_err(|_| ExecutionError::Denied)?;
                        let meta = std::fs::symlink_metadata(entry.path())
                            .map_err(|_| ExecutionError::Denied)?;
                        count += 1;
                        if count > 16
                            || !meta.file_type().is_socket()
                            || meta.uid() != uid
                            || meta.mode() & 0o077 != 0
                        {
                            return Err(ExecutionError::Denied);
                        }
                    }
                    result.push(ResolvedMount {
                        source,
                        target: format!("/run/webdock/{}", grant.name),
                        read_only: false,
                    });
                }
                GrantKind::Secret { sha256 } => {
                    let parent = project.join("secrets");
                    private_owned_directory(&parent, uid)?;
                    let source = parent.join(&grant.name);
                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                        .open(&source)
                        .map_err(|_| ExecutionError::Denied)?;
                    let meta = file.metadata().map_err(|_| ExecutionError::Denied)?;
                    if !meta.is_file()
                        || meta.nlink() != 1
                        || meta.uid() != uid
                        || meta.mode() & 0o777 != 0o600
                        || meta.len() > 65536
                        || std::fs::canonicalize(&source).map_err(|_| ExecutionError::Denied)?
                            != source
                    {
                        return Err(ExecutionError::Denied);
                    }
                    let mut bytes = Vec::new();
                    file.take(65537)
                        .read_to_end(&mut bytes)
                        .map_err(|_| ExecutionError::Denied)?;
                    if bytes.len() > 65536 || format!("{:x}", Sha256::digest(&bytes)) != *sha256 {
                        return Err(ExecutionError::Ownership);
                    }
                    result.push(ResolvedMount {
                        source,
                        target: format!("/run/secrets/{}", grant.name),
                        read_only: true,
                    });
                }
            }
        }
        Ok(result)
    }
    pub fn kubernetes_secret_name(spec: &RuntimeSpec, name: &str) -> String {
        format!("wd-secret-{}-{name}", spec.resource_id)
    }
}
fn private_owned_directory(path: &Path, uid: u32) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ExecutionError::Denied)?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || std::fs::canonicalize(path).map_err(|_| ExecutionError::Denied)? != path
    {
        return Err(ExecutionError::Denied);
    }
    Ok(())
}
