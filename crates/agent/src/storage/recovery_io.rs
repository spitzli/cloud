//! Native quota-backed ntfy data snapshots/install. Linked only after schema/compiler release.
//! All paths derive from the existing pool and real native operation/resource IDs.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryReservationRef {
    pub reservation_id: String,
    pub grant_sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationRecoveryPacket {
    #[serde(rename = "operationID")]
    pub operation_id: RecordId,
    pub generation: u64,
    pub source: StoragePacket,
    pub target: Option<StoragePacket>,
    pub source_uuid: String,
    pub manifest_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging_origin: Option<ApplicationStagingOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation: Option<RecoveryReservationRef>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ApplicationStagingOrigin {
    OnlineRestore {
        operation_id: RecordId,
        generation: u64,
        target_nonce: String,
    },
    OperatorCapsule {
        target_nonce: String,
        admitted_set_id: RecordId,
    },
}
impl ApplicationRecoveryPacket {
    pub fn staging_relative_path(&self) -> Result<PathBuf> {
        if self.target.is_none() {
            if self.staging_origin.is_some() {
                return Err(ExecutionError::Ownership);
            }
            return Ok(PathBuf::from(format!(
                "{}-{}",
                self.operation_id, self.generation
            )));
        }
        let (nonce, name) = match self
            .staging_origin
            .as_ref()
            .ok_or(ExecutionError::Ownership)?
        {
            ApplicationStagingOrigin::OnlineRestore {
                operation_id,
                generation,
                target_nonce,
            } => {
                if operation_id != &self.operation_id
                    || *generation != self.generation
                    || !(1..=2147483646).contains(generation)
                {
                    return Err(ExecutionError::Ownership);
                }
                (target_nonce, format!("online-{operation_id}-{generation}"))
            }
            ApplicationStagingOrigin::OperatorCapsule {
                target_nonce,
                admitted_set_id,
            } => (target_nonce, format!("capsule-{admitted_set_id}")),
        };
        if !uuid::Uuid::parse_str(nonce).is_ok_and(|id| !id.is_nil() && id.to_string() == *nonce) {
            return Err(ExecutionError::Ownership);
        }
        Ok(PathBuf::from("restore").join(nonce).join(name))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationFileReceipt {
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationManifest {
    pub version: u8,
    pub profile: String,
    pub source: VolumeRecord,
    pub files: BTreeMap<String, ApplicationFileReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationRecoveryResult {
    pub directory: PathBuf,
    pub manifest_sha256: String,
    pub volume: VolumeRecord,
}
fn digest_file(path: &Path, guard: &StorageGuard) -> Result<ApplicationFileReceipt> {
    let mut stream = io(OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path))?;
    let m = io(stream.metadata())?;
    if !m.is_file() || m.nlink() != 1 || m.len() > recovery::MAX_RECOVERY_BYTES {
        return Err(ExecutionError::Ownership);
    }
    let mut total = 0;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        guard.check()?;
        let n = io(stream.read(&mut buffer))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > m.len() {
            return Err(ExecutionError::Ownership);
        }
        hash.update(&buffer[..n]);
    }
    if total != m.len() {
        return Err(ExecutionError::Ownership);
    }
    Ok(ApplicationFileReceipt {
        bytes: total,
        sha256: format!("{:x}", hash.finalize()),
    })
}
fn data_paths(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    for entry in io(std::fs::read_dir(root))? {
        let entry = io(entry)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| ExecutionError::Ownership)?;
        if name == "attachments" {
            let meta = io(std::fs::symlink_metadata(entry.path()))?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(ExecutionError::Ownership);
            }
            for file in io(std::fs::read_dir(entry.path()))? {
                let file = io(file)?;
                let name = format!(
                    "attachments/{}",
                    file.file_name().to_str().ok_or(ExecutionError::Ownership)?
                );
                recovery::ntfy_data_path(Path::new(&name))?;
                found.push((name, file.path()));
            }
        } else if ["cache.db", "auth.db"].contains(&name.as_str()) {
            found.push((name, entry.path()));
        } else if [
            "cache.db-wal",
            "cache.db-shm",
            "cache.db-journal",
            "auth.db-wal",
            "auth.db-shm",
            "auth.db-journal",
        ]
        .contains(&name.as_str())
        {
            let meta = io(std::fs::symlink_metadata(entry.path()))?;
            if !meta.is_file() || meta.nlink() != 1 {
                return Err(ExecutionError::Ownership);
            }
        } else {
            return Err(ExecutionError::Invalid("unsupported ntfy data file"));
        }
        if found.len() > 10000 {
            return Err(ExecutionError::Invalid("ntfy attachment count"));
        }
    }
    if !found.iter().any(|(name, _)| name == "cache.db") {
        return Err(ExecutionError::Invalid("ntfy SQLite cache missing"));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}
fn copy_data(source: &Path, target: &Path, guard: &StorageGuard, uid: u32) -> Result<()> {
    let m = io(std::fs::symlink_metadata(source))?;
    if !m.is_file() || m.nlink() != 1 || m.file_type().is_symlink() {
        return Err(ExecutionError::Ownership);
    }
    let mut input = io(OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(source))?;
    let mut output = io(OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(target))?;
    let mut buffer = [0; 65536];
    loop {
        guard.check()?;
        let n = io(input.read(&mut buffer))?;
        if n == 0 {
            break;
        }
        io(output.write_all(&buffer[..n]))?;
    }
    io(output.sync_all())?;
    io(std::os::unix::fs::chown(target, Some(uid), Some(uid)))?;
    Ok(())
}
fn staging_private(path: &Path, packet: &ApplicationRecoveryPacket) -> Result<()> {
    if packet.reservation.is_none() {
        return private(path, true);
    }
    let m = io(std::fs::symlink_metadata(path))?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != 65532
        || m.gid() != 65532
        || m.mode() & 0o077 != 0
        || io(std::fs::canonicalize(path))? != path
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
fn staging_mkdir(path: &Path, packet: &ApplicationRecoveryPacket) -> Result<()> {
    mkdir(path)?;
    if packet.reservation.is_some() {
        io(std::os::unix::fs::chown(path, Some(65532), Some(65532)))?;
    }
    Ok(())
}
impl StoragePool {
    pub(super) fn recovery_source_authorized(
        &self,
        packet: &ApplicationRecoveryPacket,
        guard: &StorageGuard,
    ) -> Result<()> {
        let reference = packet.reservation.as_ref().ok_or(ExecutionError::Denied)?;
        let receipt = self.verify_recovery_reservation(
            &reference.reservation_id,
            &reference.grant_sha256,
            guard,
        )?;
        let owner = receipt
            .application_source
            .as_ref()
            .ok_or(ExecutionError::Denied)?;
        if receipt.action != webdock_agent_protocol::recovery::RecoveryAction::Backup
            || owner.app_id != packet.source.app_id
            || owner.project_id != packet.source.project_id
            || owner.cluster_id != packet.source.cluster_id
            || owner.revision != packet.source.revision
            || owner.volume_bytes != packet.source.spec.volume_bytes
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    pub fn application_recovery_staging(
        &self,
        packet: &ApplicationRecoveryPacket,
        guard: &StorageGuard,
    ) -> Result<PathBuf> {
        guard.check()?;
        let path = self.recovery_directory(packet)?;
        let base = if let Some(reference) = &packet.reservation {
            self.verify_recovery_reservation(
                &reference.reservation_id,
                &reference.grant_sha256,
                guard,
            )?
            .staging_directory
            .join("application")
        } else {
            self.config.root.join("_recovery")
        };
        if !base.exists() {
            staging_mkdir(&base, packet)?;
        }
        staging_private(&base, packet)?;
        if let Some(parent) = path.parent() {
            let mut current = base.clone();
            for part in parent
                .strip_prefix(&base)
                .map_err(|_| ExecutionError::Ownership)?
                .components()
            {
                if !matches!(part, std::path::Component::Normal(_)) {
                    return Err(ExecutionError::Ownership);
                }
                current.push(part);
                if !current.exists() {
                    staging_mkdir(&current, packet)?;
                }
                staging_private(&current, packet)?;
            }
        }
        staging_mkdir(&path, packet)?;
        Ok(path)
    }
    fn recovery_directory(&self, packet: &ApplicationRecoveryPacket) -> Result<PathBuf> {
        packet.source.validate()?;
        if self.recovery_peer_enabled() && packet.reservation.is_none() {
            return Err(ExecutionError::Denied);
        }
        if packet.generation == 0
            || packet.generation > 2147483646
            || !valid_uuid(&packet.source_uuid)
            || self.kind != StorageKind::Apps
        {
            return Err(ExecutionError::Invalid("application recovery packet"));
        }
        if let Some(reference) = &packet.reservation {
            let guard =
                StorageGuard::new(packet.source.lease_until, Arc::new(AtomicBool::new(false)));
            let receipt = self.verify_recovery_reservation(
                &reference.reservation_id,
                &reference.grant_sha256,
                &guard,
            )?;
            let owner = if packet.target.is_some() {
                receipt.application_target.as_ref()
            } else {
                receipt.application_source.as_ref()
            }
            .ok_or(ExecutionError::Ownership)?;
            let actual = packet.target.as_ref().unwrap_or(&packet.source);
            if owner.app_id != actual.app_id
                || owner.project_id != actual.project_id
                || owner.cluster_id != actual.cluster_id
                || owner.revision != actual.revision
                || owner.volume_bytes != actual.spec.volume_bytes
            {
                return Err(ExecutionError::Ownership);
            }
            match packet.staging_origin.as_ref() {
                Some(ApplicationStagingOrigin::OnlineRestore {
                    operation_id,
                    generation,
                    target_nonce,
                }) if receipt.origin == "core"
                    && receipt.operation_id.as_ref() == Some(operation_id)
                    && receipt.generation == Some(*generation)
                    && receipt.target_nonce == *target_nonce => {}
                Some(ApplicationStagingOrigin::OperatorCapsule {
                    target_nonce,
                    admitted_set_id,
                }) if receipt.origin == "operator-capsule"
                    && receipt.target_nonce == *target_nonce
                    && receipt.source_set_id.as_ref() == Some(admitted_set_id) => {}
                None if receipt.action
                    == webdock_agent_protocol::recovery::RecoveryAction::Backup
                    && packet.target.is_none() => {}
                _ => return Err(ExecutionError::Ownership),
            }
            return Ok(receipt
                .staging_directory
                .join("application")
                .join(packet.staging_relative_path()?));
        }
        Ok(self
            .config
            .root
            .join("_recovery")
            .join(packet.staging_relative_path()?))
    }
    /// The enrolled executor has already stopped/observed every source volume writer.
    /// Storage identity and real fixed-size filesystem are independently revalidated here.
    pub fn snapshot_application(
        &self,
        packet: &ApplicationRecoveryPacket,
        guard: &StorageGuard,
    ) -> Result<ApplicationRecoveryResult> {
        guard.check()?;
        if self.recovery_peer_enabled() {
            self.recovery_source_authorized(packet, guard)?;
        }
        self.volume_state(&packet.source, guard)?;
        let record = self
            .record(&packet.source.app_id)?
            .ok_or(ExecutionError::Ownership)?;
        if record.uuid != packet.source_uuid
            || record.bytes > recovery::MAX_RECOVERY_BYTES
            || packet.target.is_some()
        {
            return Err(ExecutionError::Ownership);
        }
        let staging_root = if let Some(reference) = &packet.reservation {
            self.verify_recovery_reservation(
                &reference.reservation_id,
                &reference.grant_sha256,
                guard,
            )?
            .staging_directory
            .join("application")
        } else {
            self.config.root.join("_recovery")
        };
        if !staging_root.exists() {
            staging_mkdir(&staging_root, packet)?;
        }
        staging_private(&staging_root, packet)?;
        let staging = self.recovery_directory(packet)?;
        // Exclusive creation means a crash/partial transfer requires reconciliation,
        // never reuses a directory whose completed receipt was not acknowledged.
        staging_mkdir(&staging, packet)?;
        let reserve = io(OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(staging.join(".reservation")))?;
        guard.check()?;
        io(reserve.allocate(record.bytes.saturating_mul(2)))?;
        let source = self
            .config
            .root
            .join(record.app_id.as_str())
            .join("mount/data");
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for (name, input) in data_paths(&source)? {
            guard.check()?;
            let output = staging.join(&name);
            if name.starts_with("attachments/") {
                let directory = staging.join("attachments");
                if !directory.exists() {
                    staging_mkdir(&directory, packet)?;
                }
                copy_data(
                    &input,
                    &output,
                    guard,
                    if packet.reservation.is_some() {
                        65532
                    } else {
                        0
                    },
                )?;
            } else {
                // SQLite's supported backup API through its maintained CLI, only
                // after all source writers are stopped. Never copy a live DB/WAL.
                let sql = format!(".backup '{}'", path_str(&output)?.replace('\'', "''"));
                checked(
                    "/usr/bin/sqlite3",
                    &["-readonly", path_str(&input)?, &sql],
                    guard,
                    60,
                )?;
                io(std::fs::set_permissions(
                    &output,
                    std::fs::Permissions::from_mode(0o600),
                ))?;
                if checked(
                    "/usr/bin/sqlite3",
                    &["-readonly", path_str(&output)?, "PRAGMA integrity_check;"],
                    guard,
                    30,
                )? != "ok"
                {
                    return Err(ExecutionError::Runtime);
                }
            }
            if packet.reservation.is_some() {
                io(std::os::unix::fs::chown(&output, Some(65532), Some(65532)))?;
            }
            let receipt = digest_file(&output, guard)?;
            total = total
                .checked_add(receipt.bytes)
                .ok_or(ExecutionError::Runtime)?;
            if total > record.bytes {
                return Err(ExecutionError::Invalid("application recovery capacity"));
            }
            files.insert(name, receipt);
        }
        let manifest = ApplicationManifest {
            version: 1,
            profile: recovery::NTFY_PROFILE.into(),
            source: record.clone(),
            files,
        };
        let manifest_path = staging.join("manifest.json");
        let bytes = serde_json::to_vec(&manifest).map_err(|_| ExecutionError::Runtime)?;
        if bytes.len() > 1024 * 1024 {
            return Err(ExecutionError::Invalid("application manifest size"));
        }
        let mut output = io(OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&manifest_path))?;
        io(output.write_all(&bytes))?;
        io(output.sync_all())?;
        if packet.reservation.is_some() {
            io(std::os::unix::fs::chown(
                &manifest_path,
                Some(65532),
                Some(65532),
            ))?;
        }
        io(reserve.set_len(0))?;
        io(reserve.sync_all())?;
        drop(reserve);
        io(std::fs::remove_file(staging.join(".reservation")))?;
        sync_directory(&staging)?;
        guard.check()?;
        Ok(ApplicationRecoveryResult {
            directory: staging,
            manifest_sha256: format!("{:x}", Sha256::digest(&bytes)),
            volume: record,
        })
    }
    /// Source data need not exist. Its recorded identity comes from the verified,
    /// Core-referenced manifest; the target keeps its own new ext4 UUID/owner record.
    pub fn install_application(
        &mut self,
        packet: &ApplicationRecoveryPacket,
        guard: &StorageGuard,
    ) -> Result<ApplicationRecoveryResult> {
        let staging = self.recovery_directory(packet)?;
        staging_private(&staging, packet)?;
        let manifest_path = staging.join("manifest.json");
        let receipt = digest_file(&manifest_path, guard)?;
        if Some(&receipt.sha256) != packet.manifest_sha256.as_ref() || receipt.bytes > 1024 * 1024 {
            return Err(ExecutionError::Ownership);
        }
        let manifest: ApplicationManifest = read_json(&manifest_path, 1024 * 1024)?;
        if manifest.version != 1
            || manifest.profile != recovery::NTFY_PROFILE
            || manifest.source.uuid != packet.source_uuid
            || manifest.files.len() > 10000
            || !manifest.files.contains_key("cache.db")
        {
            return Err(ExecutionError::Ownership);
        }
        manifest.source.authorize(&packet.source)?;
        let target = packet
            .target
            .as_ref()
            .ok_or(ExecutionError::Invalid("fresh recovery target"))?;
        target.validate()?;
        // Verify all staged bytes before allocating/writing a target filesystem.
        let mut total = 0u64;
        for (name, expected) in &manifest.files {
            recovery::ntfy_data_path(Path::new(name))?;
            let actual = digest_file(&staging.join(name), guard)?;
            if actual.bytes != expected.bytes || actual.sha256 != expected.sha256 {
                return Err(ExecutionError::Ownership);
            }
            total = total
                .checked_add(actual.bytes)
                .ok_or(ExecutionError::Runtime)?;
        }
        if total > target.spec.volume_bytes {
            return Err(ExecutionError::Invalid("application target capacity"));
        }
        // ensure() checks real pool allocation/capacity and creates a fresh quota-backed
        // filesystem before a single restored application byte is written.
        if self.record(&target.app_id)?.is_some()
            || self.config.root.join(target.app_id.as_str()).exists()
        {
            return Err(ExecutionError::Ownership);
        }
        let reserved = if let Some(reference) = &packet.reservation {
            let receipt = self.verify_recovery_reservation(
                &reference.reservation_id,
                &reference.grant_sha256,
                guard,
            )?;
            if receipt.peak.target_bytes != target.spec.volume_bytes {
                return Err(ExecutionError::Ownership);
            }
            target.spec.volume_bytes
        } else {
            0
        };
        let (_, data) = self.ensure_reserved(target, guard, reserved)?;
        let target_record = self
            .record(&target.app_id)?
            .ok_or(ExecutionError::Ownership)?;
        if let Some(reference) = &packet.reservation {
            self.commit_recovery_application_uuid(
                &reference.reservation_id,
                &reference.grant_sha256,
                &target_record,
            )?;
        }
        recovery::validate_fresh_target(
            &manifest.source,
            &packet.source,
            &target_record,
            target,
            manifest
                .source
                .bytes
                .saturating_add(target_record.bytes.saturating_mul(2)),
            guard,
        )?;
        if io(std::fs::read_dir(&data))?.next().is_some() {
            return Err(ExecutionError::Ownership);
        }
        let reservation = staging.join(".restore-reservation");
        let file = io(OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&reservation))?;
        io(file.allocate(target_record.bytes))?;
        for (name, expected) in &manifest.files {
            let output = data.join(name);
            if name.starts_with("attachments/") {
                let directory = data.join("attachments");
                if !directory.exists() {
                    mkdir(&directory)?;
                    io(std::os::unix::fs::chown(
                        &directory,
                        Some(65532),
                        Some(65532),
                    ))?;
                    io(std::fs::set_permissions(
                        &directory,
                        std::fs::Permissions::from_mode(0o700),
                    ))?;
                }
            }
            copy_data(&staging.join(name), &output, guard, 65532)?;
            if digest_file(&output, guard)?.sha256 != expected.sha256 {
                return Err(ExecutionError::Ownership);
            }
        }
        for name in ["cache.db", "auth.db"] {
            let path = data.join(name);
            if path.exists()
                && checked(
                    "/usr/bin/sqlite3",
                    &["-readonly", path_str(&path)?, "PRAGMA integrity_check;"],
                    guard,
                    30,
                )? != "ok"
            {
                return Err(ExecutionError::Runtime);
            }
        }
        guard.check()?;
        io(std::fs::remove_file(&reservation))?;
        sync_directory(&data)?;
        Ok(ApplicationRecoveryResult {
            directory: data,
            manifest_sha256: receipt.sha256,
            volume: target_record,
        })
    }
}

#[cfg(test)]
mod staging_tests {
    use super::*;
    fn packet() -> ApplicationRecoveryPacket {
        serde_json::from_value(serde_json::json!({"operationID":"123","generation":1,"source":{"appID":"456","projectID":"789","clusterID":"111","revision":1,"leaseUntil":(chrono::Utc::now()+chrono::Duration::seconds(60)),"spec":{"volumeBytes":67108864}},"target":null,"sourceUuid":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","manifestSha256":null})).unwrap()
    }
    #[test]
    fn restore_origin_never_reuses_retained_backup_staging_and_missing_origin_is_denied() {
        let backup = packet();
        assert_eq!(
            backup.staging_relative_path().unwrap(),
            PathBuf::from("123-1")
        );
        assert!(
            serde_json::to_value(&backup)
                .unwrap()
                .get("stagingOrigin")
                .is_none()
        );
        let mut restore = backup.clone();
        restore.target = Some(backup.source.clone());
        assert!(
            restore.staging_relative_path().is_err(),
            "restore may not default to source backup staging"
        );
        restore.operation_id = RecordId::parse("321").unwrap();
        restore.generation = 2;
        restore.staging_origin = Some(ApplicationStagingOrigin::OnlineRestore {
            operation_id: RecordId::parse("321").unwrap(),
            generation: 2,
            target_nonce: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".into(),
        });
        assert_ne!(
            restore.staging_relative_path().unwrap(),
            backup.staging_relative_path().unwrap()
        );
    }
}
