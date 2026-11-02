//! Concrete private staging for the two supported recovery recipes.
use super::config::{RecoveryProfileConfig, private};
use crate::storage::StorageGuard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use webdock_supervisor::{ExecutionError, Result};
struct GuardedRead<'a, R> {
    inner: R,
    guard: &'a StorageGuard,
}
impl<R: Read> Read for GuardedRead<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.guard.check().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "recovery guard expired",
            )
        })?;
        self.inner.read(buffer)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustodyReceipt {
    pub id: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryMaterial {
    pub version: u8,
    pub files: BTreeMap<String, String>,
    pub environment: BTreeMap<String, String>,
}
pub fn receipt(path: &Path, id: &str) -> Result<CustodyReceipt> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ExecutionError::Runtime)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o077 != 0 {
        return Err(ExecutionError::Ownership);
    }
    let mut hash = Sha256::new();
    let mut stream = std::fs::File::open(path).map_err(|_| ExecutionError::Runtime)?;
    let mut buffer = [0; 65536];
    loop {
        let n = stream
            .read(&mut buffer)
            .map_err(|_| ExecutionError::Runtime)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(CustodyReceipt {
        id: id.into(),
        name: path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(ExecutionError::Ownership)?
            .into(),
        bytes: metadata.len(),
        sha256: format!("{:x}", hash.finalize()),
    })
}
pub async fn age(
    input: &Path,
    output: &Path,
    profile: &RecoveryProfileConfig,
    decrypt: bool,
) -> Result<()> {
    if output.exists() {
        return Err(ExecutionError::Conflict);
    }
    let mut command = tokio::process::Command::new("/usr/bin/age");
    command.arg(if decrypt { "--decrypt" } else { "--encrypt" });
    if decrypt {
        private(&profile.age_identity, 16384)?;
        command.arg("--identity").arg(&profile.age_identity);
    } else {
        command.arg("--recipient").arg(&profile.age_recipient);
    }
    let child = command
        .arg("--output")
        .arg(output)
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| ExecutionError::Runtime)?;
    let status = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(|_| ExecutionError::Runtime)?;
    if !status.status.success() {
        let _ = std::fs::remove_file(output);
        return Err(ExecutionError::Denied);
    }
    std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| ExecutionError::Runtime)?;
    std::fs::File::open(output)
        .and_then(|f| f.sync_all())
        .map_err(|_| ExecutionError::Runtime)
}
pub fn pack(directory: &Path, output: &Path, guard: &StorageGuard, capacity: u64) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(output)
        .map_err(|_| ExecutionError::Conflict)?;
    let mut archive = tar::Builder::new(file);
    let mut total = 0;
    let mut pending = vec![directory.to_path_buf()];
    let mut count = 0;
    while let Some(root) = pending.pop() {
        for entry in std::fs::read_dir(&root).map_err(|_| ExecutionError::Runtime)? {
            guard.check()?;
            let entry = entry.map_err(|_| ExecutionError::Runtime)?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|_| ExecutionError::Runtime)?;
            if metadata.file_type().is_symlink() {
                return Err(ExecutionError::Ownership);
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(ExecutionError::Ownership);
            }
            total += metadata.len();
            count += 1;
            if total > capacity || count > 20000 {
                return Err(ExecutionError::Invalid("recovery archive capacity"));
            }
            let input = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(&path)
                .map_err(|_| ExecutionError::Runtime)?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata(&metadata);
            header.set_uid(0);
            header.set_gid(0);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            archive
                .append_data(
                    &mut header,
                    path.strip_prefix(directory)
                        .map_err(|_| ExecutionError::Ownership)?,
                    GuardedRead {
                        inner: input,
                        guard,
                    },
                )
                .map_err(|_| ExecutionError::Runtime)?;
        }
    }
    archive.finish().map_err(|_| ExecutionError::Runtime)?;
    archive
        .into_inner()
        .map_err(|_| ExecutionError::Runtime)?
        .sync_all()
        .map_err(|_| ExecutionError::Runtime)
}
pub fn unpack(input: &Path, directory: &Path, guard: &StorageGuard, capacity: u64) -> Result<()> {
    if std::fs::read_dir(directory)
        .map_err(|_| ExecutionError::Runtime)?
        .next()
        .is_some()
    {
        return Err(ExecutionError::Ownership);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(input)
        .map_err(|_| ExecutionError::Runtime)?;
    let mut archive = tar::Archive::new(file);
    let mut total = 0u64;
    let mut count = 0usize;
    let mut names = std::collections::BTreeSet::new();
    for entry in archive.entries().map_err(|_| ExecutionError::Runtime)? {
        guard.check()?;
        let mut entry = entry.map_err(|_| ExecutionError::Runtime)?;
        let raw_path = entry
            .path()
            .map_err(|_| ExecutionError::Ownership)?
            .into_owned();
        if raw_path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(ExecutionError::Ownership);
        }
        let path = raw_path
            .components()
            .filter(|part| !matches!(part, Component::CurDir))
            .collect::<PathBuf>();
        count += 1;
        if count > 20000 || path.as_os_str().len() > 1024 {
            return Err(ExecutionError::Invalid("recovery archive entry bound"));
        }
        if entry.header().entry_type().is_dir() {
            if path.as_os_str().is_empty() {
                continue;
            }
            let target = directory.join(&path);
            std::fs::create_dir_all(&target).map_err(|_| ExecutionError::Runtime)?;
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| ExecutionError::Runtime)?;
            continue;
        }
        if !entry.header().entry_type().is_file()
            || !names.insert(path.clone())
            || path.as_os_str().len() > 1024
        {
            return Err(ExecutionError::Ownership);
        }
        total = total
            .checked_add(entry.size())
            .ok_or(ExecutionError::Runtime)?;
        if total > capacity {
            return Err(ExecutionError::Invalid("recovery archive capacity"));
        }
        let output = directory.join(&path);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|_| ExecutionError::Runtime)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(output)
            .map_err(|_| ExecutionError::Ownership)?;
        let written = std::io::copy(
            &mut GuardedRead {
                inner: &mut entry,
                guard,
            },
            &mut file,
        )
        .map_err(|_| ExecutionError::Runtime)?;
        if written != entry.size() {
            return Err(ExecutionError::Runtime);
        }
        file.sync_all().map_err(|_| ExecutionError::Runtime)?;
    }
    std::fs::File::open(directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| ExecutionError::Runtime)
}
pub fn capture_material(
    profile: &RecoveryProfileConfig,
    environment: BTreeMap<String, String>,
    path: &Path,
) -> Result<()> {
    use base64::Engine;
    let mut files = BTreeMap::new();
    for (name, file) in &profile.independent_files {
        files.insert(
            name.clone(),
            base64::engine::general_purpose::STANDARD.encode(private(file, 262144)?),
        );
    }
    let material = RecoveryMaterial {
        version: 1,
        files,
        environment,
    };
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| ExecutionError::Conflict)?;
    file.write_all(&serde_json::to_vec(&material).map_err(|_| ExecutionError::Runtime)?)
        .and_then(|()| file.sync_all())
        .map_err(|_| ExecutionError::Runtime)
}
pub fn material_path(directory: &Path, receipt: &CustodyReceipt) -> Result<PathBuf> {
    if receipt.name.len() > 128
        || receipt.name.is_empty()
        || receipt
            .name
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && !b"_.-".contains(&b))
        || receipt.bytes > 4 * 1024 * 1024
    {
        return Err(ExecutionError::Ownership);
    }
    let path = directory.join(&receipt.name);
    let actual = self::receipt(&path, &receipt.id)?;
    if actual.bytes != receipt.bytes || actual.sha256 != receipt.sha256 {
        return Err(ExecutionError::Ownership);
    }
    Ok(path)
}
