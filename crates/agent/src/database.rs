//! Fixed PostgreSQL profiles and operator-owned placement. No tenant images, SQL or YAML.
mod backup;
mod executor;
#[allow(
    dead_code,
    reason = "Shared export helper includes SQL DROP; native dedicated purge removes the whole owned installation."
)]
mod exports;
mod lifecycle;
mod manifests;
mod observations;
mod sql;
pub use backup::{DatabaseBackupRuntimeConfig, backup_manifests};
pub use executor::{
    DatabaseAuthority, DatabaseRuntime, DatabaseStep, DenyDatabaseAuthority, NativeDatabaseExecutor,
};
pub use manifests::backend_manifests;
pub use observations::DatabaseCapabilities;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use webdock_agent_protocol::database::{DatabaseAction, DatabaseJobSpec};
use webdock_supervisor::{ExecutionError, Result};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuxiliaryDatabase {
    pub name: String,
    pub roles: Vec<String>,
    #[serde(rename = "projectID")]
    pub project_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseStorageRoute {
    pub storage_socket: PathBuf,
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseRuntimeConfig {
    pub image: String,
    pub pooler_image: String,
    pub issuer: String,
    #[serde(rename = "apiCIDRs")]
    pub api_cidrs: Vec<String>,
    #[serde(rename = "podCIDRs")]
    pub pod_cidrs: Vec<String>,
    pub storage_socket: PathBuf,
    pub ca_file: PathBuf,
    #[serde(default)]
    pub platform_databases: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub platform_auxiliary_databases: BTreeMap<String, Vec<AuxiliaryDatabase>>,
    #[serde(default)]
    pub additional_storage_pools: Vec<DatabaseStorageRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<DatabaseBackupRuntimeConfig>,
}
fn invalid() -> ExecutionError {
    ExecutionError::Invalid("database operator configuration")
}
fn id(value: &str) -> bool {
    webdock_core::id::RecordId::parse(value).is_ok()
}
fn role(value: &str) -> bool {
    value.strip_prefix("webdock_").is_some_and(|v| {
        !v.is_empty()
            && v.len() <= 54
            && v.as_bytes()[0].is_ascii_lowercase()
            && v.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    })
}
fn socket(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    value.len() <= 103
        && value
            .strip_prefix("/run/webdock-postgres-storage/")
            .is_some_and(|rest| {
                rest.ends_with(".sock")
                    && rest.split('/').all(|part| {
                        !part.is_empty()
                            && part != "."
                            && part != ".."
                            && part.bytes().all(|b| {
                                b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'
                            })
                    })
            })
}
impl DatabaseRuntimeConfig {
    pub fn validate(&self) -> Result<()> {
        let image: oci_spec::distribution::Reference = self.image.parse().map_err(|_| invalid())?;
        let pooler: oci_spec::distribution::Reference =
            self.pooler_image.parse().map_err(|_| invalid())?;
        let pinned = |value: &oci_spec::distribution::Reference| {
            value.digest().is_some_and(|d| {
                d.strip_prefix("sha256:").is_some_and(|v| {
                    v.len() == 64
                        && v.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
            })
        };
        if self.image.len() > 512
            || self.pooler_image.len() > 512
            || !pinned(&image)
            || !pinned(&pooler)
            || !image
                .tag()
                .and_then(|v| v.strip_prefix("18."))
                .is_some_and(|v| !v.is_empty() && v.as_bytes()[0].is_ascii_digit())
            || self.issuer.is_empty()
            || self.issuer.len() > 63
            || !self.issuer.as_bytes()[0].is_ascii_alphanumeric()
            || !self
                .issuer
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !socket(&self.storage_socket)
            || !self.ca_file.is_absolute()
        {
            return Err(invalid());
        }
        if let Some(backup) = &self.backup {
            backup.validate()?;
        }
        for cidrs in [&self.api_cidrs, &self.pod_cidrs] {
            if cidrs.is_empty() || cidrs.len() > 20 {
                return Err(invalid());
            }
            for cidr in cidrs {
                let network: ipnet::IpNet = cidr.parse().map_err(|_| invalid())?;
                if network != network.trunc() {
                    return Err(invalid());
                }
            }
        }
        if self.platform_databases.len() > 10
            || self.platform_auxiliary_databases.len() > 10
            || self.additional_storage_pools.len() > 7
        {
            return Err(invalid());
        }
        for (identity, roles) in &self.platform_databases {
            if !id(identity)
                || roles.is_empty()
                || roles.len() > 32
                || roles.iter().any(|r| !role(r))
                || roles.iter().collect::<BTreeSet<_>>().len() != roles.len()
            {
                return Err(invalid());
            }
        }
        let mut names = BTreeSet::new();
        for (identity, entries) in &self.platform_auxiliary_databases {
            if !id(identity) || entries.is_empty() || entries.len() > 10 {
                return Err(invalid());
            }
            for entry in entries {
                if !role(&entry.name)
                    || !id(&entry.project_id)
                    || entry.roles.is_empty()
                    || entry.roles.len() > 10
                    || entry.roles.iter().any(|r| !role(r))
                    || entry.roles.iter().collect::<BTreeSet<_>>().len() != entry.roles.len()
                    || !names.insert(&entry.name)
                {
                    return Err(invalid());
                }
            }
        }
        let mut paths = BTreeSet::from([&self.storage_socket]);
        let mut nodes = BTreeSet::new();
        let mut pools = BTreeSet::new();
        for route in &self.additional_storage_pools {
            if !socket(&route.storage_socket)
                || !paths.insert(&route.storage_socket)
                || route.node.is_empty()
                || route.node.len() > 253
                || !route.node.as_bytes()[0].is_ascii_alphanumeric()
                || !route
                    .node
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
                || uuid::Uuid::parse_str(&route.pool_id).is_err()
                || uuid::Uuid::parse_str(&route.pool_id).unwrap().to_string() != route.pool_id
                || !nodes.insert(&route.node)
                || !pools.insert(&route.pool_id)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = crate::config::read_private(path, 1024 * 1024).map_err(|_| invalid())?;
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        config.validate()?;
        config.ca_certificate()?;
        Ok(config)
    }
    pub fn ca_certificate(&self) -> Result<Vec<u8>> {
        use std::{
            io::Read,
            os::unix::fs::{MetadataExt, OpenOptionsExt},
        };
        if std::fs::canonicalize(&self.ca_file).map_err(|_| invalid())? != self.ca_file {
            return Err(invalid());
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&self.ca_file)
            .map_err(|_| invalid())?;
        let meta = file.metadata().map_err(|_| invalid())?;
        if !meta.is_file()
            || meta.nlink() != 1
            || (meta.uid() != 0 && meta.uid() != rustix::process::geteuid().as_raw())
            || meta.mode() & 0o022 != 0
        {
            return Err(invalid());
        }
        // Public trust bundles may be readable by the agent; they must never be writable by it through another identity.
        let mut bytes = Vec::new();
        file.take(262145)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() > 262144 {
            return Err(invalid());
        }
        if bytes.windows(11).any(|w| w == b"PRIVATE KEY") {
            return Err(invalid());
        }
        let mut count = 0;
        for pem in x509_parser::pem::Pem::iter_from_buffer(&bytes) {
            let pem = pem.map_err(|_| invalid())?;
            if pem.label != "CERTIFICATE" {
                return Err(invalid());
            }
            let cert = pem.parse_x509().map_err(|_| invalid())?;
            if !cert.validity().is_valid() {
                return Err(invalid());
            }
            count += 1;
        }
        if count == 0 {
            return Err(invalid());
        }
        Ok(bytes)
    }
    pub fn admit(&self, job: &DatabaseJobSpec) -> Result<()> {
        self.validate()?;
        job.validate()
            .map_err(|_| ExecutionError::Invalid("database command"))?;
        if matches!(
            job.action,
            DatabaseAction::Pause
                | DatabaseAction::Restart
                | DatabaseAction::Resize
                | DatabaseAction::Delete
                | DatabaseAction::Purge
                | DatabaseAction::RotateCredentials
                | DatabaseAction::Migrate
        ) && job
            .members
            .iter()
            .map(|m| m.id.as_str())
            .chain(job.database.iter().map(|d| d.id.as_str()))
            .any(|id| {
                self.platform_databases.contains_key(id)
                    || self.platform_auxiliary_databases.contains_key(id)
            })
        {
            return Err(ExecutionError::Denied);
        }
        if job.backup.is_some() || job.recovery.is_some() {
            let config = self.backup.as_ref().ok_or(ExecutionError::Denied)?;
            let p = job
                .backup
                .as_ref()
                .or_else(|| job.recovery.as_ref().map(|r| &r.source))
                .ok_or(ExecutionError::Denied)?;
            if !config.endpoints.contains(&p.destination.endpoint_url)
                || (job.backup.is_some() && p.resource_revision != job.backend.revision)
            {
                return Err(ExecutionError::Denied);
            }
        }
        Ok(())
    }
    pub fn storage_route(&self, node: &str, pool: &str) -> Result<&Path> {
        self.validate()?;
        let mut selected = self.storage_socket.as_path();
        for route in &self.additional_storage_pools {
            let a = route.node == node;
            let b = route.pool_id == pool;
            if a != b {
                return Err(ExecutionError::Ownership);
            }
            if a {
                selected = &route.storage_socket;
            }
        }
        Ok(selected)
    }
    fn auxiliary<'a>(&'a self, job: &DatabaseJobSpec) -> Result<Vec<&'a AuxiliaryDatabase>> {
        let entries = self
            .platform_auxiliary_databases
            .iter()
            .filter(|(id, _)| job.members.iter().any(|m| &m.id == *id && m.enabled))
            .flat_map(|(_, entries)| entries)
            .collect::<Vec<_>>();
        if !entries.is_empty() && job.backend.kind != "dedicated" {
            return Err(ExecutionError::Ownership);
        }
        Ok(entries)
    }
    fn hba(&self, job: &DatabaseJobSpec) -> Result<Vec<String>> {
        let mut rows = Vec::new();
        for member in job.members.iter().filter(|m| m.enabled) {
            let mut roles = vec![
                format!("wd_migrate_{}", member.id),
                format!("wd_app_{}", member.id),
            ];
            if let Some(export) =
                exports::ExportRole::new(&member.id, &member.database_name, &job.backend.kind)
                    .map_err(|_| ExecutionError::Ownership)?
            {
                roles.push(export.name);
            }
            if let Some(extra) = self.platform_databases.get(&member.id) {
                if job.backend.kind != "dedicated" {
                    return Err(ExecutionError::Ownership);
                }
                roles.extend(extra.iter().cloned());
            }
            rows.push(format!(
                "hostssl {} {} all scram-sha-256",
                member.database_name,
                roles.join(",")
            ));
        }
        for entry in self.auxiliary(job)? {
            rows.push(format!(
                "hostssl {} {} all scram-sha-256",
                entry.name,
                entry.roles.join(",")
            ));
        }
        rows.push("host all all all reject".into());
        Ok(rows)
    }
}
