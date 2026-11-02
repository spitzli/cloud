//! Database execution is a distinct domain; application commands cannot represent these phases.
use std::fmt;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatabaseProtocolError;
impl fmt::Display for DatabaseProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Invalid database execution contract.")
    }
}
impl std::error::Error for DatabaseProtocolError {}
pub type Result<T> = std::result::Result<T, DatabaseProtocolError>;
fn invalid() -> DatabaseProtocolError {
    DatabaseProtocolError
}
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseLease {
    #[serde(rename = "operationID")]
    pub operation_id: String,
    /// Enrolled agent generation; operation generation is separately bound by the lease token.
    pub generation: i32,
    pub lease_token: String,
}
impl DatabaseLease {
    pub fn validate(&self) -> Result<()> {
        if !record(&self.operation_id) || self.generation <= 0 || !hex64(&self.lease_token) {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseCheckpointPhase {
    Applied,
    Configuring,
    Observing,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseCheckpoint {
    #[serde(flatten)]
    pub lease: DatabaseLease,
    pub phase: DatabaseCheckpointPhase,
    #[serde(rename = "cnpgUID")]
    pub cnpg_uid: String,
    pub ca_certificate: Option<String>,
    #[serde(rename = "previousPodUID")]
    pub previous_pod_uid: Option<String>,
    #[serde(rename = "migrationJobUID")]
    pub migration_job_uid: Option<String>,
    #[serde(rename = "copySHA256")]
    pub copy_sha256: Option<String>,
}
impl DatabaseCheckpoint {
    pub fn validate(&self) -> Result<()> {
        self.lease.validate()?;
        if !uid(&self.cnpg_uid)
            || self.previous_pod_uid.as_ref().is_some_and(|v| !uid(v))
            || self.migration_job_uid.as_ref().is_some_and(|v| !uid(v))
            || self.copy_sha256.as_ref().is_some_and(|v| !hex64(v))
            || self
                .ca_certificate
                .as_ref()
                .is_some_and(|v| v.len() > 262144)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseCompletionOutcome {
    Succeeded,
    NeedsReconciliation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseCompletionProof {
    #[serde(rename = "installationID")]
    pub installation_id: String,
    #[serde(rename = "databaseID")]
    pub database_id: Option<String>,
    #[serde(rename = "cnpgUID")]
    pub cnpg_uid: String,
    pub revision: i32,
    pub state: DatabaseProofState,
    pub endpoint_ready: bool,
    #[serde(rename = "copySHA256")]
    pub copy_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<Box<DatabaseBackupProof>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseProofState {
    Ready,
    Paused,
    Retained,
    Purged,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseCompletion {
    #[serde(flatten)]
    pub lease: DatabaseLease,
    pub outcome: DatabaseCompletionOutcome,
    pub proof: Option<DatabaseCompletionProof>,
}
impl DatabaseCompletion {
    pub fn validate(&self) -> Result<()> {
        self.lease.validate()?;
        if self.outcome == DatabaseCompletionOutcome::Succeeded && self.proof.is_none() {
            return Err(invalid());
        }
        if let Some(p) = &self.proof
            && let Some(backup) = &p.backup
        {
            backup.validate()?;
        }
        if let Some(p) = &self.proof
            && (!record(&p.installation_id)
                || p.database_id.as_ref().is_some_and(|s| !record(s))
                || !uid(&p.cnpg_uid)
                || p.revision <= 0
                || p.copy_sha256.as_ref().is_some_and(|s| !hex64(s)))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DatabaseAction {
    Create,
    Pause,
    Resume,
    Restart,
    RotateCredentials,
    Delete,
    Purge,
    Resize,
    ConfigureBackup,
    Backup,
    Restore,
    PrepareMigration,
    Migrate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseExecutionPhase {
    Apply,
    Configure,
    Observe,
    Uncertain,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseResources {
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
    pub volume_bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackendSpec {
    pub id: String,
    pub kind: String,
    pub namespace: String,
    pub node: String,
    #[serde(rename = "poolID")]
    pub pool_id: String,
    pub revision: i32,
    #[serde(rename = "cnpgUID")]
    pub cnpg_uid: Option<String>,
    pub resources: DatabaseResources,
    pub host: String,
    pub direct_port: u16,
    pub pooled_port: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseTargetSpec {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub revision: i32,
    pub credential_revision: i32,
    pub connection_limit: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseMemberSpec {
    pub id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    pub database_name: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseMigrationSource {
    pub id: String,
    pub namespace: String,
    #[serde(rename = "cnpgUID")]
    pub cnpg_uid: String,
    pub host: String,
    pub kind: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseMigrationSpec {
    pub id: String,
    pub source: DatabaseMigrationSource,
    #[serde(rename = "jobUID")]
    pub job_uid: Option<String>,
    #[serde(rename = "copySHA256")]
    pub copy_sha256: Option<String>,
}
/// Safe to persist. Renewable expiry, bearer lease proof and credentials are outside this spec.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseJobSpec {
    pub version: u8,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    pub agent_generation: i32,
    pub operation_generation: i32,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    pub action: DatabaseAction,
    pub phase: DatabaseExecutionPhase,
    pub revision: i32,
    #[serde(rename = "previousPodUID")]
    pub previous_pod_uid: Option<String>,
    pub backend: DatabaseBackendSpec,
    pub database: Option<DatabaseTargetSpec>,
    pub members: Vec<DatabaseMemberSpec>,
    pub migration: Option<DatabaseMigrationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<DatabaseBackupPolicySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<DatabaseRecoverySpec>,
}
impl DatabaseJobSpec {
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        Ok(hex::encode(Sha256::digest(
            serde_json::to_vec(self).map_err(|_| invalid())?,
        )))
    }
}
/// Ephemeral authenticated transport material. Deliberately has no Debug implementation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseCredentials {
    pub application_password: String,
    pub migration_password: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseMaterial {
    pub credentials: Option<DatabaseCredentials>,
    pub transfer_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_store: Option<DatabaseObjectStoreMaterial>,
}
/// Not Serialize: transport must deliberately separate safe spec and ephemeral material.
pub struct DatabaseClaim {
    pub spec: DatabaseJobSpec,
    pub lease: DatabaseLease,
    pub lease_until: chrono::DateTime<chrono::Utc>,
    pub material: DatabaseMaterial,
}
impl DatabaseClaim {
    /// Retained HTTPS agent wire, only at an authenticated transport boundary. Never persist this value.
    pub fn retained_payload(&self) -> Result<Value> {
        if self.spec.backup.is_some() || self.spec.recovery.is_some() {
            return Err(invalid());
        }
        let mut value = serde_json::to_value(&self.spec).map_err(|_| invalid())?;
        let object = value.as_object_mut().ok_or_else(invalid)?;
        object.remove("agentGeneration");
        object.remove("operationGeneration");
        object.insert("generation".into(), json!(self.lease.generation));
        object.insert("leaseToken".into(), json!(self.lease.lease_token));
        object.insert(
            "leaseUntil".into(),
            json!(
                self.lease_until
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ),
        );
        if let Some(credentials) = &self.material.credentials {
            value["database"]["credentials"] =
                serde_json::to_value(credentials).map_err(|_| invalid())?;
        }
        if let Some(password) = &self.material.transfer_password {
            value["migration"]["transferPassword"] = json!(password);
        }
        if value["migration"].is_null() {
            value.as_object_mut().unwrap().remove("migration");
        }
        Ok(value)
    }
}
/// Immutable archive identity. Credentials and recovery key bytes are transported separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackupDestination {
    pub id: String,
    #[serde(rename = "endpointURL")]
    pub endpoint_url: String,
    pub region: String,
    pub bucket: String,
    pub archive_path: String,
    pub server_name: String,
    #[serde(rename = "credentialID")]
    pub credential_id: String,
    pub credential_revision: i32,
    #[serde(rename = "recoveryCredentialID")]
    pub recovery_credential_id: String,
    pub recovery_credential_revision: i32,
    #[serde(rename = "keyRecoveryID")]
    pub key_recovery_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackupResources {
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
    pub ephemeral_bytes: u64,
}
impl DatabaseBackupResources {
    pub fn validate(&self) -> Result<()> {
        if !(100..=64000).contains(&self.cpu_millicores)
            || !(134217728..=1099511627776).contains(&self.memory_bytes)
            || !(134217728..=1099511627776).contains(&self.ephemeral_bytes)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
/// A Core grant, never a renewable browser/session authority. Revocation suspends recurrence;
/// stopping WAL and authorizing retention deletion are independent operator decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackupPolicySpec {
    pub id: String,
    #[serde(rename = "installationID")]
    pub installation_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "sourceCnpgUID")]
    pub source_cnpg_uid: String,
    pub revision: i32,
    pub resource_revision: i32,
    pub destination: DatabaseBackupDestination,
    pub schedule: String,
    pub suspended: bool,
    pub wal_enabled: bool,
    pub retention_days: u32,
    pub retention_delete: bool,
    pub sidecar: DatabaseBackupResources,
    pub recovery_job: DatabaseBackupResources,
}
impl DatabaseBackupPolicySpec {
    pub fn validate(&self) -> Result<()> {
        let d = &self.destination;
        if [
            &self.id,
            &self.installation_id,
            &self.customer_id,
            &self.project_id,
            &d.id,
            &d.credential_id,
            &d.recovery_credential_id,
            &d.key_recovery_id,
        ]
        .iter()
        .any(|v| !record(v))
            || !uid(&self.source_cnpg_uid)
            || self.revision <= 0
            || self.resource_revision <= 0
            || d.credential_revision <= 0
            || d.recovery_credential_revision <= 0
            || d.credential_id == d.recovery_credential_id
            || d.archive_path
                != format!(
                    "postgresql/{}/{}/{}/{}",
                    self.customer_id, self.project_id, self.installation_id, self.id
                )
            || d.server_name != format!("wd-{}-{}", self.installation_id, self.id)
            || (!self.wal_enabled && !self.suspended)
            || !(1..=3650).contains(&self.retention_days)
            || !(3..=63).contains(&d.bucket.len())
            || !d
                .bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || d.bucket.starts_with('-')
            || d.bucket.ends_with('-')
            || d.region.is_empty()
            || d.region.len() > 63
            || !d
                .region
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(invalid());
        }
        if !daily_schedule(&self.schedule) {
            return Err(invalid());
        }
        let authority = d
            .endpoint_url
            .strip_prefix("https://")
            .ok_or_else(invalid)?;
        let host = authority.strip_suffix('/').unwrap_or(authority);
        if host.is_empty()
            || host.len() > 253
            || d.endpoint_url.len() > 512
            || !host
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'-' | b'.' | b':'))
            || host.split(':').count() > 2
            || host
                .split_once(':')
                .is_some_and(|(_, p)| p.parse::<u16>().ok().is_none_or(|v| v == 0))
        {
            return Err(invalid());
        }
        self.sidecar.validate()?;
        self.recovery_job.validate()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseRecoverySpec {
    pub source: DatabaseBackupPolicySpec,
    #[serde(rename = "sourceDatabaseID")]
    pub source_database_id: String,
    pub base_backup_id: String,
    pub target_time: chrono::DateTime<chrono::Utc>,
    pub timeline: u32,
    pub credential_revision: i32,
    #[serde(rename = "verificationSHA256")]
    pub verification_sha256: String,
}
/// Ephemeral; no Debug, and never part of a persisted job fingerprint.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseObjectStoreMaterial {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub read_only: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackupProof {
    #[serde(rename = "policyID")]
    pub policy_id: String,
    pub revision: i32,
    #[serde(rename = "scheduledBackupUID")]
    pub scheduled_backup_uid: String,
    pub suspended: bool,
    pub controls: DatabaseBackupControls,
    #[serde(rename = "backupUID")]
    pub backup_uid: Option<String>,
    pub base_backup_id: Option<String>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseBackupControls {
    pub schedule: String,
    pub wal_enabled: bool,
    pub retention_delete: bool,
    pub retention_days: u32,
    pub scheduled_backup_resource_version: String,
    pub cluster_resource_version: String,
    #[serde(rename = "objectStoreUID")]
    pub object_store_uid: String,
    pub object_store_resource_version: String,
}

fn daily_schedule(schedule: &str) -> bool {
    let fields = schedule.split(' ').collect::<Vec<_>>();
    fields.len() == 6
        && fields[0] == "0"
        && fields[3..] == ["*", "*", "*"]
        && fields[1].parse::<u8>().is_ok_and(|v| v <= 59)
        && fields[2].parse::<u8>().is_ok_and(|v| v <= 23)
}
impl DatabaseBackupProof {
    pub fn validate(&self) -> Result<()> {
        if !record(&self.policy_id)
            || self.revision <= 0
            || !uid(&self.scheduled_backup_uid)
            || !uid(&self.controls.object_store_uid)
            || [
                &self.controls.scheduled_backup_resource_version,
                &self.controls.cluster_resource_version,
                &self.controls.object_store_resource_version,
            ]
            .iter()
            .any(|v| !uid(v))
            || !daily_schedule(&self.controls.schedule)
            || !(1..=3650).contains(&self.controls.retention_days)
            || self.backup_uid.as_ref().is_some_and(|s| !uid(s))
            || self.base_backup_id.as_ref().is_some_and(|s| {
                s.len() != 15
                    || s.bytes().enumerate().any(|(i, v)| {
                        if i == 8 {
                            v != b'T'
                        } else {
                            !v.is_ascii_digit()
                        }
                    })
            })
            || [
                self.backup_uid.is_some(),
                self.base_backup_id.is_some(),
                self.completed_at.is_some(),
            ]
            .iter()
            .any(|v| *v != self.backup_uid.is_some())
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn record(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('0')
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<i64>().is_ok()
}
fn uid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl DatabaseJobSpec {
    /// Strict retained packet bounds before any provider or privileged storage operation.
    pub fn validate(&self) -> Result<()> {
        let rev = |value: i32| (1..=2_147_483_646).contains(&value);
        if self.version != 1
            || !record(&self.operation_id)
            || !record(&self.cluster_id)
            || !rev(self.agent_generation)
            || !rev(self.operation_generation)
            || !rev(self.revision)
            || self.previous_pod_uid.as_ref().is_some_and(|s| !uid(s))
        {
            return Err(invalid());
        }
        let b = &self.backend;
        if !record(&b.id)
            || b.namespace != format!("wd-db-{}", b.id)
            || !["shared-pool", "dedicated"].contains(&b.kind.as_str())
            || !rev(b.revision)
            || b.cnpg_uid.as_ref().is_some_and(|s| !uid(s))
            || !node_name(&b.node)
            || uuid::Uuid::parse_str(&b.pool_id)
                .ok()
                .is_none_or(|u| u.to_string() != b.pool_id)
            || !database_host(&b.id, &b.host)
            || !(32000..=32199).contains(&b.direct_port)
            || !(32000..=32199).contains(&b.pooled_port)
            || b.direct_port == b.pooled_port
        {
            return Err(invalid());
        }
        let r = &b.resources;
        if !(100..=64000).contains(&r.cpu_millicores)
            || !(134217728..=1099511627776).contains(&r.memory_bytes)
            || !(67108864..=1099511627776).contains(&r.volume_bytes)
        {
            return Err(invalid());
        }
        if self.members.len() > 100 {
            return Err(invalid());
        }
        let mut seen = std::collections::HashSet::new();
        for member in &self.members {
            if !record(&member.id)
                || !record(&member.project_id)
                || member.database_name != format!("db_{}", member.id)
                || !seen.insert(member.id.as_str())
            {
                return Err(invalid());
            }
        }
        if let Some(d) = &self.database
            && (!record(&d.id)
                || d.name != format!("db_{}", d.id)
                || d.kind
                    != if b.kind == "shared-pool" {
                        "shared"
                    } else {
                        "dedicated"
                    }
                || d.revision != self.revision
                || !rev(d.credential_revision)
                || !(2..=100).contains(&d.connection_limit)
                || !seen.contains(d.id.as_str())
                || (b.kind == "shared-pool" && self.action == DatabaseAction::Restart))
        {
            return Err(invalid());
        }
        if self.action == DatabaseAction::PrepareMigration && self.database.is_some() {
            return Err(invalid());
        }
        if let Some(m) = &self.migration {
            let s = &m.source;
            if self.action != DatabaseAction::Migrate
                || self.database.is_none()
                || !record(&m.id)
                || !record(&s.id)
                || s.id == b.id
                || s.namespace != format!("wd-db-{}", s.id)
                || !uid(&s.cnpg_uid)
                || !["shared-pool", "dedicated"].contains(&s.kind.as_str())
                || !database_host(&s.id, &s.host)
                || m.job_uid.as_ref().is_some_and(|s| !uid(s))
                || m.copy_sha256.as_ref().is_some_and(|s| !hex64(s))
            {
                return Err(invalid());
            }
        } else if self.action == DatabaseAction::Migrate {
            return Err(invalid());
        }
        if let Some(p) = &self.backup {
            p.validate()?;
            if !matches!(
                self.action,
                DatabaseAction::ConfigureBackup | DatabaseAction::Backup
            ) || b.kind != "dedicated"
                || p.installation_id != b.id
                || Some(&p.source_cnpg_uid) != b.cnpg_uid.as_ref()
                || self.members.len() != 1
                || self.members[0].project_id != p.project_id
                || (self.action == DatabaseAction::Backup && (p.suspended || !p.wal_enabled))
            {
                return Err(invalid());
            }
        } else if matches!(
            self.action,
            DatabaseAction::ConfigureBackup | DatabaseAction::Backup
        ) {
            return Err(invalid());
        }
        if let Some(r) = &self.recovery {
            r.source.validate()?;
            if self.action != DatabaseAction::Restore
                || self.backup.is_some()
                || b.kind != "dedicated"
                || b.id == r.source.installation_id
                || !record(&r.source_database_id)
                || r.credential_revision <= 0
                || r.timeline == 0
                || !hex64(&r.verification_sha256)
                || r.base_backup_id.len() != 15
                || r.base_backup_id.bytes().enumerate().any(|(i, v)| {
                    if i == 8 {
                        v != b'T'
                    } else {
                        !v.is_ascii_digit()
                    }
                })
                || self
                    .database
                    .as_ref()
                    .is_none_or(|d| d.credential_revision != r.credential_revision)
                || self.members.len() != 1
                || self.members[0].project_id != r.source.project_id
            {
                return Err(invalid());
            }
        } else if self.action == DatabaseAction::Restore {
            return Err(invalid());
        }
        Ok(())
    }
}
impl DatabaseClaim {
    pub fn validate(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        self.spec.validate()?;
        self.lease.validate()?;
        let remaining = self
            .lease_until
            .signed_duration_since(now)
            .num_milliseconds();
        if self.lease.operation_id != self.spec.operation_id
            || self.lease.generation != self.spec.agent_generation
            || !(1..=300000).contains(&remaining)
        {
            return Err(invalid());
        }
        if self.material.object_store.is_some()
            != (self.spec.backup.is_some() || self.spec.recovery.is_some())
        {
            return Err(invalid());
        }
        if let Some(s) = &self.material.object_store
            && (s.read_only != self.spec.recovery.is_some()
                || !(3..=128).contains(&s.access_key_id.len())
                || !(16..=512).contains(&s.secret_access_key.len())
                || s.access_key_id.chars().any(char::is_control)
                || s.secret_access_key.chars().any(char::is_control))
        {
            return Err(invalid());
        }
        if self.material.credentials.is_some() != self.spec.database.is_some()
            || self.material.transfer_password.is_some() != self.spec.migration.is_some()
        {
            return Err(invalid());
        }
        if let Some(c) = &self.material.credentials
            && (!password(&c.application_password) || !password(&c.migration_password))
        {
            return Err(invalid());
        }
        if self
            .material
            .transfer_password
            .as_ref()
            .is_some_and(|s| !password(s))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn password(s: &str) -> bool {
    s.len() == 43
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn node_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}
fn database_host(id: &str, host: &str) -> bool {
    let Some(domain) = host.strip_prefix(&format!("db-{id}.")) else {
        return false;
    };
    let labels = domain.split('.').collect::<Vec<_>>();
    labels.len() >= 2
        && labels.iter().all(|s| {
            !s.is_empty()
                && s.len() <= 63
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && labels
            .last()
            .is_some_and(|s| s.len() >= 2 && s.bytes().all(|b| b.is_ascii_lowercase()))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> DatabaseJobSpec {
        serde_json::from_value(json!({"version":1,"operationID":"100","agentGeneration":3,"operationGeneration":2,"clusterID":"101","action":"create","phase":"apply","revision":1,"previousPodUID":null,"backend":{"id":"102","kind":"dedicated","namespace":"wd-db-102","node":"fixture-node","poolID":"00000000-0000-4000-8000-000000000001","revision":1,"cnpgUID":null,"resources":{"cpuMillicores":500,"memoryBytes":1073741824,"volumeBytes":10737418240_u64},"host":"db-102.example.invalid","directPort":32000,"pooledPort":32001},"database":{"id":"103","kind":"dedicated","name":"db_103","revision":1,"credentialRevision":1,"connectionLimit":10},"members":[{"id":"103","projectID":"104","databaseName":"db_103","enabled":true}],"migration":null})).unwrap()
    }
    #[test]
    fn spec_is_independent_of_renewable_lease_material_and_enforces_original_placement() {
        let original = spec();
        original.validate().unwrap();
        let fingerprint = original.fingerprint().unwrap();
        let roundtrip: DatabaseJobSpec =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(fingerprint, roundtrip.fingerprint().unwrap());
        for kind in 0..5 {
            let mut bad = original.clone();
            match kind {
                0 => bad.backend.namespace = "foreign".into(),
                1 => bad.backend.direct_port = 5432,
                2 => bad.members.clear(),
                3 => bad.database.as_mut().unwrap().credential_revision = 0,
                _ => bad.backend.resources.memory_bytes = 1,
            };
            assert!(bad.validate().is_err());
        }
        let mut generation = original.clone();
        generation.operation_generation += 1;
        assert_ne!(generation.fingerprint().unwrap(), fingerprint);
        let now = chrono::Utc::now();
        let mut claim = DatabaseClaim {
            spec: original,
            lease: DatabaseLease {
                operation_id: "100".into(),
                generation: 3,
                lease_token: "ab".repeat(32),
            },
            lease_until: now + chrono::Duration::seconds(250),
            material: DatabaseMaterial {
                credentials: Some(DatabaseCredentials {
                    application_password: "a".repeat(43),
                    migration_password: "b".repeat(43),
                }),
                transfer_password: None,
                object_store: None,
            },
        };
        claim.validate(now).unwrap();
        assert!(
            !serde_json::to_string(&claim.spec)
                .unwrap()
                .contains("Password")
        );
        claim.lease_until = now + chrono::Duration::seconds(200);
        assert_eq!(claim.spec.fingerprint().unwrap(), fingerprint);
        claim.lease_until = now + chrono::Duration::seconds(301);
        assert!(claim.validate(now).is_err());
    }
}
