//! Operator-reviewed repository grants. Credentials never come from runtime commands.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use webdock_supervisor::*;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryConfig {
    pub version: u8,
    pub eu_storage_evidence: String,
    pub projects: BTreeMap<String, RegistryProject>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryProject {
    pub repository: String,
    #[serde(default)]
    pub pull_secret: Option<String>,
    #[serde(default)]
    pub credential_file: Option<PathBuf>,
    pub read_only: bool,
    pub credential_scope_evidence: String,
}
impl RegistryConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 262144 {
            return Err(ExecutionError::Invalid("registry configuration limit"));
        }
        let config: Self = serde_json::from_slice(bytes)
            .map_err(|_| ExecutionError::Invalid("registry configuration"))?;
        if config.version != 1
            || config.eu_storage_evidence.trim().is_empty()
            || config.eu_storage_evidence.len() > 4096
            || config.projects.is_empty()
            || config.projects.len() > 32
        {
            return Err(ExecutionError::Invalid("registry configuration"));
        }
        for (project, grant) in &config.projects {
            webdock_core::id::RecordId::parse(project)
                .map_err(|_| ExecutionError::Invalid("registry project"))?;
            if !grant.read_only
                || grant.credential_scope_evidence.trim().is_empty()
                || grant.credential_scope_evidence.len() > 4096
                || !image_digest(&format!("{}@sha256:{}", grant.repository, "0".repeat(64)))
                || (grant.pull_secret.is_none() && grant.credential_file.is_none())
                || grant.pull_secret.as_ref().is_some_and(|name| {
                    name.is_empty()
                        || name.len() > 63
                        || !name.as_bytes()[0].is_ascii_alphanumeric()
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                })
                || grant
                    .credential_file
                    .as_ref()
                    .is_some_and(|path| !path.is_absolute() || path.as_os_str().len() > 2048)
            {
                return Err(ExecutionError::Invalid("read-only registry grant"));
            }
        }
        Ok(config)
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&private_bytes(path, 262144)?)
    }
    pub fn select(&self, project: &str, image: &str) -> Result<Option<&RegistryProject>> {
        if !image_digest(image) {
            return Err(ExecutionError::Invalid("registry image digest"));
        }
        let host = image
            .split('/')
            .next()
            .ok_or(ExecutionError::Invalid("registry host"))?;
        if !self
            .projects
            .values()
            .any(|p| p.repository.split('/').next() == Some(host))
        {
            return Ok(None);
        }
        let grant = self.projects.get(project).ok_or(ExecutionError::Denied)?;
        if image
            .strip_prefix(&grant.repository)
            .is_none_or(|suffix| !suffix.starts_with("@sha256:"))
        {
            return Err(ExecutionError::Denied);
        }
        Ok(Some(grant))
    }
}
fn private_bytes(path: &Path, max: usize) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ExecutionError::Denied)?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
        || std::fs::canonicalize(path).map_err(|_| ExecutionError::Denied)? != path
    {
        return Err(ExecutionError::Ownership);
    }
    crate::config::read_private(path, max as u64).map_err(|_| ExecutionError::Denied)
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryCredential {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub identitytoken: Option<String>,
    #[serde(default)]
    pub registrytoken: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub serveraddress: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    auths: BTreeMap<String, RegistryCredential>,
}
impl RegistryCredential {
    pub fn parse(bytes: &[u8], authority: &str) -> Result<Self> {
        if bytes.len() > 65536 {
            return Err(ExecutionError::Invalid("registry credential limit"));
        }
        let file: CredentialFile = serde_json::from_slice(bytes)
            .map_err(|_| ExecutionError::Invalid("registry credential"))?;
        if file.auths.len() != 1 {
            return Err(ExecutionError::Ownership);
        }
        let (key, mut credential) = file
            .auths
            .into_iter()
            .next()
            .ok_or(ExecutionError::Ownership)?;
        let normalized = if authority == "docker.io" && key == "https://index.docker.io/v1/" {
            "docker.io"
        } else {
            key.strip_prefix("https://").unwrap_or(&key)
        };
        if normalized != authority
            || credential
                .serveraddress
                .as_deref()
                .is_some_and(|v| v.strip_prefix("https://").unwrap_or(v) != authority)
        {
            return Err(ExecutionError::Ownership);
        }
        for value in [
            &credential.username,
            &credential.password,
            &credential.auth,
            &credential.identitytoken,
            &credential.registrytoken,
            &credential.email,
        ] {
            if value
                .as_ref()
                .is_some_and(|v| v.len() > 16384 || v.contains('\0'))
            {
                return Err(ExecutionError::Invalid("registry credential"));
            }
        }
        if credential.username.is_none()
            && credential.password.is_none()
            && let Some(auth) = &credential.auth
        {
            let raw = STANDARD
                .decode(auth)
                .map_err(|_| ExecutionError::Invalid("registry auth encoding"))?;
            let text = String::from_utf8(raw)
                .map_err(|_| ExecutionError::Invalid("registry auth encoding"))?;
            let (user, password) = text
                .split_once(':')
                .ok_or(ExecutionError::Invalid("registry auth encoding"))?;
            credential.username = Some(user.into());
            credential.password = Some(password.into());
        }
        if !(credential.username.as_ref().is_some_and(|u| !u.is_empty())
            && credential.password.as_ref().is_some_and(|p| !p.is_empty()))
            && !credential
                .identitytoken
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            && !credential
                .registrytoken
                .as_ref()
                .is_some_and(|v| !v.is_empty())
        {
            return Err(ExecutionError::Invalid("registry credential"));
        }
        if [&credential.username, &credential.password]
            .into_iter()
            .any(|v| {
                v.as_ref()
                    .is_some_and(|s| s.len() > 16384 || s.contains('\0'))
            })
        {
            return Err(ExecutionError::Invalid("registry credential"));
        }
        credential.serveraddress = Some(authority.into());
        credential.auth = None;
        Ok(credential)
    }
    pub fn load(path: &Path, authority: &str) -> Result<Self> {
        Self::parse(&private_bytes(path, 65536)?, authority)
    }
    #[cfg(feature = "docker")]
    pub fn docker(self) -> bollard::auth::DockerCredentials {
        bollard::auth::DockerCredentials {
            username: self.username,
            password: self.password,
            auth: None,
            email: self.email,
            serveraddress: self.serveraddress,
            identitytoken: self.identitytoken,
            registrytoken: self.registrytoken,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullSelection {
    Unconfigured,
    Anonymous,
    Secret(String),
}
