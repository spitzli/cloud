//! Opt-in retained files from a disposable guest's final image filesystem.
use crate::{Result, artifact::safe_path, error};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    process::Command,
};

pub const RECIPE: &str = "dockerfile-frontend";
pub const CONFIG_PATH: &str = ".webdock/frontend.json";
pub const MAX_CONFIG: usize = 4096;
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontendConfig {
    pub version: u8,
    pub directory: String,
}
impl FrontendConfig {
    pub fn from_source<'a>(
        source: &'a crate::artifact::Archive,
        root: &str,
    ) -> Result<(Self, &'a [u8])> {
        let path = if root == "." {
            CONFIG_PATH.into()
        } else {
            safe_path(root)?;
            format!("{root}/{CONFIG_PATH}")
        };
        let bytes = source
            .files()
            .get(&path)
            .ok_or_else(|| error("frontend_config_missing"))?;
        Ok((Self::parse(bytes)?, bytes))
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_CONFIG {
            return Err(error("frontend_config_limit"));
        }
        let config: Self =
            serde_json::from_slice(bytes).map_err(|_| error("invalid_frontend_config"))?;
        config.validate()?;
        Ok(config)
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1 || safe_path(&self.directory).is_err() {
            return Err(error("invalid_frontend_config"));
        }
        Ok(())
    }
    /// Guest only: rootfs is a private exporter destination, never a host/customer-selected mount.
    pub fn pack(&self, rootfs: &Path, budget: u64) -> Result<Vec<u8>> {
        self.validate()?;
        let mut directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(rootfs)
            .map_err(|_| error("invalid_frontend_output"))?;
        for component in self.directory.split('/') {
            directory = child(&directory, component)?;
            if !directory.metadata().is_ok_and(|m| m.is_dir()) {
                return Err(error("invalid_frontend_output"));
            }
        }
        let mut archive = tar::Builder::new(BoundedBytes {
            bytes: Vec::new(),
            limit: budget.min(crate::artifact::MAX_OUTPUT),
        });
        let mut count = 0;
        let mut files = 0;
        pack_directory(&directory, "", &mut archive, &mut count, &mut files)?;
        if files == 0 {
            return Err(error("frontend_output_empty"));
        }
        Ok(archive
            .into_inner()
            .map_err(|_| error("frontend_output_limit"))?
            .bytes)
    }
}

/// Treat guest files as untrusted, including tar entry types and encoded overhead.
pub fn validate_output(bytes: &[u8]) -> Result<crate::artifact::Archive> {
    if bytes.len() as u64 > crate::artifact::MAX_OUTPUT {
        return Err(error("frontend_output_limit"));
    }
    let archive = crate::artifact::Archive::read(bytes, crate::artifact::MAX_OUTPUT)?;
    if archive.files().is_empty() {
        return Err(error("frontend_output_empty"));
    }
    let mut records = 0;
    for entry in tar::Archive::new(bytes)
        .entries()
        .map_err(|_| error("invalid_frontend_output"))?
    {
        let entry = entry.map_err(|_| error("invalid_frontend_output"))?;
        if !entry.header().entry_type().is_file() {
            return Err(error("invalid_frontend_output"));
        }
        records += 1;
        if records > MAX_ENTRIES {
            return Err(error("frontend_entry_limit"));
        }
    }
    Ok(archive)
}

/// Private library consumer. The caller obtains this exact receipt and build scope under Core authority.
/// This exposes no file/URL selector and grants no customer-facing download authority.
pub fn retained(
    store: &crate::artifact::ArtifactStore,
    artifact: &crate::artifact::StoredArtifact,
    expected: &crate::evidence::BuildEvidence,
    signer: Option<&ed25519_dalek::VerifyingKey>,
) -> Result<crate::artifact::Archive> {
    let bytes = store.read(artifact)?;
    let archive = crate::artifact::Archive::read(&bytes, crate::artifact::MAX_OUTPUT)?;
    let provenance = crate::evidence::validate(
        &archive,
        &expected.customer_id,
        &expected.build_id,
        expected.generation,
        signer,
    )?
    .ok_or_else(|| error("frontend_output_missing"))?;
    if provenance.build != *expected || provenance.version != 2 {
        return Err(error("artifact_evidence_scope_mismatch"));
    }
    validate_output(
        archive
            .files()
            .get("frontend.tar")
            .ok_or_else(|| error("frontend_output_missing"))?,
    )
}

// O_PATH pins the inode without opening a device/FIFO or following a swapped symlink.
fn child(directory: &File, name: &str) -> Result<File> {
    let name = std::ffi::CString::new(name).map_err(|_| error("invalid_frontend_output"))?;
    // SAFETY: the parent descriptor and NUL-terminated single component remain valid for openat.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(error("invalid_frontend_output"));
    }
    // SAFETY: openat returned a new owned descriptor, transferred exactly once to File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: u64,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.limit.saturating_sub(self.bytes.len() as u64) {
            return Err(std::io::Error::other("frontend_output_limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn pack_directory(
    directory: &File,
    prefix: &str,
    archive: &mut tar::Builder<BoundedBytes>,
    count: &mut usize,
    files: &mut usize,
) -> Result<()> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))
        .map_err(|_| error("invalid_frontend_output"))?
    {
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err(error("frontend_entry_limit"));
        }
        names.push(
            entry
                .map_err(|_| error("invalid_frontend_output"))?
                .file_name()
                .into_string()
                .map_err(|_| error("invalid_frontend_output"))?,
        );
    }
    names.sort();
    for name in names {
        let path = format!("{prefix}{name}");
        safe_path(&path).map_err(|_| error("invalid_frontend_output"))?;
        let inode = child(directory, &name)?;
        let metadata = inode
            .metadata()
            .map_err(|_| error("invalid_frontend_output"))?;
        if metadata.is_dir() {
            pack_directory(&inode, &format!("{path}/"), archive, count, files)?;
        } else if metadata.is_file() && metadata.nlink() == 1 {
            if metadata.len()
                > archive
                    .get_ref()
                    .limit
                    .saturating_sub(archive.get_ref().bytes.len() as u64)
            {
                return Err(error("frontend_output_limit"));
            }
            let file = File::open(format!("/proc/self/fd/{}", inode.as_raw_fd()))
                .map_err(|_| error("invalid_frontend_output"))?;
            let mut header = tar::Header::new_ustar();
            header.set_size(metadata.len());
            header.set_mode(0o644);
            if header.set_path(&path).is_err() {
                archive
                    .append_pax_extensions([("path", path.as_bytes())])
                    .map_err(|_| error("frontend_output_limit"))?;
                header
                    .set_path("file")
                    .map_err(|_| error("invalid_frontend_output"))?;
            }
            header.set_cksum();
            archive
                .append(&header, file.take(metadata.len()))
                .map_err(|_| error("frontend_output_limit"))?;
            *files += 1;
        } else {
            return Err(error("invalid_frontend_output"));
        }
    }
    Ok(())
}

pub fn configure_exporters(
    command: &mut Command,
    recipe: &str,
    image: &Path,
    rootfs: &Path,
) -> Result<()> {
    if !["dockerfile", RECIPE].contains(&recipe) {
        return Err(error("recipe_unavailable"));
    }
    fn valid(p: &Path) -> Option<&str> {
        p.to_str()
            .filter(|v| !v.is_empty() && !v.contains([',', '\n', '\r', '\0']))
    }
    let image = valid(image).ok_or_else(|| error("invalid_export_path"))?;
    let rootfs = valid(rootfs).ok_or_else(|| error("invalid_export_path"))?;
    command
        .arg("--output")
        .arg(format!("type=oci,dest={image}"));
    if recipe == RECIPE {
        command
            .arg("--output")
            .arg(format!("type=local,dest={rootfs}"));
    }
    Ok(())
}
