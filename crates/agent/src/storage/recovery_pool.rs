//! Installation recovery custody is separate from legacy application/project volume records.
use super::*;
use webdock_agent_protocol::recovery::{RecoveryAction, RecoveryProfile};
// Fixture diagnostics disclose only fixed variants, never error payloads.
#[cfg(any(feature = "docker", test))]
pub(crate) fn reservation_result_variant<T>(result: &Result<T>) -> &'static str {
    match result {
        Ok(_) => "Ok",
        Err(ExecutionError::Invalid(_)) => "Invalid",
        Err(ExecutionError::Denied) => "Denied",
        Err(ExecutionError::Expired) => "Expired",
        Err(ExecutionError::Fenced) => "Fenced",
        Err(ExecutionError::Conflict) => "Conflict",
        Err(ExecutionError::Ownership) => "Ownership",
        Err(ExecutionError::Unsupported(_)) => "Unsupported",
        Err(ExecutionError::Runtime) => "Runtime",
        Err(ExecutionError::Journal) => "Journal",
    }
}
#[cfg(any(feature = "docker", test))]
fn reservation_marker_line(
    actor: &'static str,
    stage: &'static str,
    result: &'static str,
) -> String {
    format!("recovery_component reservation actor={actor} stage={stage} result={result}")
}
#[cfg(feature = "docker")]
pub(crate) fn reservation_marker(actor: &'static str, stage: &'static str, result: &'static str) {
    if std::env::var("WEBDOCK_RECOVERY_COMPONENT_TEST").as_deref() == Ok("1") {
        eprintln!("{}", reservation_marker_line(actor, stage, result));
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryPoolIdentity {
    pub configuration: StorageConfig,
    #[serde(rename = "poolUID")]
    pub pool_uid: String,
    pub revision: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryStorageTrust {
    pub version: u8,
    #[serde(rename = "installationID")]
    pub installation_id: String,
    pub core_signers: PathBuf,
    pub operator_signers: PathBuf,
    pub postgres_tls_certificate: Option<PathBuf>,
    pub postgres_tls_key: Option<PathBuf>,
    pub verification_directory: PathBuf,
    pub agent_staging_directory: PathBuf,
    pub socket_directory: PathBuf,
    pub targets: Vec<RecoveryStorageTarget>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryStorageTarget {
    pub id: String,
    pub sha256: String,
    pub nonce: Option<String>,
    pub action: RecoveryAction,
    pub profile: RecoveryProfile,
    pub capacity_bytes: u64,
    pub volumes: BTreeMap<String, ApprovedRecoveryVolume>,
    pub application_target: Option<RecoveryApplicationOwner>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "role",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RecoveryVolumeRole {
    CoreLedgerPostgres {},
    PrivateMailPostgres {},
    IncomingCe {
        #[serde(rename = "originalCustomerID")]
        original_customer_id: RecordId,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovedRecoveryVolume {
    pub role: RecoveryVolumeRole,
    pub bytes: u64,
    pub image: String,
    pub uid: u32,
    pub gid: u32,
    pub cpu_millicores: u32,
    pub memory_bytes: u64,
    pub tmp_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryApplicationOwner {
    #[serde(rename = "appID")]
    pub app_id: RecordId,
    #[serde(rename = "projectID")]
    pub project_id: RecordId,
    #[serde(rename = "clusterID")]
    pub cluster_id: RecordId,
    pub revision: u64,
    pub volume_bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryPeak {
    pub encrypted_bytes: u64,
    pub plaintext_bytes: u64,
    pub extracted_bytes: u64,
    pub material_and_scratch_bytes: u64,
    pub target_bytes: u64,
}
impl RecoveryPeak {
    pub fn staging_bytes(&self) -> Result<u64> {
        [
            self.encrypted_bytes,
            self.plaintext_bytes,
            self.extracted_bytes,
            self.material_and_scratch_bytes,
        ]
        .into_iter()
        .try_fold(0u64, |n, v| {
            n.checked_add(v).ok_or(ExecutionError::Ownership)
        })
    }
    pub fn total(&self) -> Result<u64> {
        self.staging_bytes()?
            .checked_add(self.target_bytes)
            .ok_or(ExecutionError::Ownership)
    }
    pub fn validate(&self, ceiling: u64) -> Result<()> {
        if self.encrypted_bytes == 0
            || self.encrypted_bytes > webdock_object_storage::MAX_TRANSFER
            || self.plaintext_bytes < self.encrypted_bytes
            || self.extracted_bytes > ceiling
            || self.material_and_scratch_bytes < MIN
            || self.total()? > MAX
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "origin",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RecoveryReservationAuthority {
    Core {
        document: webdock_agent_protocol::recovery::custody::SignedDocument,
    },
    OperatorCapsule {
        document: webdock_agent_protocol::recovery::custody::SignedDocument,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryReserveRequest {
    pub authority: RecoveryReservationAuthority,
    pub target_profile_id: String,
    pub target_profile_sha256: String,
    pub target_nonce: String,
    pub pool: RecoveryPoolIdentity,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryVolumeGrant {
    pub version: u8,
    pub scope: String,
    pub profile: RecoveryProfile,
    pub action: RecoveryAction,
    #[serde(rename = "installationID")]
    pub installation_id: String,
    #[serde(rename = "operationID")]
    pub operation_id: RecordId,
    pub generation: u64,
    #[serde(rename = "policyID")]
    pub policy_id: RecordId,
    pub policy_revision: u64,
    pub agent_generation: u64,
    pub lease_hash: String,
    pub epoch: String,
    #[serde(rename = "reservationID")]
    pub reservation_id: RecordId,
    #[serde(rename = "sourceSetID")]
    pub source_set_id: Option<RecordId>,
    pub target_profile_id: String,
    pub target_profile_sha256: String,
    pub target_nonce: String,
    pub pool: RecoveryPoolIdentity,
    pub peak: RecoveryPeak,
    pub volumes: BTreeMap<String, RecoveryMailVolume>,
    pub application_target: Option<RecoveryApplicationOwner>,
    pub application_source: Option<RecoveryApplicationOwner>,
    pub expires_at: DateTime<Utc>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryMailVolume {
    #[serde(rename = "resourceID")]
    pub resource_id: RecordId,
    pub volume_bytes: u64,
    pub role: RecoveryVolumeRole,
    pub uid: u32,
    pub gid: u32,
    pub image: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryVolumeRecord {
    pub scope: String,
    #[serde(rename = "resourceID")]
    pub resource_id: String,
    pub role: RecoveryVolumeRole,
    pub uid: u32,
    pub gid: u32,
    #[serde(rename = "reservationID")]
    pub reservation_id: String,
    pub grant_sha256: String,
    pub target_nonce: String,
    pub pool: RecoveryPoolIdentity,
    pub bytes: u64,
    pub data_directory: PathBuf,
    pub uuid: String,
    pub state: String,
    pub data_sha256: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryReservationReceipt {
    pub origin: String,
    #[serde(rename = "reservationID")]
    pub reservation_id: String,
    pub grant_sha256: String,
    pub target_nonce: String,
    pub target_profile_sha256: String,
    pub pool: RecoveryPoolIdentity,
    pub peak: RecoveryPeak,
    pub staging_directory: PathBuf,
    pub staging_uuid: String,
    pub expires_at: DateTime<Utc>,
    #[serde(rename = "sourceSetID")]
    pub source_set_id: Option<RecordId>,
    pub profile: RecoveryProfile,
    pub action: RecoveryAction,
    pub operation_id: Option<RecordId>,
    pub generation: Option<u64>,
    pub application_source: Option<RecoveryApplicationOwner>,
    pub application_uuid: Option<String>,
    pub application_target: Option<RecoveryApplicationOwner>,
    pub volumes: BTreeMap<String, RecoveryVolumeRecord>,
    pub state: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryPoolState {
    version: u8,
    pool: RecoveryPoolIdentity,
    reservations: BTreeMap<String, RecoveryReservationReceipt>,
}
fn hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn resource(value: &str) -> bool {
    RecordId::parse(value).is_ok()
        || uuid::Uuid::parse_str(value).is_ok_and(|v| !v.is_nil() && v.to_string() == value)
}
fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn sha(value: &impl Serialize) -> Result<String> {
    Ok(format!(
        "{:x}",
        sha2::Sha256::digest(serde_json::to_vec(value).map_err(|_| ExecutionError::Ownership)?)
    ))
}
impl RecoveryStorageTrust {
    fn validate(&self) -> Result<()> {
        if self.version != 1
            || !slug(&self.installation_id)
            || self.targets.is_empty()
            || self.targets.len() > 16
        {
            return Err(ExecutionError::Ownership);
        }
        for file in [&self.core_signers, &self.operator_signers] {
            private(file, false)?;
        }
        private(&self.verification_directory, true)?;
        recovery_socket_parent(&self.socket_directory, false)?;
        let data = io(std::fs::symlink_metadata(&self.agent_staging_directory))?;
        if !self.agent_staging_directory.is_absolute()
            || data.file_type().is_symlink()
            || !data.is_dir()
            || data.uid() != 65532
            || data.mode() & 0o077 != 0
            || io(std::fs::canonicalize(&self.agent_staging_directory))?
                != self.agent_staging_directory
        {
            return Err(ExecutionError::Ownership);
        }
        let mut seen = std::collections::BTreeSet::new();
        for target in &self.targets {
            if !slug(&target.id)
                || !hex64(&target.sha256)
                || !seen.insert(&target.id)
                || target
                    .nonce
                    .as_ref()
                    .is_some_and(|nonce| !valid_uuid(nonce))
                || (target.action == RecoveryAction::Restore && target.nonce.is_none())
                || (target.action == RecoveryAction::Backup
                    && (target.nonce.is_some() || !target.volumes.is_empty()))
                || !(MIN..=webdock_object_storage::MAX_TRANSFER).contains(&target.capacity_bytes)
                || target.volumes.len() > 102
            {
                return Err(ExecutionError::Ownership);
            }
            for (name, volume) in &target.volumes {
                if !slug(name)
                    || !(MIN..=webdock_object_storage::MAX_TRANSFER).contains(&volume.bytes)
                    || !webdock_supervisor::image_digest(&volume.image)
                    || volume.uid == 0
                    || volume.gid == 0
                    || volume.uid > 65535
                    || volume.gid > 65535
                    || !(100..=8000).contains(&volume.cpu_millicores)
                    || !(134217728..=8589934592).contains(&volume.memory_bytes)
                    || !(1048576..=1073741824).contains(&volume.tmp_bytes)
                {
                    return Err(ExecutionError::Ownership);
                }
                if matches!(volume.role, RecoveryVolumeRole::IncomingCe { .. })
                    && (volume.uid != 65532 || volume.gid != 65532)
                {
                    return Err(ExecutionError::Ownership);
                }
            }
        }
        Ok(())
    }
}
impl StoragePool {
    pub fn with_recovery_trust(mut self, path: &Path) -> Result<Self> {
        if self.kind != StorageKind::Apps {
            return Err(ExecutionError::Unsupported("recovery installation pool"));
        }
        private(path, false)?;
        let trust: RecoveryStorageTrust = read_json(path, 1024 * 1024)?;
        trust.validate()?;
        let bytes = io(std::fs::read(path))?;
        self.recovery_trust = Some((
            path.into(),
            format!("{:x}", sha2::Sha256::digest(bytes)),
            trust,
        ));
        Ok(self)
    }
    pub(super) fn recovery_peer_enabled(&self) -> bool {
        self.recovery_trust.is_some()
    }
    pub(super) fn install_recovery_socket_permissions(&self, parent: &Path) -> Result<()> {
        let trust = self.active_recovery_trust()?;
        if parent != trust.socket_directory {
            return Err(ExecutionError::Ownership);
        }
        recovery_socket_parent(parent, false)?;
        io(std::os::unix::fs::chown(parent, Some(0), Some(65532)))?;
        io(std::fs::set_permissions(
            parent,
            std::fs::Permissions::from_mode(0o710),
        ))?;
        recovery_socket_parent(parent, true)
    }
    fn active_recovery_trust(&self) -> Result<&RecoveryStorageTrust> {
        let (path, expected, trust) =
            self.recovery_trust
                .as_ref()
                .ok_or(ExecutionError::Unsupported(
                    "recovery trust is not installed",
                ))?;
        private(path, false)?;
        if format!("{:x}", sha2::Sha256::digest(io(std::fs::read(path))?)) != *expected {
            return Err(ExecutionError::Fenced);
        }
        trust.validate()?;
        Ok(trust)
    }
    pub(super) fn validate_recovery_namespace(&self, name: &str) -> Result<()> {
        if self.kind != StorageKind::Apps || !matches!(name, "_recovery-staging" | "_mail-recovery")
        {
            return Err(ExecutionError::Ownership);
        }
        let directory = self.config.root.join(name);
        private(&directory, true)?;
        // Do not recurse through recovery_state -> identity -> legacy inventory.
        let state: RecoveryPoolState = read_json(
            &self.config.root.join("_pool/recovery-custody.json"),
            1024 * 1024,
        )?;
        let identity = self.recovery_pool_identity_metadata()?;
        if state.version != 1 || state.pool != identity || state.reservations.len() > 1000 {
            return Err(ExecutionError::Ownership);
        }
        let mut expected = std::collections::BTreeSet::new();
        let mut required = std::collections::BTreeSet::new();
        let mut active = false;
        for (id, receipt) in &state.reservations {
            if !resource(id)
                || receipt.reservation_id != *id
                || receipt.pool != identity
                || !hex64(&receipt.grant_sha256)
                || !valid_uuid(&receipt.target_nonce)
                || !matches!(
                    receipt.state.as_str(),
                    "allocating" | "reserved" | "released"
                )
                || receipt.volumes.len() > 102
            {
                return Err(ExecutionError::Ownership);
            }
            let owned = receipt.state != "released";
            active |= owned;
            if name == "_recovery-staging" && owned {
                expected.insert(id.clone());
                if receipt.state == "reserved" {
                    required.insert(id.clone());
                }
            }
            for volume in receipt.volumes.values() {
                if !resource(&volume.resource_id)
                    || volume.pool != identity
                    || volume.reservation_id != *id
                    || volume.grant_sha256 != receipt.grant_sha256
                    || volume.target_nonce != receipt.target_nonce
                    || volume.scope != "installation-mail-recovery"
                    || volume.data_directory
                        != self
                            .config
                            .root
                            .join("_mail-recovery")
                            .join(&volume.resource_id)
                            .join("mount/data")
                {
                    return Err(ExecutionError::Ownership);
                }
                if name == "_mail-recovery" && owned {
                    if !expected.insert(volume.resource_id.clone()) {
                        return Err(ExecutionError::Ownership);
                    }
                    if receipt.state == "reserved" {
                        required.insert(volume.resource_id.clone());
                    }
                }
            }
        }
        // Empty namespaces are legitimate only once durable allocation custody exists.
        if !active {
            return Err(ExecutionError::Ownership);
        }
        for entry in io(std::fs::read_dir(&directory))? {
            let entry = io(entry)?;
            let child = entry
                .file_name()
                .into_string()
                .map_err(|_| ExecutionError::Ownership)?;
            if !expected.contains(&child) {
                return Err(ExecutionError::Ownership);
            }
            private(&entry.path(), true)?;
            required.remove(&child);
        }
        if !required.is_empty() {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    fn recovery_state(&self) -> Result<RecoveryPoolState> {
        let path = self.config.root.join("_pool/recovery-custody.json");
        if !path.try_exists().map_err(|_| ExecutionError::Runtime)? {
            return Ok(RecoveryPoolState {
                version: 1,
                pool: self.recovery_pool_identity()?,
                reservations: BTreeMap::new(),
            });
        }
        let state: RecoveryPoolState = read_json(&path, 1024 * 1024)?;
        if state.version != 1
            || state.pool != self.recovery_pool_identity()?
            || state.reservations.len() > 1000
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(state)
    }
    fn persist_recovery_state(&mut self, state: &RecoveryPoolState) -> Result<()> {
        let path = self.config.root.join("_pool/recovery-custody.json");
        let result = (|| -> Result<()> {
            let mut file =
                tempfile::NamedTempFile::new_in(path.parent().ok_or(ExecutionError::Ownership)?)
                    .map_err(|_| ExecutionError::Journal)?;
            io(file.write_all(&serde_json::to_vec(state).map_err(|_| ExecutionError::Journal)?))?;
            io(file.as_file().sync_all())?;
            file.persist(&path).map_err(|_| ExecutionError::Journal)?;
            sync_directory(path.parent().ok_or(ExecutionError::Ownership)?)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    pub(super) fn recovery_reserved_bytes(&self) -> Result<u64> {
        let legacy = self.state()?;
        self.recovery_state()?
            .reservations
            .values()
            .filter(|r| r.state != "released")
            .try_fold(0u64, |total, r| {
                let mut bytes = r.peak.total()?;
                if let (Some(owner), Some(uuid)) = (&r.application_target, &r.application_uuid) {
                    if let Some(record) = legacy.volumes.get(owner.app_id.as_str()) {
                        if record.uuid != *uuid
                            || record.project_id != owner.project_id
                            || record.cluster_id != owner.cluster_id
                            || record.bytes != owner.volume_bytes
                        {
                            return Err(ExecutionError::Ownership);
                        }
                        if record.state != VolumeState::Purged {
                            bytes = bytes
                                .checked_sub(owner.volume_bytes)
                                .ok_or(ExecutionError::Ownership)?;
                        }
                    } else {
                        return Err(ExecutionError::Ownership);
                    }
                }
                total.checked_add(bytes).ok_or(ExecutionError::Ownership)
            })
    }
    pub(super) fn commit_recovery_application_uuid(
        &mut self,
        id: &str,
        hash: &str,
        record: &VolumeRecord,
    ) -> Result<()> {
        let mut state = self.recovery_state()?;
        let receipt = state
            .reservations
            .get_mut(id)
            .ok_or(ExecutionError::Ownership)?;
        let owner = receipt
            .application_target
            .as_ref()
            .ok_or(ExecutionError::Ownership)?;
        if receipt.grant_sha256 != hash
            || receipt.application_uuid.is_some()
            || record.app_id != owner.app_id
            || record.project_id != owner.project_id
            || record.cluster_id != owner.cluster_id
            || record.bytes != owner.volume_bytes
            || record.state != VolumeState::Ready
        {
            return Err(ExecutionError::Ownership);
        }
        receipt.application_uuid = Some(record.uuid.clone());
        self.persist_recovery_state(&state)
    }
    #[cfg(feature = "docker")]
    fn recovery_capacity(&self, additional: u64) -> Result<()> {
        let legacy = self
            .state()?
            .volumes
            .values()
            .filter(|r| r.state != VolumeState::Purged)
            .try_fold(0u64, |n, r| {
                n.checked_add(r.bytes).ok_or(ExecutionError::Ownership)
            })?;
        let free = rustix::fs::statvfs(&self.config.root).map_err(|_| ExecutionError::Runtime)?;
        let free = free
            .f_bavail
            .checked_mul(free.f_frsize)
            .ok_or(ExecutionError::Ownership)?;
        if legacy
            .checked_add(self.recovery_reserved_bytes()?)
            .and_then(|n| n.checked_add(additional))
            .is_none_or(|n| n > self.config.capacity_bytes)
            || additional
                .checked_add(self.config.reserve_bytes)
                .is_none_or(|n| n > free)
        {
            return Err(ExecutionError::Invalid(
                "recovery peak capacity unavailable",
            ));
        }
        Ok(())
    }
    pub fn recovery_pool_identity(&self) -> Result<RecoveryPoolIdentity> {
        self.state()?;
        self.recovery_pool_identity_metadata()
    }
    // Used only after normal inventory/config checks or by the bounded custody reader below.
    fn recovery_pool_identity_metadata(&self) -> Result<RecoveryPoolIdentity> {
        let metadata = io(std::fs::symlink_metadata(&self.config.root))?;
        private(&self.config.root, true)?;
        Ok(RecoveryPoolIdentity {
            configuration: self.config.clone(),
            pool_uid: format!(
                "{}:{}:{}",
                self.config.pool_id,
                metadata.dev(),
                metadata.ino()
            ),
            revision: format!(
                "{:x}",
                sha2::Sha256::digest(
                    serde_json::to_vec(&self.config).map_err(|_| ExecutionError::Ownership)?
                )
            ),
        })
    }
}
use sha2::Digest;
#[cfg(test)]
mod tests {
    #[test]
    fn reservation_diagnostic_variants_never_format_error_payloads() {
        use super::*;
        for (error, expected) in [
            (ExecutionError::Invalid("PRIVATE-url-password"), "Invalid"),
            (ExecutionError::Unsupported("PRIVATE-token"), "Unsupported"),
            (ExecutionError::Denied, "Denied"),
            (ExecutionError::Expired, "Expired"),
            (ExecutionError::Fenced, "Fenced"),
            (ExecutionError::Conflict, "Conflict"),
            (ExecutionError::Ownership, "Ownership"),
            (ExecutionError::Runtime, "Runtime"),
            (ExecutionError::Journal, "Journal"),
        ] {
            let result: Result<()> = Err(error);
            let variant = reservation_result_variant(&result);
            assert_eq!(variant, expected);
            let line = reservation_marker_line("agent", "rpc_result", variant);
            assert!(!line.contains("PRIVATE"));
            assert!(!line.contains("password"));
        }
        assert_eq!(reservation_result_variant(&Ok(())), "Ok");
    }
    use super::*;
    fn namespace_fixture() -> (tempfile::TempDir, StoragePool, RecoveryPoolState) {
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("pool");
        mkdir(&root).unwrap();
        let config = StorageConfig {
            root: root.clone(),
            node: "namespace-unit".into(),
            cluster_id: RecordId::parse("789").unwrap(),
            pool_id: uuid::Uuid::new_v4().to_string(),
            capacity_bytes: 1073741824,
            reserve_bytes: MIN,
        };
        let config_path = temp.path().join("pool.json");
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        StoragePool::initialize(&config_path).unwrap();
        let mut pool = StoragePool::open(&config_path).unwrap();
        let identity = pool.recovery_pool_identity().unwrap();
        let hash = "a".repeat(64);
        let nonce = uuid::Uuid::new_v4().to_string();
        let volume = RecoveryVolumeRecord {
            scope: "installation-mail-recovery".into(),
            resource_id: "456".into(),
            role: RecoveryVolumeRole::CoreLedgerPostgres {},
            uid: 70,
            gid: 70,
            reservation_id: "123".into(),
            grant_sha256: hash.clone(),
            target_nonce: nonce.clone(),
            pool: identity.clone(),
            bytes: MIN,
            data_directory: root.join("_mail-recovery/456/mount/data"),
            uuid: uuid::Uuid::new_v4().to_string(),
            state: "empty".into(),
            data_sha256: None,
        };
        let receipt = RecoveryReservationReceipt {
            origin: "core-policy".into(),
            reservation_id: "123".into(),
            grant_sha256: hash,
            target_nonce: nonce,
            target_profile_sha256: "b".repeat(64),
            pool: identity.clone(),
            peak: RecoveryPeak {
                encrypted_bytes: MIN,
                plaintext_bytes: MIN,
                extracted_bytes: MIN,
                material_and_scratch_bytes: MIN,
                target_bytes: MIN,
            },
            staging_directory: temp.path().join("stage/123/data"),
            staging_uuid: uuid::Uuid::new_v4().to_string(),
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            source_set_id: None,
            profile: RecoveryProfile::MailInstallationV1,
            action: RecoveryAction::Restore,
            operation_id: Some(RecordId::parse("111").unwrap()),
            generation: Some(1),
            application_source: None,
            application_uuid: None,
            application_target: None,
            volumes: BTreeMap::from([("core".into(), volume)]),
            state: "allocating".into(),
        };
        let state = RecoveryPoolState {
            version: 1,
            pool: identity,
            reservations: BTreeMap::from([("123".into(), receipt)]),
        };
        pool.persist_recovery_state(&state).unwrap();
        (temp, pool, state)
    }
    #[test]
    #[ignore = "owned metadata-only filesystem test; run in isolated user namespace, no provider/mount"]
    fn recovery_namespace_durable_partial_allocation_reopens_without_adoption() {
        let (_temp, pool, _state) = namespace_fixture();
        let config_path = pool.config_path.clone();
        let root = pool.config.root.clone();
        for name in ["_recovery-staging", "_mail-recovery"] {
            mkdir(&root.join(name)).unwrap();
        }
        // Real durable receipt precedes directories exactly as reserve_recovery does.
        pool.state()
            .expect("recognized recovery namespaces must not poison legacy inventory");
        mkdir(&root.join("_recovery-staging/123")).unwrap();
        mkdir(&root.join("_mail-recovery/456")).unwrap();
        drop(pool);
        let reopened = StoragePool::open(&config_path).unwrap();
        assert_eq!(
            reopened.recovery_state().unwrap().reservations["123"].state,
            "allocating"
        );
        assert!(reopened.state().unwrap().volumes.is_empty());
    }
    #[test]
    #[ignore = "owned metadata-only filesystem test; run in isolated user namespace, no provider/mount"]
    fn recovery_namespace_inventory_denies_unowned_or_ambiguous_entries() {
        for case in [
            "unknown-root",
            "prefix",
            "unknown-stage",
            "unknown-mail",
            "child-file",
            "child-symlink",
            "namespace-symlink",
            "namespace-mode",
            "child-mode",
            "missing-custody",
            "corrupt-custody",
            "linked-custody",
            "empty-custody",
            "foreign-pool",
            "foreign-volume",
            "changed-config",
            "database-kind",
            "reserved-missing-child",
            "released-leftovers",
        ] {
            let (_temp, mut pool, mut state) = namespace_fixture();
            let root = pool.config.root.clone();
            let journal = root.join("_pool/recovery-custody.json");
            mkdir(&root.join("_recovery-staging")).unwrap();
            mkdir(&root.join("_mail-recovery")).unwrap();
            match case {
                "unknown-root" => mkdir(&root.join("foreign")).unwrap(),
                "prefix" => mkdir(&root.join("_recovery-staging-foreign")).unwrap(),
                "unknown-stage" => mkdir(&root.join("_recovery-staging/999")).unwrap(),
                "unknown-mail" => mkdir(&root.join("_mail-recovery/999")).unwrap(),
                "child-file" => {
                    std::fs::write(root.join("_recovery-staging/123"), b"foreign").unwrap()
                }
                "child-symlink" => std::os::unix::fs::symlink(
                    root.join("_pool"),
                    root.join("_recovery-staging/123"),
                )
                .unwrap(),
                "namespace-symlink" => {
                    std::fs::remove_dir(root.join("_mail-recovery")).unwrap();
                    std::os::unix::fs::symlink(root.join("_pool"), root.join("_mail-recovery"))
                        .unwrap();
                }
                "namespace-mode" => std::fs::set_permissions(
                    root.join("_mail-recovery"),
                    std::fs::Permissions::from_mode(0o750),
                )
                .unwrap(),
                "child-mode" => {
                    mkdir(&root.join("_recovery-staging/123")).unwrap();
                    std::fs::set_permissions(
                        root.join("_recovery-staging/123"),
                        std::fs::Permissions::from_mode(0o750),
                    )
                    .unwrap();
                }
                "missing-custody" => std::fs::remove_file(&journal).unwrap(),
                "corrupt-custody" => std::fs::write(&journal, b"corrupt").unwrap(),
                "linked-custody" => {
                    std::fs::hard_link(&journal, root.join("_pool/other-link")).unwrap()
                }
                "empty-custody" => {
                    state.reservations.clear();
                    pool.persist_recovery_state(&state).unwrap();
                }
                "foreign-pool" => {
                    state.pool.pool_uid.push_str("foreign");
                    pool.persist_recovery_state(&state).unwrap();
                }
                "foreign-volume" => {
                    state
                        .reservations
                        .get_mut("123")
                        .unwrap()
                        .volumes
                        .get_mut("core")
                        .unwrap()
                        .reservation_id = "999".into();
                    pool.persist_recovery_state(&state).unwrap();
                }
                "changed-config" => {
                    let mut config = pool.config.clone();
                    config.pool_id = uuid::Uuid::new_v4().to_string();
                    std::fs::write(&pool.config_path, serde_json::to_vec(&config).unwrap())
                        .unwrap();
                }
                "database-kind" => pool.kind = StorageKind::Database,
                "reserved-missing-child" => {
                    state.reservations.get_mut("123").unwrap().state = "reserved".into();
                    pool.persist_recovery_state(&state).unwrap();
                }
                "released-leftovers" => {
                    state.reservations.get_mut("123").unwrap().state = "released".into();
                    pool.persist_recovery_state(&state).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(pool.state().is_err(), "must reject {case}");
        }
    }
    #[test]
    fn recovery_namespace_wrong_real_owner_is_denied() {
        if rustix::process::geteuid().as_raw() != 0 {
            let root = tempfile::tempdir().unwrap();
            mkdir(&root.path().join("_recovery-staging")).unwrap();
            let config = StorageConfig {
                root: root.path().into(),
                node: "owner-denial".into(),
                cluster_id: RecordId::parse("789").unwrap(),
                pool_id: uuid::Uuid::new_v4().to_string(),
                capacity_bytes: MIN,
                reserve_bytes: MIN,
            };
            let pool = StoragePool {
                kind: StorageKind::Apps,
                config,
                config_path: root.path().join("unused-config"),
                _lock: tempfile::tempfile().unwrap(),
                poisoned: false,
                recovery_trust: None,
            };
            assert!(matches!(
                pool.validate_recovery_namespace("_recovery-staging"),
                Err(ExecutionError::Ownership)
            ));
        }
    }
    #[test]
    fn postgres_roles_reject_extra_customer_project_and_arbitrary_fields() {
        let mut accepted = Vec::new();
        for role in ["core-ledger-postgres", "private-mail-postgres"] {
            for (field, value) in [
                ("originalCustomerID", serde_json::json!("123")),
                ("customerID", serde_json::json!("123")),
                ("projectID", serde_json::json!("456")),
                ("arbitrary", serde_json::json!(true)),
            ] {
                let mut input = serde_json::json!({"role":role});
                input[field] = value;
                if serde_json::from_value::<RecoveryVolumeRole>(input).is_ok() {
                    accepted.push((role, field));
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "PG role variants accepted unknown fields: {accepted:?}"
        );
    }
    #[test]
    fn ce_role_requires_exact_original_customer_record_id_and_rejects_extra_fields() {
        let valid = serde_json::json!({"role":"incoming-ce","originalCustomerID":"123"});
        serde_json::from_value::<RecoveryVolumeRole>(valid.clone()).unwrap();
        for input in [
            serde_json::json!({"role":"incoming-ce"}),
            serde_json::json!({"role":"incoming-ce","originalCustomerId":"123"}),
            serde_json::json!({"role":"incoming-ce","originalCustomerID":123}),
            serde_json::json!({"role":"incoming-ce","originalCustomerID":"0"}),
            serde_json::json!({"role":"incoming-ce","originalCustomerID":""}),
            serde_json::json!({"role":"incoming-ce","originalCustomerID":"../123"}),
        ] {
            assert!(serde_json::from_value::<RecoveryVolumeRole>(input).is_err());
        }
        for field in ["customerID", "projectID", "arbitrary"] {
            let mut input = valid.clone();
            input[field] = serde_json::json!("unexpected");
            assert!(serde_json::from_value::<RecoveryVolumeRole>(input).is_err());
        }
    }
    #[test]
    fn recovery_role_valid_json_roundtrips_without_wire_changes() {
        for input in [
            serde_json::json!({"role":"core-ledger-postgres"}),
            serde_json::json!({"role":"private-mail-postgres"}),
            serde_json::json!({"role":"incoming-ce","originalCustomerID":"123"}),
        ] {
            let role: RecoveryVolumeRole = serde_json::from_value(input.clone()).unwrap();
            assert_eq!(serde_json::to_value(role).unwrap(), input);
        }
    }
    #[test]
    #[ignore = "owned privileged filesystem primitive only; no Core grant or full recovery acceptance"]
    fn owned_filesystem_uses_fresh_uuid_actual_uid70_and_real_byte_ceiling() {
        assert_eq!(
            std::env::var("WEBDOCK_RECOVERY_PHYSICAL_UNIT").as_deref(),
            Ok("yes")
        );
        assert!(Path::new("/.dockerenv").is_file());
        assert_eq!(rustix::process::geteuid().as_raw(), 0);
        let root = PathBuf::from(format!("/owned/recovery-fs-{}", uuid::Uuid::new_v4()));
        mkdir(&root).unwrap();
        let pool_path = root.join("pool");
        mkdir(&pool_path).unwrap();
        let config = root.join("config.json");
        let value = serde_json::json!({"root":pool_path,"node":"physical-unit","clusterID":"789","poolID":uuid::Uuid::new_v4().to_string(),"capacityBytes":268435456,"reserveBytes":67108864});
        std::fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        StoragePool::initialize(&config).unwrap();
        let pool = StoragePool::open(&config).unwrap();
        let directory = pool_path.join("typed-pg");
        let uuid = uuid::Uuid::new_v4().to_string();
        let guard = StorageGuard::new(
            Utc::now() + chrono::Duration::seconds(90),
            Arc::new(AtomicBool::new(false)),
        );
        let data = pool
            .fresh_recovery_filesystem(&directory, None, MIN, &uuid, (70, 70), &guard)
            .unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.verify_recovery_filesystem(&directory, &data, MIN, &uuid, (70, 70), &guard)
                .unwrap();
            assert!(
                pool.verify_recovery_filesystem(&directory, &data, MIN, &uuid, (26, 26), &guard)
                    .is_err()
            );
            assert!(
                pool.verify_recovery_filesystem(
                    &directory,
                    &data,
                    MIN,
                    "ffffffff-ffff-4fff-8fff-ffffffffffff",
                    (70, 70),
                    &guard
                )
                .is_err()
            );
            let file = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(data.join("bounded-data"))
                .unwrap();
            std::os::unix::fs::chown(data.join("bounded-data"), Some(70), Some(70)).unwrap();
            use std::os::unix::process::CommandExt;
            let outcome = Command::new("/usr/bin/dd")
                .args([
                    "if=/dev/zero",
                    "of=/proc/self/fd/0",
                    "bs=1048576",
                    "count=80",
                    "conv=fsync",
                    "status=none",
                ])
                .env("LC_ALL", "C")
                .uid(70)
                .gid(70)
                .stdin(std::process::Stdio::from(file))
                .output()
                .unwrap();
            assert!(!outcome.status.success());
            assert!(String::from_utf8_lossy(&outcome.stderr).contains("No space left on device"));
            assert!(std::fs::metadata(data.join("bounded-data")).unwrap().len() < MIN);
        }));
        let mount = directory.join("mount");
        assert!(
            Command::new("/usr/bin/umount")
                .arg(&mount)
                .status()
                .unwrap()
                .success()
        );
        let remaining = Command::new("/usr/sbin/losetup")
            .arg("--associated")
            .arg(directory.join("volume.ext4"))
            .output()
            .unwrap();
        assert!(
            remaining.status.success() && remaining.stdout.is_empty(),
            "only own loop association must be absent after unmount"
        );
        drop(pool);
        std::fs::remove_dir_all(&root).unwrap();
        assert!(!root.exists());
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
    #[test]
    fn peak_covers_simultaneous_cipher_plain_extracted_scratch_and_target_bytes() {
        let peak = RecoveryPeak {
            encrypted_bytes: MIN,
            plaintext_bytes: MIN,
            extracted_bytes: MIN,
            material_and_scratch_bytes: MIN,
            target_bytes: MIN,
        };
        assert_eq!(peak.total().unwrap(), 5 * MIN);
        peak.validate(MIN).unwrap();
        let overflowing = RecoveryPeak {
            encrypted_bytes: MIN,
            plaintext_bytes: u64::MAX,
            extracted_bytes: MIN,
            material_and_scratch_bytes: MIN,
            target_bytes: MIN,
        };
        assert!(overflowing.total().is_err());
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryVolumeRequest {
    pub reservation_id: String,
    pub grant_sha256: String,
    pub role_name: String,
}
#[cfg(feature = "docker")]
#[derive(Clone)]
struct AdmittedPlan {
    origin: String,
    id: String,
    grant_sha256: String,
    nonce: String,
    target_sha256: String,
    pool: RecoveryPoolIdentity,
    peak: RecoveryPeak,
    expires_at: DateTime<Utc>,
    source_set_id: Option<RecordId>,
    volumes: BTreeMap<String, (String, ApprovedRecoveryVolume)>,
    application_target: Option<RecoveryApplicationOwner>,
    application_source: Option<RecoveryApplicationOwner>,
    profile: RecoveryProfile,
    action: RecoveryAction,
    operation_id: Option<RecordId>,
    generation: Option<u64>,
}
impl StoragePool {
    #[cfg(feature = "docker")]
    fn admitted_recovery_plan(&self, request: &RecoveryReserveRequest) -> Result<AdmittedPlan> {
        let trust = self.active_recovery_trust()?;
        let pool = self.recovery_pool_identity()?;
        if request.pool != pool {
            return Err(ExecutionError::Ownership);
        }
        let target = trust
            .targets
            .iter()
            .find(|target| {
                target.id == request.target_profile_id
                    && target.sha256 == request.target_profile_sha256
                    && target
                        .nonce
                        .as_ref()
                        .is_none_or(|nonce| nonce == &request.target_nonce)
            })
            .ok_or(ExecutionError::Ownership)?;
        let grant_sha256 = sha(&request.authority)?;
        let plan = match &request.authority {
            RecoveryReservationAuthority::Core { document } => {
                let grant: RecoveryVolumeGrant = crate::recovery::capsule::verify(
                    document,
                    &trust.core_signers,
                    "webdock-core-recovery-reservation",
                    &trust.verification_directory,
                    "webdock-recovery-reservation-v1",
                )?;
                if grant.version != 1
                    || grant.scope
                        != match (target.profile, target.action) {
                            (RecoveryProfile::MailInstallationV1, RecoveryAction::Restore) => {
                                "installation-mail-recovery"
                            }
                            (RecoveryProfile::MailInstallationV1, RecoveryAction::Backup) => {
                                "installation-mail-backup"
                            }
                            (_, RecoveryAction::Restore) => "application-recovery",
                            (_, RecoveryAction::Backup) => "application-backup",
                        }
                    || grant.profile != target.profile
                    || grant.action != target.action
                    || grant.installation_id != trust.installation_id
                    || grant.pool != pool
                    || grant.target_profile_id != target.id
                    || grant.target_profile_sha256 != target.sha256
                    || target
                        .nonce
                        .as_ref()
                        .is_some_and(|nonce| grant.target_nonce != *nonce)
                    || !valid_uuid(&grant.target_nonce)
                    || !(1..=2147483646).contains(&grant.generation)
                    || !(1..=2147483646).contains(&grant.agent_generation)
                    || grant.policy_revision == 0
                    || !hex64(&grant.lease_hash)
                    || !valid_uuid(&grant.epoch)
                    || (target.action == RecoveryAction::Restore && grant.source_set_id.is_none())
                    || (target.action == RecoveryAction::Backup && grant.source_set_id.is_some())
                    || grant.expires_at <= Utc::now()
                    || grant.expires_at > Utc::now() + chrono::Duration::seconds(300)
                    || grant.volumes.len() != target.volumes.len()
                {
                    return Err(ExecutionError::Ownership);
                }
                grant.peak.validate(target.capacity_bytes)?;
                let mut volumes = BTreeMap::new();
                for (name, approved) in &target.volumes {
                    let granted = grant.volumes.get(name).ok_or(ExecutionError::Ownership)?;
                    if granted.role != approved.role
                        || granted.volume_bytes != approved.bytes
                        || granted.uid != approved.uid
                        || granted.gid != approved.gid
                        || granted.image != approved.image
                    {
                        return Err(ExecutionError::Ownership);
                    }
                    volumes.insert(
                        name.clone(),
                        (granted.resource_id.to_string(), approved.clone()),
                    );
                }
                AdmittedPlan {
                    origin: "core".into(),
                    id: grant.reservation_id.to_string(),
                    grant_sha256,
                    nonce: grant.target_nonce,
                    target_sha256: grant.target_profile_sha256,
                    pool,
                    peak: grant.peak,
                    expires_at: grant.expires_at,
                    source_set_id: grant.source_set_id,
                    volumes,
                    application_target: grant.application_target,
                    application_source: grant.application_source,
                    profile: grant.profile,
                    action: grant.action,
                    operation_id: Some(grant.operation_id),
                    generation: Some(grant.generation),
                }
            }
            RecoveryReservationAuthority::OperatorCapsule { document } => {
                let verified = crate::recovery::capsule::verify_capsule(
                    document,
                    &trust.core_signers,
                    &trust.operator_signers,
                    &target.id,
                    &target.sha256,
                    target.capacity_bytes,
                    &trust.verification_directory,
                    Utc::now().timestamp(),
                )?;
                if target.action != RecoveryAction::Restore
                    || target.nonce.as_ref() != Some(&verified.nonce)
                    || verified.set.profile != target.profile
                {
                    return Err(ExecutionError::Ownership);
                }
                let object: webdock_object_storage::ObjectReceipt = serde_json::from_value(
                    verified.set.proof["objects"]
                        .as_array()
                        .and_then(|objects| objects.first())
                        .cloned()
                        .ok_or(ExecutionError::Ownership)?,
                )
                .map_err(|_| ExecutionError::Ownership)?;
                let mut incoming = std::collections::BTreeSet::new();
                if target.profile == RecoveryProfile::MailInstallationV1 {
                    for source in verified.set.proof["ce"]
                        .as_array()
                        .ok_or(ExecutionError::Ownership)?
                    {
                        incoming.insert(
                            RecordId::parse(
                                source["customerID"]
                                    .as_str()
                                    .ok_or(ExecutionError::Ownership)?,
                            )
                            .map_err(|_| ExecutionError::Ownership)?
                            .to_string(),
                        );
                    }
                    let approved = target
                        .volumes
                        .values()
                        .filter_map(|volume| match &volume.role {
                            RecoveryVolumeRole::IncomingCe {
                                original_customer_id,
                            } => Some(original_customer_id.to_string()),
                            _ => None,
                        })
                        .collect::<std::collections::BTreeSet<_>>();
                    if incoming != approved
                        || target
                            .volumes
                            .values()
                            .filter(|v| matches!(v.role, RecoveryVolumeRole::CoreLedgerPostgres {}))
                            .count()
                            != 1
                        || target
                            .volumes
                            .values()
                            .filter(|v| {
                                matches!(v.role, RecoveryVolumeRole::PrivateMailPostgres {})
                            })
                            .count()
                            != 1
                    {
                        return Err(ExecutionError::Ownership);
                    }
                }
                let target_bytes = target
                    .volumes
                    .values()
                    .try_fold(0u64, |n, v| {
                        n.checked_add(v.bytes).ok_or(ExecutionError::Ownership)
                    })?
                    .checked_add(
                        target
                            .application_target
                            .as_ref()
                            .map_or(0, |a| a.volume_bytes),
                    )
                    .ok_or(ExecutionError::Ownership)?;
                let peak = RecoveryPeak {
                    encrypted_bytes: object.size_bytes,
                    plaintext_bytes: object.size_bytes,
                    extracted_bytes: verified.set.capacity_bytes,
                    material_and_scratch_bytes: MIN,
                    target_bytes,
                };
                peak.validate(target.capacity_bytes)?;
                let volumes = target
                    .volumes
                    .iter()
                    .map(|(name, volume)| {
                        (
                            name.clone(),
                            (uuid::Uuid::new_v4().to_string(), volume.clone()),
                        )
                    })
                    .collect();
                AdmittedPlan {
                    origin: "operator-capsule".into(),
                    id: uuid::Uuid::new_v4().to_string(),
                    grant_sha256,
                    nonce: verified.nonce,
                    target_sha256: target.sha256.clone(),
                    pool,
                    peak,
                    expires_at: DateTime::from_timestamp(verified.expires_at, 0)
                        .ok_or(ExecutionError::Expired)?,
                    source_set_id: Some(
                        RecordId::parse(&verified.set.set_id)
                            .map_err(|_| ExecutionError::Ownership)?,
                    ),
                    volumes,
                    application_target: target.application_target.clone(),
                    application_source: None,
                    profile: verified.set.profile,
                    action: RecoveryAction::Restore,
                    operation_id: None,
                    generation: None,
                }
            }
        };
        let volume_total = plan
            .volumes
            .values()
            .try_fold(0u64, |n, (_, v)| {
                n.checked_add(v.bytes).ok_or(ExecutionError::Ownership)
            })?
            .checked_add(
                plan.application_target
                    .as_ref()
                    .map_or(0, |a| a.volume_bytes),
            )
            .ok_or(ExecutionError::Ownership)?;
        if volume_total != plan.peak.target_bytes {
            return Err(ExecutionError::Ownership);
        }
        Ok(plan)
    }
    #[cfg(any(feature = "docker", test))]
    fn fresh_recovery_filesystem(
        &self,
        directory: &Path,
        staging_mount: Option<&Path>,
        bytes: u64,
        uuid: &str,
        owner: (u32, u32),
        guard: &StorageGuard,
    ) -> Result<PathBuf> {
        let (uid, gid) = owner;
        guard.check()?;
        if directory
            .try_exists()
            .map_err(|_| ExecutionError::Runtime)?
        {
            return Err(ExecutionError::Conflict);
        }
        mkdir(directory)?;
        let image = directory.join("volume.ext4");
        let file = io(OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&image))?;
        rustix::fs::fallocate(&file, rustix::fs::FallocateFlags::empty(), 0, bytes)
            .map_err(|_| ExecutionError::Runtime)?;
        io(file.sync_all())?;
        drop(file);
        checked(
            "/usr/sbin/mkfs.ext4",
            &[
                "-q",
                "-m",
                "0",
                "-U",
                uuid,
                "-E",
                "nodiscard,lazy_itable_init=0,lazy_journal_init=0",
                path_str(&image)?,
            ],
            guard,
            60,
        )?;
        let mount = staging_mount
            .map(Path::to_path_buf)
            .unwrap_or_else(|| directory.join("mount"));
        guard.check()?;
        mkdir(&mount)?;
        checked(
            "/usr/bin/mount",
            &[
                "-t",
                "ext4",
                "-o",
                "loop,nodev,nosuid,noexec",
                path_str(&image)?,
                path_str(&mount)?,
            ],
            guard,
            45,
        )?;
        let data = mount.join("data");
        mkdir(&data)?;
        io(std::fs::set_permissions(
            &data,
            std::fs::Permissions::from_mode(0o700),
        ))?;
        io(std::os::unix::fs::chown(&data, Some(uid), Some(gid)))?;
        checked("/usr/bin/sync", &["-f", path_str(&mount)?], guard, 30)?;
        sync_directory(directory)?;
        guard.check()?;
        Ok(data)
    }
    fn verify_recovery_filesystem(
        &self,
        directory: &Path,
        data: &Path,
        bytes: u64,
        uuid: &str,
        owner: (u32, u32),
        guard: &StorageGuard,
    ) -> Result<()> {
        let (uid, gid) = owner;
        private(directory, true)?;
        let image = directory.join("volume.ext4");
        private(&image, false)?;
        let image_meta = io(std::fs::symlink_metadata(&image))?;
        if image_meta.len() != bytes || image_meta.nlink() != 1 || !valid_uuid(uuid) {
            return Err(ExecutionError::Ownership);
        }
        if checked(
            "/usr/sbin/blkid",
            &["-s", "UUID", "-o", "value", path_str(&image)?],
            guard,
            10,
        )? != uuid
        {
            return Err(ExecutionError::Ownership);
        }
        let mount = data.parent().ok_or(ExecutionError::Ownership)?;
        let actual = checked(
            "/usr/bin/findmnt",
            &[
                "--json",
                "--mountpoint",
                path_str(mount)?,
                "--output",
                "SOURCE,FSTYPE,UUID",
            ],
            guard,
            10,
        )?;
        let actual: serde_json::Value =
            serde_json::from_str(&actual).map_err(|_| ExecutionError::Ownership)?;
        let entries = actual["filesystems"]
            .as_array()
            .ok_or(ExecutionError::Ownership)?;
        if entries.len() != 1 || entries[0]["fstype"] != "ext4" || entries[0]["uuid"] != uuid {
            return Err(ExecutionError::Ownership);
        }
        let device = entries[0]["source"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?;
        if !device
            .strip_prefix("/dev/loop")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(ExecutionError::Ownership);
        }
        let loops = checked(
            "/usr/sbin/losetup",
            &[
                "--json",
                "--list",
                "--output",
                "NAME,BACK-FILE,BACK-INO",
                device,
            ],
            guard,
            10,
        )?;
        let loops: serde_json::Value =
            serde_json::from_str(&loops).map_err(|_| ExecutionError::Ownership)?;
        let loops = loops["loopdevices"]
            .as_array()
            .ok_or(ExecutionError::Ownership)?;
        if loops.len() != 1
            || loops[0]["back-file"] != path_str(&image)?
            || loops[0]["back-ino"].as_u64() != Some(image_meta.ino())
        {
            return Err(ExecutionError::Ownership);
        }
        let owner = io(std::fs::symlink_metadata(data))?;
        if !owner.is_dir()
            || owner.file_type().is_symlink()
            || owner.uid() != uid
            || owner.gid() != gid
            || owner.mode() & 0o077 != 0
        {
            return Err(ExecutionError::Ownership);
        }
        guard.check()
    }
    pub fn verify_recovery_reservation(
        &self,
        id: &str,
        hash: &str,
        guard: &StorageGuard,
    ) -> Result<RecoveryReservationReceipt> {
        let trust = self.active_recovery_trust()?;
        if !resource(id) || !hex64(hash) {
            return Err(ExecutionError::Ownership);
        }
        let state = self.recovery_state()?;
        let receipt = state
            .reservations
            .get(id)
            .ok_or(ExecutionError::Ownership)?;
        if receipt.state != "reserved"
            || receipt.grant_sha256 != hash
            || receipt.pool != self.recovery_pool_identity()?
            || receipt.expires_at <= Utc::now()
        {
            return Err(ExecutionError::Fenced);
        }
        let stage_directory = self.config.root.join("_recovery-staging").join(id);
        if receipt.staging_directory != trust.agent_staging_directory.join(id).join("data") {
            return Err(ExecutionError::Ownership);
        }
        self.verify_recovery_filesystem(
            &stage_directory,
            &receipt.staging_directory,
            receipt.peak.staging_bytes()?,
            &receipt.staging_uuid,
            (65532, 65532),
            guard,
        )?;
        for volume in receipt.volumes.values() {
            let directory = self
                .config
                .root
                .join("_mail-recovery")
                .join(&volume.resource_id);
            if volume.scope != "installation-mail-recovery"
                || volume.reservation_id != id
                || volume.grant_sha256 != hash
                || volume.target_nonce != receipt.target_nonce
                || volume.data_directory != directory.join("mount/data")
                || volume.pool != receipt.pool
            {
                return Err(ExecutionError::Ownership);
            }
            self.verify_recovery_filesystem(
                &directory,
                &volume.data_directory,
                volume.bytes,
                &volume.uuid,
                (volume.uid, volume.gid),
                guard,
            )?;
        }
        Ok(receipt.clone())
    }
    #[cfg(feature = "docker")]
    pub fn reserve_recovery(
        &mut self,
        request: &RecoveryReserveRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<RecoveryReservationReceipt> {
        let plan = self.admitted_recovery_plan(request)?;
        reservation_marker("helper", "plan_verified", "Ok");
        let guard = StorageGuard::new(plan.expires_at, cancelled);
        guard.check()?;
        let mut state = self.recovery_state()?;
        if let Some(prior) = state
            .reservations
            .values()
            .find(|r| r.grant_sha256 == plan.grant_sha256)
        {
            return self.verify_recovery_reservation(
                &prior.reservation_id,
                &plan.grant_sha256,
                &guard,
            );
        }
        if state
            .reservations
            .values()
            .any(|r| r.target_nonce == plan.nonce)
            || state.reservations.len() >= 1000
        {
            return Err(ExecutionError::Conflict);
        }
        let trust = self.active_recovery_trust()?.clone();
        let legacy = self.state()?;
        if plan.application_target.as_ref().is_some_and(|owner| {
            legacy.volumes.contains_key(owner.app_id.as_str())
                || self.config.root.join(owner.app_id.as_str()).exists()
        }) {
            return Err(ExecutionError::Ownership);
        }
        for (id, _) in plan.volumes.values() {
            if !resource(id)
                || legacy.volumes.contains_key(id)
                || self
                    .config
                    .root
                    .join("_mail-recovery")
                    .join(id)
                    .try_exists()
                    .map_err(|_| ExecutionError::Runtime)?
            {
                return Err(ExecutionError::Ownership);
            }
        }
        let stage_mount = trust.agent_staging_directory.join(&plan.id);
        if !resource(&plan.id)
            || stage_mount
                .try_exists()
                .map_err(|_| ExecutionError::Runtime)?
            || self
                .config
                .root
                .join("_recovery-staging")
                .join(&plan.id)
                .try_exists()
                .map_err(|_| ExecutionError::Runtime)?
        {
            return Err(ExecutionError::Conflict);
        }
        self.recovery_capacity(plan.peak.total()?)?;
        let mut receipt = RecoveryReservationReceipt {
            origin: plan.origin,
            reservation_id: plan.id.clone(),
            grant_sha256: plan.grant_sha256.clone(),
            target_nonce: plan.nonce.clone(),
            target_profile_sha256: plan.target_sha256,
            pool: plan.pool.clone(),
            peak: plan.peak.clone(),
            staging_directory: stage_mount.join("data"),
            staging_uuid: uuid::Uuid::new_v4().to_string(),
            expires_at: plan.expires_at,
            source_set_id: plan.source_set_id,
            application_target: plan.application_target,
            application_source: plan.application_source,
            application_uuid: None,
            profile: plan.profile,
            action: plan.action,
            operation_id: plan.operation_id,
            generation: plan.generation,
            volumes: BTreeMap::new(),
            state: "allocating".into(),
        };
        for (name, (id, approved)) in &plan.volumes {
            receipt.volumes.insert(
                name.clone(),
                RecoveryVolumeRecord {
                    scope: "installation-mail-recovery".into(),
                    resource_id: id.clone(),
                    role: approved.role.clone(),
                    uid: approved.uid,
                    gid: approved.gid,
                    reservation_id: plan.id.clone(),
                    grant_sha256: plan.grant_sha256.clone(),
                    target_nonce: plan.nonce.clone(),
                    pool: plan.pool.clone(),
                    bytes: approved.bytes,
                    data_directory: self
                        .config
                        .root
                        .join("_mail-recovery")
                        .join(id)
                        .join("mount/data"),
                    uuid: uuid::Uuid::new_v4().to_string(),
                    state: "empty".into(),
                    data_sha256: None,
                },
            );
        }
        // Charge the entire signed/independently authorized peak before the first allocation.
        state.reservations.insert(plan.id.clone(), receipt.clone());
        self.persist_recovery_state(&state)?;
        for name in ["_recovery-staging", "_mail-recovery"] {
            let path = self.config.root.join(name);
            if !path.exists() {
                mkdir(&path)?;
            }
            private(&path, true)?;
        }
        self.fresh_recovery_filesystem(
            &self.config.root.join("_recovery-staging").join(&plan.id),
            Some(&stage_mount),
            plan.peak.staging_bytes()?,
            &receipt.staging_uuid,
            (65532, 65532),
            &guard,
        )?;
        for volume in receipt.volumes.values() {
            self.fresh_recovery_filesystem(
                &self
                    .config
                    .root
                    .join("_mail-recovery")
                    .join(&volume.resource_id),
                None,
                volume.bytes,
                &volume.uuid,
                (volume.uid, volume.gid),
                &guard,
            )?;
        }
        guard.check()?;
        receipt.state = "reserved".into();
        state.reservations.insert(plan.id.clone(), receipt.clone());
        self.persist_recovery_state(&state)?;
        self.verify_recovery_reservation(&plan.id, &plan.grant_sha256, &guard)
    }
}

pub(super) fn recovery_socket_parent(path: &Path, installed: bool) -> Result<()> {
    if !path.is_absolute() || io(std::fs::canonicalize(path))? != path {
        return Err(ExecutionError::Ownership);
    }
    let own = io(std::fs::symlink_metadata(path))?;
    let valid_mode = if installed {
        own.gid() == 65532 && own.mode() & 0o777 == 0o710
    } else {
        own.mode() & 0o777 == 0o700 || own.gid() == 65532 && own.mode() & 0o777 == 0o710
    };
    if !own.is_dir() || own.file_type().is_symlink() || own.uid() != 0 || !valid_mode {
        return Err(ExecutionError::Ownership);
    }
    for ancestor in path.ancestors().skip(1) {
        let metadata = io(std::fs::symlink_metadata(ancestor))?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
        {
            return Err(ExecutionError::Ownership);
        }
    }
    Ok(())
}
pub(super) fn recovery_action(action: &str) -> bool {
    matches!(
        action,
        "recovery.pool-info"
            | "recovery.reserve"
            | "recovery.record"
            | "recovery.install-ce"
            | "application.snapshot"
            | "application.install"
            | "application.record"
            | "application.staging"
    )
}

impl StoragePool {
    pub fn recovery_volume_record(
        &self,
        request: &RecoveryVolumeRequest,
        guard: &StorageGuard,
    ) -> Result<RecoveryVolumeRecord> {
        let receipt = self.verify_recovery_reservation(
            &request.reservation_id,
            &request.grant_sha256,
            guard,
        )?;
        let record = receipt
            .volumes
            .get(&request.role_name)
            .ok_or(ExecutionError::Ownership)?;
        if let Some(expected) = &record.data_sha256
            && recovery_tree_hash(&record.data_directory, guard, record.bytes)? != *expected
        {
            return Err(ExecutionError::Fenced);
        }
        Ok(record.clone())
    }
    pub fn install_recovery_volume(
        &mut self,
        request: &RecoveryVolumeRequest,
        guard: &StorageGuard,
    ) -> Result<RecoveryVolumeRecord> {
        let receipt = self.verify_recovery_reservation(
            &request.reservation_id,
            &request.grant_sha256,
            guard,
        )?;
        let record = receipt
            .volumes
            .get(&request.role_name)
            .ok_or(ExecutionError::Ownership)?
            .clone();
        if record.state == "installed" {
            return self.recovery_volume_record(request, guard);
        }
        if record.state != "empty"
            || io(std::fs::read_dir(&record.data_directory))?
                .next()
                .is_some()
        {
            return Err(ExecutionError::Fenced);
        }
        match &record.role {
            RecoveryVolumeRole::IncomingCe {
                original_customer_id,
            } => {
                let source = receipt
                    .staging_directory
                    .join("mail-restore/data/ce")
                    .join(original_customer_id.as_str());
                recovery_copy_tree(
                    &source,
                    &record.data_directory,
                    guard,
                    record.bytes,
                    record.uid,
                    record.gid,
                )?;
            }
            RecoveryVolumeRole::CoreLedgerPostgres {}
            | RecoveryVolumeRole::PrivateMailPostgres {} => {
                let trust = self.active_recovery_trust()?;
                let certificate = trust
                    .postgres_tls_certificate
                    .as_ref()
                    .ok_or(ExecutionError::Ownership)?;
                let key = trust
                    .postgres_tls_key
                    .as_ref()
                    .ok_or(ExecutionError::Ownership)?;
                private(certificate, false)?;
                private(key, false)?;
                let directory = record.data_directory.join(".operator-tls");
                mkdir(&directory)?;
                io(std::os::unix::fs::chown(
                    &directory,
                    Some(record.uid),
                    Some(record.gid),
                ))?;
                recovery_copy_file(
                    certificate,
                    &directory.join("server.crt"),
                    guard,
                    record.uid,
                    record.gid,
                    65536,
                )?;
                recovery_copy_file(
                    key,
                    &directory.join("server.key"),
                    guard,
                    record.uid,
                    record.gid,
                    65536,
                )?;
            }
        }
        guard.check()?;
        let mut installed = record;
        installed.data_sha256 = Some(recovery_tree_hash(
            &installed.data_directory,
            guard,
            installed.bytes,
        )?);
        installed.state = "installed".into();
        let mut state = self.recovery_state()?;
        state
            .reservations
            .get_mut(&request.reservation_id)
            .ok_or(ExecutionError::Ownership)?
            .volumes
            .insert(request.role_name.clone(), installed.clone());
        self.persist_recovery_state(&state)?;
        self.recovery_volume_record(request, guard)
    }
}
fn recovery_copy_file(
    source: &Path,
    target: &Path,
    guard: &StorageGuard,
    uid: u32,
    gid: u32,
    limit: u64,
) -> Result<()> {
    let mut input = io(OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(source))?;
    let metadata = io(input.metadata())?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > limit {
        return Err(ExecutionError::Ownership);
    }
    let mut output = io(OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(target))?;
    let mut count = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        guard.check()?;
        let n = io(input.read(&mut buffer))?;
        if n == 0 {
            break;
        }
        count = count
            .checked_add(n as u64)
            .ok_or(ExecutionError::Ownership)?;
        if count > limit {
            return Err(ExecutionError::Ownership);
        }
        io(output.write_all(&buffer[..n]))?;
    }
    if count != metadata.len() {
        return Err(ExecutionError::Ownership);
    }
    io(output.sync_all())?;
    io(std::os::unix::fs::chown(target, Some(uid), Some(gid)))?;
    Ok(())
}
fn recovery_copy_tree(
    source: &Path,
    target: &Path,
    guard: &StorageGuard,
    limit: u64,
    uid: u32,
    gid: u32,
) -> Result<()> {
    let mut pending = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut bytes = 0u64;
    let mut count = 0;
    while let Some((source, target)) = pending.pop() {
        guard.check()?;
        let own = io(std::fs::symlink_metadata(&source))?;
        if !own.is_dir() || own.file_type().is_symlink() {
            return Err(ExecutionError::Ownership);
        }
        for entry in io(std::fs::read_dir(&source))? {
            let entry = io(entry)?;
            count += 1;
            if count > 20000 {
                return Err(ExecutionError::Ownership);
            }
            let metadata = io(std::fs::symlink_metadata(entry.path()))?;
            let output = target.join(entry.file_name());
            if metadata.file_type().is_symlink() {
                return Err(ExecutionError::Ownership);
            }
            if metadata.is_dir() {
                mkdir(&output)?;
                io(std::os::unix::fs::chown(&output, Some(uid), Some(gid)))?;
                pending.push((entry.path(), output));
            } else {
                bytes = bytes
                    .checked_add(metadata.len())
                    .filter(|n| *n <= limit)
                    .ok_or(ExecutionError::Ownership)?;
                recovery_copy_file(&entry.path(), &output, guard, uid, gid, metadata.len())?;
            }
        }
    }
    sync_directory(target)
}
fn recovery_tree_hash(root: &Path, guard: &StorageGuard, limit: u64) -> Result<String> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut bytes = 0u64;
    while let Some(directory) = pending.pop() {
        guard.check()?;
        for entry in io(std::fs::read_dir(&directory))? {
            let entry = io(entry)?;
            let path = entry.path();
            let metadata = io(std::fs::symlink_metadata(&path))?;
            if metadata.file_type().is_symlink() {
                return Err(ExecutionError::Ownership);
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() || metadata.nlink() != 1 || entries.len() > 20000 {
                return Err(ExecutionError::Ownership);
            }
            bytes = bytes
                .checked_add(metadata.len())
                .filter(|n| *n <= limit)
                .ok_or(ExecutionError::Ownership)?;
            let mut stream = io(OpenOptions::new()
                .read(true)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(&path))?;
            let mut hash = sha2::Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                guard.check()?;
                let n = io(stream.read(&mut buffer))?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            entries.push((
                path.strip_prefix(root)
                    .map_err(|_| ExecutionError::Ownership)?
                    .to_path_buf(),
                metadata.len(),
                format!("{:x}", hash.finalize()),
            ));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    sha(&entries)
}
