//! Dedicated build pool: authenticated allocation and irreversible retirement.
//! Retirement fences future allocation; physical deletion is unavailable until
//! a node reconciler can prove exact Job/Pod/PV absence. No tenant RPC is exposed.
use super::*;
use ed25519_dalek::VerifyingKey;
use spitzli_agent_protocol::build_storage::{
    BuildStorageAction, BuildStorageReservation, BuildStorageTarget, SignedBuildStorageReservation,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildStoragePhase {
    Reserved,
    Retired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    reservation: BuildStorageReservation,
    phase: BuildStoragePhase,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildStorageLedger {
    entries: BTreeMap<String, Entry>,
}

fn invalid() -> ExecutionError {
    ExecutionError::Invalid("build storage authority")
}

impl BuildStorageLedger {
    fn validate(&self) -> Result<()> {
        for (id, entry) in &self.entries {
            entry.reservation.validate_shape().map_err(|_| invalid())?;
            if id != &entry.reservation.volume_id
                || (entry.phase == BuildStoragePhase::Retired)
                    != (entry.reservation.action == BuildStorageAction::Retire)
            {
                return Err(ExecutionError::Journal);
            }
        }
        Ok(())
    }

    /// Pure state transition, called only after signature/target/expiry verification.
    pub fn apply_verified(&mut self, request: &BuildStorageReservation) -> Result<()> {
        self.validate()?;
        request.validate_shape().map_err(|_| invalid())?;
        if let Some(entry) = self.entries.get(&request.volume_id) {
            if entry
                .reservation
                .allocation_fingerprint()
                .map_err(|_| invalid())?
                != request.allocation_fingerprint().map_err(|_| invalid())?
                || request.epoch < entry.reservation.epoch
            {
                return Err(ExecutionError::Ownership);
            }
            if request.action == BuildStorageAction::Inspect {
                if entry.phase == BuildStoragePhase::Retired
                    || request.epoch == entry.reservation.epoch
                {
                    return Ok(());
                }
                // A fresh read-only check advances the authority epoch as well;
                // this prevents an older reserve from being delivered afterwards.
                self.entries.insert(
                    request.volume_id.clone(),
                    Entry {
                        reservation: request.clone(),
                        phase: BuildStoragePhase::Reserved,
                    },
                );
                return Ok(());
            }
            if request.epoch == entry.reservation.epoch {
                return if request == &entry.reservation {
                    Ok(())
                } else {
                    Err(ExecutionError::Conflict)
                };
            }
            if entry.phase == BuildStoragePhase::Retired
                && request.action != BuildStorageAction::Retire
            {
                return Err(ExecutionError::Fenced);
            }
        } else if request.action == BuildStorageAction::Inspect {
            return Err(ExecutionError::Ownership);
        }
        self.entries.insert(
            request.volume_id.clone(),
            Entry {
                reservation: request.clone(),
                phase: if request.action == BuildStorageAction::Retire {
                    BuildStoragePhase::Retired
                } else {
                    BuildStoragePhase::Reserved
                },
            },
        );
        Ok(())
    }

    pub fn phase(&self, request: &BuildStorageReservation) -> Result<BuildStoragePhase> {
        self.validate()?;
        let entry = self
            .entries
            .get(&request.volume_id)
            .ok_or(ExecutionError::Ownership)?;
        if entry
            .reservation
            .allocation_fingerprint()
            .map_err(|_| invalid())?
            != request.allocation_fingerprint().map_err(|_| invalid())?
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(entry.phase)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildStorageTrust {
    pub version: u8,
    pub target: BuildStorageTarget,
    /// Ed25519 public verification key, hex encoded; never a private signing key.
    pub public_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildFilesystem {
    pub allocation_fingerprint: String,
    pub volume_bytes: u64,
    pub filesystem_uuid: String,
    /// Infrastructure-only attestation; never expose this path in the customer API.
    pub data_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildStorageResult {
    pub phase: BuildStoragePhase,
    pub filesystem: Option<BuildFilesystem>,
}

impl StoragePool {
    fn build_ledger(&self) -> Result<BuildStorageLedger> {
        if self.kind != StorageKind::Builds {
            return Err(ExecutionError::Denied);
        }
        self.state()?;
        let path = self.config.root.join("_pool/builds.json");
        let ledger = if path.try_exists().map_err(|_| ExecutionError::Runtime)? {
            read_json::<BuildStorageLedger>(&path, 64 * 1024 * 1024)?
        } else {
            // Never create fresh authority over an allocation whose journal disappeared.
            if !self.state()?.volumes.is_empty() {
                return Err(ExecutionError::Journal);
            }
            BuildStorageLedger::default()
        };
        ledger.validate()?;
        Ok(ledger)
    }

    pub fn apply_build_storage(
        &mut self,
        signed: &SignedBuildStorageReservation,
        trust: &BuildStorageTrust,
        guard: &StorageGuard,
    ) -> Result<BuildStorageResult> {
        guard.check()?;
        if self.kind != StorageKind::Builds || trust.version != 1 || trust.public_key.len() != 64 {
            return Err(ExecutionError::Denied);
        }
        let target = BuildStorageTarget {
            cluster_id: self.config.cluster_id.to_string(),
            node: self.config.node.clone(),
            pool_id: self.config.pool_id.clone(),
        };
        if trust.target != target {
            return Err(ExecutionError::Ownership);
        }
        let bytes: [u8; 32] = hex_key(&trust.public_key)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| invalid())?;
        signed
            .verify(&key, &target, Utc::now())
            .map_err(|_| ExecutionError::Denied)?;
        let request = &signed.reservation;
        let scoped_guard = StorageGuard::new(
            guard.deadline.min(request.expires_at),
            guard.cancelled.clone(),
        );
        let guard = &scoped_guard;
        guard.check()?;
        let mut ledger = self.build_ledger()?;
        ledger.apply_verified(request)?;
        let directory = self.config.root.join("_pool");
        let mut file = io(tempfile::NamedTempFile::new_in(&directory))?;
        let bytes = serde_json::to_vec(&ledger).map_err(|_| ExecutionError::Journal)?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(ExecutionError::Journal);
        }
        io(file.write_all(&bytes))?;
        io(file.as_file().sync_all())?;
        if file.persist(directory.join("builds.json")).is_err()
            || sync_directory(&directory).is_err()
        {
            self.poisoned = true;
            return Err(ExecutionError::Journal);
        }
        let phase = ledger.phase(request)?;
        if phase == BuildStoragePhase::Retired {
            // A tombstone is not a deletion receipt. Capacity remains charged in
            // the common pool ledger until exact consumer absence is implemented.
            return Ok(BuildStorageResult {
                phase,
                filesystem: None,
            });
        }
        // Reuse the fixed filesystem allocator; its historical app_id wire slot is
        // only a volume key here. The separate build journal owns the actual scope.
        let packet = StoragePacket::new(
            &request.volume_id,
            &request.project_id,
            &request.target.cluster_id,
            1,
            request.volume_bytes,
            request
                .expires_at
                .timestamp()
                .try_into()
                .map_err(|_| invalid())?,
        )?;
        let data_path = match request.action {
            BuildStorageAction::Reserve => self.ensure_reserved(&packet, guard, 0)?.1,
            BuildStorageAction::Inspect => {
                self.volume_state_inner(&packet, guard)?;
                self.config.root.join(&request.volume_id).join("mount/data")
            }
            BuildStorageAction::Retire => return Err(ExecutionError::Journal),
        };
        let record = self
            .record(&packet.app_id)?
            .ok_or(ExecutionError::Journal)?;
        guard.check()?;
        Ok(BuildStorageResult {
            phase,
            filesystem: Some(BuildFilesystem {
                allocation_fingerprint: request.allocation_fingerprint().map_err(|_| invalid())?,
                volume_bytes: record.bytes,
                filesystem_uuid: record.uuid,
                data_path,
            }),
        })
    }

    /// Files are root-owned/private, bounded and contain no signer private key.
    pub fn apply_build_files(
        &mut self,
        request: &Path,
        trust: &Path,
        guard: &StorageGuard,
    ) -> Result<BuildStorageResult> {
        let request = read_json::<SignedBuildStorageReservation>(request, 8192)?;
        let trust = read_json::<BuildStorageTrust>(trust, 8192)?;
        self.apply_build_storage(&request, &trust, guard)
    }
}

fn hex_key(value: &str) -> Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    for (index, chunk) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        bytes[index] = u8::from_str_radix(std::str::from_utf8(chunk).map_err(|_| invalid())?, 16)
            .map_err(|_| invalid())?;
    }
    Ok(bytes)
}

/// A path/UUID alone cannot prove which inode a live loop mount still references.
pub(super) fn validate_mount(
    mount: &serde_json::Value,
    directory: &Path,
    guard: &StorageGuard,
) -> Result<()> {
    let options = mount["options"]
        .as_str()
        .ok_or(ExecutionError::Ownership)?
        .split(',')
        .collect::<std::collections::BTreeSet<_>>();
    if !options.contains("rw")
        || !options.contains("nodev")
        || !options.contains("nosuid")
        || options.contains("noexec")
        || mount["target"] != path_str(&directory.join("mount"))?
    {
        return Err(ExecutionError::Ownership);
    }
    let image = directory.join("volume.ext4");
    private(&image, false)?;
    let meta = io(std::fs::metadata(&image))?;
    let source = mount["source"].as_str().ok_or(ExecutionError::Ownership)?;
    if !source
        .strip_prefix("/dev/loop")
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ExecutionError::Ownership);
    }
    let loops = checked(
        "/usr/sbin/losetup",
        &[
            "--json",
            "--list",
            "--output",
            "NAME,BACK-INO,BACK-MAJ:MIN",
            source,
        ],
        guard,
        10,
    )?;
    let loops: serde_json::Value =
        serde_json::from_str(&loops).map_err(|_| ExecutionError::Runtime)?;
    let loops = loops["loopdevices"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    let device = format!(
        "{}:{}",
        rustix::fs::major(meta.dev()),
        rustix::fs::minor(meta.dev())
    );
    if loops.len() != 1
        || loops[0]["back-ino"].as_u64() != Some(meta.ino())
        || loops[0]["back-maj:min"] != device
    {
        return Err(ExecutionError::Ownership);
    }
    let uuid = checked(
        "/usr/sbin/blkid",
        &["-p", "-s", "UUID", "-o", "value", path_str(&image)?],
        guard,
        10,
    )?;
    if mount["uuid"] != uuid {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
