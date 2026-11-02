use crate::{Command, ExecutionError, ObservedState, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
const MAX_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    commands: BTreeMap<String, Receipt>,
    resources: BTreeMap<String, Fence>,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Fence {
    #[serde(default)]
    revision: Option<u64>,
    generation: u64,
    operation_id: String,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Receipt {
    fingerprint: String,
    #[serde(default)]
    payload_fingerprint: Option<String>,
    #[serde(default)]
    generation: Option<u64>,
    status: Status,
    result: Option<ObservedState>,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
enum Status {
    Running,
    NeedsReconciliation,
    NeedsVerification,
    Succeeded,
}
pub enum Claim {
    Execute { recovery: bool },
    Replay(ObservedState),
    Verify(ObservedState),
}
pub struct Journal {
    path: PathBuf,
    state: State,
    _lock: File,
    poisoned: bool,
}
impl Journal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_io(path.as_ref()).map_err(|_| ExecutionError::Journal)
    }
    fn open_io(path: &Path) -> std::io::Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("directory required"))?;
        let metadata = std::fs::symlink_metadata(parent)?;
        if !path.is_absolute()
            || !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
            || std::fs::canonicalize(parent)? != parent
        {
            return Err(std::io::Error::other("private directory required"));
        }
        let lock_path = path.with_extension("lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(lock_path)?;
        let metadata = lock.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(std::io::Error::other("unsafe journal lock"));
        }
        lock.try_lock_exclusive()?;
        let state = match OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path)
        {
            Ok(file) => {
                let meta = file.metadata()?;
                if !meta.is_file()
                    || meta.len() > MAX_BYTES
                    || meta.uid() != rustix::process::geteuid().as_raw()
                    || meta.mode() & 0o077 != 0
                {
                    return Err(std::io::Error::other("unsafe journal"));
                }
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                let state: State = serde_json::from_slice(&bytes)?;
                if state.version != 1 || state.commands.len() > 10000 {
                    return Err(std::io::Error::other("unsupported journal"));
                }
                state
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State {
                version: 1,
                ..State::default()
            },
            Err(e) => return Err(e),
        };
        Ok(Self {
            path: path.into(),
            state,
            _lock: lock,
            poisoned: false,
        })
    }
    fn persist(&mut self, next: State) -> Result<()> {
        let bytes = serde_json::to_vec(&next).map_err(|_| ExecutionError::Journal)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(ExecutionError::Journal);
        }
        let parent = self.path.parent().ok_or(ExecutionError::Journal)?;
        let result = (|| -> std::io::Result<()> {
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&bytes)?;
            file.as_file().sync_all()?;
            file.persist(&self.path).map_err(|e| e.error)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
            return Err(ExecutionError::Journal);
        }
        self.state = next;
        Ok(())
    }
    pub fn claim(&mut self, command: &Command) -> Result<Claim> {
        if self.poisoned {
            return Err(ExecutionError::Journal);
        }
        command.validate()?;
        let key = command.spec.key();
        if let Some(fence) = self.state.resources.get(&key) {
            let older = if let Some(revision) = fence.revision {
                (command.spec.revision, command.generation) < (revision, fence.generation)
                    || ((command.spec.revision, command.generation) == (revision, fence.generation)
                        && command.operation_id != fence.operation_id)
            } else {
                command.generation < fence.generation
                    || (command.generation == fence.generation
                        && command.operation_id != fence.operation_id)
            };
            if older {
                return Err(ExecutionError::Fenced);
            }
        }
        let fingerprint = command.fingerprint()?;
        let payload_fingerprint = command.payload_fingerprint()?;
        if let Some(receipt) = self.state.commands.get(&command.operation_id) {
            let mut receipt = receipt.clone();
            if receipt.fingerprint != fingerprint {
                if receipt.payload_fingerprint.as_ref() != Some(&payload_fingerprint)
                    || receipt
                        .generation
                        .is_none_or(|old| command.generation <= old)
                {
                    return Err(ExecutionError::Conflict);
                }
                let mut next = self.state.clone();
                let updated = next
                    .commands
                    .get_mut(&command.operation_id)
                    .ok_or(ExecutionError::Journal)?;
                updated.fingerprint = fingerprint;
                updated.generation = Some(command.generation);
                if matches!(
                    updated.status,
                    Status::Succeeded | Status::NeedsVerification
                ) {
                    updated.status = Status::NeedsVerification;
                }
                receipt = updated.clone();
                next.resources.insert(
                    key,
                    Fence {
                        revision: Some(command.spec.revision),
                        generation: command.generation,
                        operation_id: command.operation_id.clone(),
                    },
                );
                self.persist(next)?;
            }
            if matches!(receipt.status, Status::Succeeded) {
                return Ok(Claim::Replay(
                    receipt.result.ok_or(ExecutionError::Journal)?,
                ));
            }
            if matches!(receipt.status, Status::NeedsVerification) {
                return Ok(Claim::Verify(
                    receipt.result.ok_or(ExecutionError::Journal)?,
                ));
            }
            return Ok(Claim::Execute { recovery: true });
        }
        if self.state.commands.len() >= 10000 {
            return Err(ExecutionError::Journal);
        }
        let mut next = self.state.clone();
        next.resources.insert(
            key,
            Fence {
                revision: Some(command.spec.revision),
                generation: command.generation,
                operation_id: command.operation_id.clone(),
            },
        );
        next.commands.insert(
            command.operation_id.clone(),
            Receipt {
                fingerprint,
                payload_fingerprint: Some(payload_fingerprint),
                generation: Some(command.generation),
                status: Status::Running,
                result: None,
            },
        );
        self.persist(next)?;
        Ok(Claim::Execute { recovery: false })
    }
    pub fn complete(&mut self, command: &Command, result: &ObservedState) -> Result<()> {
        self.finish(command, Status::Succeeded, Some(result.clone()))
    }
    pub fn uncertain(&mut self, command: &Command) -> Result<()> {
        self.finish(command, Status::NeedsReconciliation, None)
    }
    fn finish(
        &mut self,
        command: &Command,
        status: Status,
        result: Option<ObservedState>,
    ) -> Result<()> {
        if self.poisoned {
            return Err(ExecutionError::Journal);
        }
        let fence = self
            .state
            .resources
            .get(&command.spec.key())
            .ok_or(ExecutionError::Fenced)?;
        if fence.generation != command.generation
            || fence.operation_id != command.operation_id
            || fence
                .revision
                .is_some_and(|revision| revision != command.spec.revision)
        {
            return Err(ExecutionError::Fenced);
        }
        let mut next = self.state.clone();
        let receipt = next
            .commands
            .get_mut(&command.operation_id)
            .ok_or(ExecutionError::Conflict)?;
        if receipt.fingerprint != command.fingerprint()? {
            return Err(ExecutionError::Conflict);
        }
        receipt.status = status;
        receipt.result = result;
        self.persist(next)
    }
}
