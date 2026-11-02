//! Prepared native ntfy recovery contract; intentionally not wired during Core freeze.
//! The operation executor must validate durable policy/lease, stop all consumers and
//! retain the source before calling these checks. No authority is inferred from a receipt.
use super::{StorageGuard, StoragePacket, VolumeRecord, VolumeState};
use std::path::{Component, Path};
use webdock_supervisor::{ExecutionError, Result};

pub const NTFY_PROFILE: &str = "ntfy-sqlite-offline-v1";
pub const MAX_RECOVERY_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// Data-layer preflight only. `StoragePool::volume_state` must independently verify
/// real mount, ext4 UUID, recorded owner, capacity and data UID/GID before writes.
pub fn validate_fresh_target(
    source: &VolumeRecord,
    source_packet: &StoragePacket,
    target: &VolumeRecord,
    target_packet: &StoragePacket,
    reserved_peak_bytes: u64,
    guard: &StorageGuard,
) -> Result<()> {
    guard.check()?;
    source_packet.validate()?;
    target_packet.validate()?;
    source.authorize(source_packet)?;
    target.authorize(target_packet)?;
    if source.state != VolumeState::Ready
        || target.state != VolumeState::Ready
        || source.app_id == target.app_id
        || source.project_id != target.project_id
        || source.cluster_id != target.cluster_id
        || source.uuid == target.uuid
        || source.bytes > target.bytes
        || source.bytes > MAX_RECOVERY_BYTES
        || target.bytes > MAX_RECOVERY_BYTES
        || reserved_peak_bytes < source.bytes.saturating_add(target.bytes.saturating_mul(2))
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}

/// ntfy data only: never the source pool state, old volume image, owner record,
/// environment, TLS key, credentials or a client-provided arbitrary relative path.
pub fn ntfy_data_path(path: &Path) -> Result<()> {
    let parts = path.components().collect::<Vec<_>>();
    let valid = match parts.as_slice() {
        [Component::Normal(name)] => *name == "cache.db" || *name == "auth.db",
        [Component::Normal(directory), Component::Normal(name)] if *directory == "attachments" => {
            name.to_str().is_some_and(|name| {
                !name.is_empty()
                    && name.len() <= 128
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            })
        }
        _ => false,
    };
    if !valid {
        return Err(ExecutionError::Invalid("ntfy recovery data path"));
    }
    Ok(())
}

/// Check bounded private staging receipts before target allocation is consumed.
/// S3 multipart ETags are deliberately absent; native transport verifies full SHA-256.
pub fn verify_data_receipt(
    path: &Path,
    expected_bytes: u64,
    expected_sha256: &str,
    guard: &StorageGuard,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::{
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    guard.check()?;
    if expected_bytes > MAX_RECOVERY_BYTES
        || expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(ExecutionError::Invalid("recovery receipt"));
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|_| ExecutionError::Runtime)?;
    let metadata = file.metadata().map_err(|_| ExecutionError::Runtime)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() != expected_bytes
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(ExecutionError::Ownership);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        guard.check()?;
        let n = file
            .read(&mut buffer)
            .map_err(|_| ExecutionError::Runtime)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(ExecutionError::Runtime)?;
        if total > expected_bytes {
            return Err(ExecutionError::Ownership);
        }
        hash.update(&buffer[..n]);
    }
    if total != expected_bytes || format!("{:x}", hash.finalize()) != expected_sha256 {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
