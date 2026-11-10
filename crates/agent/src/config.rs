use crate::management::{AgentError, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConfig {
    pub version: u8,
    pub installation: String,
    pub agent_id: String,
    pub generation: u64,
    pub state_directory: PathBuf,
    pub control: ControlEndpoint,
    pub runtime: RuntimeConfiguration,
    #[serde(default)]
    pub database_runtime: Option<PathBuf>,
    #[serde(default)]
    pub mail_runtime: Option<PathBuf>,
    #[serde(default)]
    pub recovery_runtime: Option<PathBuf>,
    #[serde(default)]
    pub mail_configuration_enabled: bool,
    #[serde(default)]
    pub policy: crate::policy::RuntimePolicyConfig,
    /// Apply plugin workloads Core sends for this cluster (Kubernetes runtimes only).
    #[serde(default)]
    pub workloads: bool,
    /// Installation-owned native runtime deployment (Kubernetes only).
    #[serde(default)]
    pub plugin_runtime_enabled: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlEndpoint {
    Unix {
        path: PathBuf,
    },
    Mtls {
        origin: String,
        domain: String,
        ca: PathBuf,
        certificate: PathBuf,
        key: PathBuf,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicRoutingConfig {
    pub domain: String,
    pub ingress_class: String,
    pub cluster_issuer: String,
}
impl PublicRoutingConfig {
    pub fn validate(&self) -> Result<()> {
        fn name(v: &str) -> bool {
            !v.is_empty()
                && v.len() <= 253
                && v.split('.').all(|l| {
                    !l.is_empty()
                        && l.len() <= 63
                        && !l.starts_with('-')
                        && !l.ends_with('-')
                        && l.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
        }
        if !name(&self.domain)
            || !self.domain.contains('.')
            || self.domain.parse::<std::net::IpAddr>().is_ok()
            || !name(&self.ingress_class)
            || !name(&self.cluster_issuer)
        {
            return Err(AgentError::Configuration("Invalid public routing policy"));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeConfiguration {
    Docker {
        socket: String,
        initializer_image: Option<String>,
        #[serde(default)]
        storage_socket: Option<PathBuf>,
    },
    Kubernetes {
        kubeconfig: PathBuf,
        context: String,
        helm: PathBuf,
        chart: PathBuf,
        chart_sha256: String,
        #[serde(default)]
        storage_socket: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        storage_node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        public_routing: Option<PublicRoutingConfig>,
    },
}
impl AgentConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let value: Self = serde_json::from_slice(&read_private(path, 1024 * 1024)?)
            .map_err(|_| AgentError::Configuration("Invalid agent configuration"))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        fn slug(value: &str, max: usize) -> bool {
            !value.is_empty()
                && value.len() <= max
                && value.as_bytes()[0].is_ascii_lowercase()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }
        if self.version != 1
            || !slug(&self.installation, 24)
            || !slug(&self.agent_id, 64)
            || self.generation == 0
            || self.generation > 9_007_199_254_740_991
        {
            return Err(AgentError::Configuration("Invalid agent identity"));
        }
        private_directory(&self.state_directory)?;
        match &self.control {
            ControlEndpoint::Unix { path } => {
                private_directory(
                    path.parent()
                        .ok_or(AgentError::Configuration("Socket directory required"))?,
                )?;
                if !path.is_absolute() {
                    return Err(AgentError::Configuration("Absolute socket required"));
                }
            }
            ControlEndpoint::Mtls {
                origin,
                domain,
                ca,
                certificate,
                key,
            } => {
                let url = spitzli_core::config::canonical_origin(origin, true)
                    .map_err(|_| AgentError::Configuration("Invalid control origin"))?;
                if !url.starts_with("https://") || domain.is_empty() || domain.len() > 253 {
                    return Err(AgentError::Configuration("Verified network TLS required"));
                }
                for file in [ca, certificate, key] {
                    read_private(file, 256 * 1024)?;
                }
            }
        }
        if let RuntimeConfiguration::Kubernetes {
            kubeconfig,
            storage_socket,
            storage_node,
            public_routing,
            ..
        } = &self.runtime
        {
            read_private(kubeconfig, 1024 * 1024)?;
            if let Some(config) = public_routing {
                config.validate()?;
            }
            if let Some(node) = storage_node
                && (storage_socket.is_none()
                    || node.is_empty()
                    || node.len() > 253
                    || !node.as_bytes()[0].is_ascii_lowercase()
                        && !node.as_bytes()[0].is_ascii_digit()
                    || !node.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'
                    }))
            {
                return Err(AgentError::Configuration(
                    "Selected local storage requires a socket and valid node name",
                ));
            }
        }
        if let Some(path) = &self.database_runtime {
            if !matches!(self.runtime, RuntimeConfiguration::Kubernetes { .. }) {
                return Err(AgentError::Configuration(
                    "Database runtime requires Kubernetes",
                ));
            }
            read_private(path, 1024 * 1024)?;
        }
        if self.mail_configuration_enabled && self.mail_runtime.is_none() {
            return Err(AgentError::Configuration(
                "Mail configuration requires a reviewed Mail runtime",
            ));
        }
        if let Some(path) = &self.mail_runtime {
            read_private(path, 1024 * 1024)?;
        }
        if let Some(path) = &self.recovery_runtime {
            if !cfg!(feature = "docker") {
                return Err(AgentError::Configuration(
                    "Qualified recovery targets require the explicitly isolated native Docker backend",
                ));
            }
            read_private(path, 1024 * 1024)?;
        }
        if self.plugin_runtime_enabled
            && !matches!(self.runtime, RuntimeConfiguration::Kubernetes { .. })
        {
            return Err(AgentError::Configuration(
                "Plugin runtime deployment requires Kubernetes",
            ));
        }
        if self.workloads && !matches!(self.runtime, RuntimeConfiguration::Kubernetes { .. }) {
            return Err(AgentError::Configuration(
                "Plugin workloads require Kubernetes",
            ));
        }
        Ok(())
    }
    pub fn identity(&self) -> crate::management::wire::AgentIdentity {
        crate::management::wire::AgentIdentity {
            installation: self.installation.clone(),
            agent_id: self.agent_id.clone(),
            generation: self.generation,
        }
    }
}
pub fn private_directory(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| AgentError::Configuration("Private directory unavailable"))?;
    if !path.is_absolute()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || std::fs::canonicalize(path)
            .map_err(|_| AgentError::Configuration("Invalid directory"))?
            != path
    {
        return Err(AgentError::Configuration(
            "Owned private canonical directory required",
        ));
    }
    Ok(())
}
pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !path.is_absolute()
        || std::fs::canonicalize(path)
            .map_err(|_| AgentError::Configuration("Private file unavailable"))?
            != path
    {
        return Err(AgentError::Configuration("Canonical private file required"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|_| AgentError::Configuration("Private file unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| AgentError::Configuration("Private file metadata unavailable"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
        || metadata.len() > limit
    {
        return Err(AgentError::Configuration(
            "Owned mode-0600 regular file required",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AgentError::Configuration("Private file unreadable"))?;
    if bytes.len() as u64 > limit {
        return Err(AgentError::Configuration("Private file exceeds limit"));
    }
    Ok(bytes)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredIdentity {
    version: u8,
    installation: String,
    agent_id: String,
    generation: u64,
    sequence: u64,
    certificate_sha256: String,
    revoked: bool,
}
pub struct IdentityState {
    path: PathBuf,
    value: StoredIdentity,
    poisoned: bool,
}
impl IdentityState {
    /// Caller must hold the executor journal's exclusive process lock for this state directory.
    pub fn open(config: &AgentConfig, certificate_sha256: &str) -> Result<Self> {
        private_directory(&config.state_directory)?;
        let path = config.state_directory.join("agent-state.json");
        let exists = match std::fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(AgentError::State),
        };
        let value = if exists {
            let mut old: StoredIdentity = serde_json::from_slice(&read_private(&path, 32768)?)
                .map_err(|_| AgentError::State)?;
            if old.version != 1
                || old.installation != config.installation
                || old.agent_id != config.agent_id
                || config.generation < old.generation
            {
                return Err(AgentError::State);
            }
            if old.revoked
                && old.generation == config.generation
                && old.certificate_sha256 == certificate_sha256
            {
                return Err(AgentError::Revoked);
            }
            if old.generation != config.generation {
                old.generation = config.generation;
                old.sequence = 0;
            }
            old.certificate_sha256 = certificate_sha256.into();
            old.revoked = false;
            old
        } else {
            StoredIdentity {
                version: 1,
                installation: config.installation.clone(),
                agent_id: config.agent_id.clone(),
                generation: config.generation,
                sequence: 0,
                certificate_sha256: certificate_sha256.into(),
                revoked: false,
            }
        };
        let mut state = Self {
            path,
            value,
            poisoned: false,
        };
        state.save()?;
        Ok(state)
    }
    pub fn next_sequence(&mut self) -> Result<u64> {
        if self.poisoned || self.value.revoked {
            return Err(AgentError::State);
        }
        self.value.sequence = self
            .value
            .sequence
            .checked_add(1)
            .ok_or(AgentError::State)?;
        self.save()?;
        Ok(self.value.sequence)
    }
    pub fn revoke(&mut self) -> Result<()> {
        self.value.revoked = true;
        self.save()
    }
    fn save(&mut self) -> Result<()> {
        if self.poisoned {
            return Err(AgentError::State);
        }
        let result = (|| -> std::io::Result<()> {
            let parent = self
                .path
                .parent()
                .ok_or_else(|| std::io::Error::other("state directory"))?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec(&self.value)?)?;
            file.as_file().sync_all()?;
            file.persist(&self.path).map_err(|e| e.error)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
            return Err(AgentError::State);
        }
        Ok(())
    }
}
