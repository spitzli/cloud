use crate::{Result, error, io};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read, Write},
    path::Path,
};
pub const MAX_SOURCE: u64 = 128 * 1024 * 1024;
pub const MAX_OUTPUT: u64 = 512 * 1024 * 1024;
/// One USTAR image.tar header and two end blocks must fit the retained envelope.
/// The maximum is 512-byte aligned, so rounding any smaller payload still fits.
pub const MAX_RETAINED_IMAGE: u64 = MAX_OUTPUT - 1536;
pub fn validate_retained_image_size(bytes: u64) -> Result<()> {
    if bytes > MAX_RETAINED_IMAGE {
        return Err(error("artifact_limit"));
    }
    Ok(())
}
pub const MAX_ENTRIES: usize = 50_000;
pub const SUPPORTED_FORMATS: &[u8] = &[0, 1, 2];
pub(crate) fn storage_format(key: &str, digest: &str) -> Result<u8> {
    let hash = digest
        .strip_prefix("sha256:")
        .filter(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or_else(|| error("invalid_artifact"))?;
    let name = key
        .rsplit('/')
        .next()
        .ok_or_else(|| error("invalid_artifact"))?;
    if name == format!("{hash}.tar") {
        Ok(0)
    } else if name == format!("{hash}.e1.tar") {
        Ok(1)
    } else if name == format!("{hash}.e2.tar") {
        Ok(2)
    } else {
        Err(error("artifact_digest_mismatch"))
    }
}
pub struct Archive {
    files: BTreeMap<String, Vec<u8>>,
    executable: BTreeSet<String>,
}
pub fn safe_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path
            .split('/')
            .any(|x| x.is_empty() || x == "." || x == "..")
    {
        return Err(error("invalid_archive_path"));
    }
    Ok(())
}
impl Archive {
    pub fn from_files(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        for path in files.keys() {
            safe_path(path)?;
        }
        if files.len() > MAX_ENTRIES {
            return Err(error("archive_entry_limit"));
        }
        Ok(Self {
            files,
            executable: BTreeSet::new(),
        })
    }
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
    pub fn into_files(self) -> BTreeMap<String, Vec<u8>> {
        self.files
    }
    pub fn read(bytes: &[u8], limit: u64) -> Result<Self> {
        if bytes.len() as u64 > limit + MAX_ENTRIES as u64 * 1024 + 10240 {
            return Err(error("archive_limit"));
        }
        // Inspect raw records too: tar's logical iterator deliberately ignores trailing records.
        let mut offset = 0usize;
        let mut count = 0usize;
        while offset + 512 <= bytes.len() {
            let block = &bytes[offset..offset + 512];
            if block.iter().all(|b| *b == 0) {
                if bytes[offset..].iter().any(|b| *b != 0) {
                    return Err(error("archive_trailing_data"));
                }
                break;
            }
            let header = tar::Header::from_byte_slice(block);
            let size = header.size().map_err(io)?;
            let kind = header.entry_type();
            if !(kind.is_file()
                || kind.is_dir()
                || kind.is_pax_global_extensions()
                || kind.is_pax_local_extensions())
            {
                return Err(error("archive_entry_type"));
            }
            if (kind.is_pax_global_extensions() || kind.is_pax_local_extensions()) && size > 65536 {
                return Err(error("archive_metadata_limit"));
            }
            if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
                let end = offset
                    .checked_add(512)
                    .and_then(|n| n.checked_add(size as usize))
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| error("archive_truncated"))?;
                let metadata = &bytes[offset + 512..end];
                if metadata.windows(10).any(|v| v == b"GNU.sparse")
                    || metadata.windows(15).any(|v| v == b"SCHILY.filetype")
                {
                    return Err(error("archive_sparse"));
                }
            }
            count += 1;
            if count > MAX_ENTRIES {
                return Err(error("archive_entry_limit"));
            }
            offset = offset
                .checked_add(512)
                .and_then(|n| n.checked_add(size.div_ceil(512).checked_mul(512)? as usize))
                .ok_or_else(|| error("archive_limit"))?;
            if offset > bytes.len() {
                return Err(error("archive_truncated"));
            }
        }
        if offset < bytes.len() && bytes.len() - offset < 512 {
            return Err(error("archive_truncated"));
        }
        let mut archive = tar::Archive::new(Cursor::new(bytes));
        let mut files = BTreeMap::new();
        let mut executable = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for entry in archive.entries().map_err(io)? {
            let mut entry = entry.map_err(io)?;
            if entry.header().entry_type().is_pax_global_extensions()
                || entry.header().entry_type().is_pax_local_extensions()
            {
                continue;
            }
            let path = entry
                .path()
                .map_err(io)?
                .to_str()
                .ok_or_else(|| error("invalid_archive_path"))?
                .trim_end_matches('/')
                .to_owned();
            safe_path(&path)?;
            if !names.insert(path.clone()) {
                return Err(error("archive_duplicate"));
            }
            if entry.header().entry_type().is_dir() {
                continue;
            }
            if !entry.header().entry_type().is_file() {
                return Err(error("archive_entry_type"));
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| error("archive_limit"))?;
            if total > limit {
                return Err(error("archive_limit"));
            }
            let mut content = Vec::new();
            entry.read_to_end(&mut content).map_err(io)?;
            if entry.header().mode().map_err(io)? & 0o111 != 0 {
                executable.insert(path.clone());
            }
            files.insert(path, content);
        }
        // A file must never also be an ancestor directory, regardless of archive ordering.
        for name in files.keys() {
            let mut p = Path::new(name).parent();
            while let Some(parent) = p {
                if files.contains_key(parent.to_str().unwrap_or("")) {
                    return Err(error("archive_path_collision"));
                }
                p = parent.parent();
            }
        }
        Ok(Self { files, executable })
    }
    pub fn guest_request(mut self, job: Vec<u8>) -> Result<Self> {
        self.files = self
            .files
            .into_iter()
            .map(|(k, v)| (format!("source/{k}"), v))
            .collect();
        self.executable = self
            .executable
            .into_iter()
            .map(|k| format!("source/{k}"))
            .collect();
        self.files.insert("job.json".into(), job);
        Ok(self)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = tar::Builder::new(Vec::new());
        for (path, data) in &self.files {
            let mut h = tar::Header::new_ustar();
            h.set_size(data.len() as u64);
            h.set_mode(if self.executable.contains(path) {
                0o755
            } else {
                0o644
            });
            if h.set_path(path).is_err() {
                out.append_pax_extensions([("path", path.as_bytes())])
                    .map_err(io)?;
                h.set_path("file").map_err(io)?;
            }
            h.set_cksum();
            out.append(&h, &data[..]).map_err(io)?;
        }
        out.into_inner().map_err(io)
    }
    /// Destination must be a newly created private directory owned by this process.
    pub fn extract_private(&self, destination: &Path) -> Result<()> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let meta = destination.symlink_metadata().map_err(io)?;
        if !meta.is_dir()
            || meta.permissions().mode() & 0o077 != 0
            || destination.read_dir().map_err(io)?.next().is_some()
        {
            return Err(error("private_staging_required"));
        }
        for (path, data) in &self.files {
            let mode = if self.executable.contains(path) {
                0o700
            } else {
                0o600
            };
            let path = destination.join(path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
                .map_err(io)?;
            file.write_all(data).map_err(io)?;
        }
        Ok(())
    }

    /// Git modes for Docker COPY, behind the attempt's still-private ancestor.
    /// VM inputs and secret staging continue using extract_private unchanged.
    #[cfg(feature = "kubernetes-job")]
    pub(crate) fn extract_build_context(&self, destination: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let parent = destination
            .parent()
            .ok_or_else(|| error("private_staging_required"))?;
        let metadata = parent.symlink_metadata().map_err(io)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(error("private_staging_required"));
        }
        self.extract_private(destination)?;
        for name in self.files.keys() {
            let path = destination.join(name);
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if self.executable.contains(name) {
                    0o755
                } else {
                    0o644
                }),
            )
            .map_err(io)?;
            let mut directory = path.parent();
            while let Some(path) = directory {
                if path == destination {
                    break;
                }
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                    .map_err(io)?;
                directory = path.parent();
            }
        }
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755))
            .map_err(io)?;
        Ok(())
    }
}
fn json(data: &[u8]) -> Result<Value> {
    serde_json::from_slice(data).map_err(|_| error("invalid_oci"))
}
pub fn validate_oci(bytes: &[u8]) -> Result<String> {
    let archive = Archive::read(bytes, MAX_OUTPUT)?;
    let files = archive.files();
    if json(
        files
            .get("oci-layout")
            .ok_or_else(|| error("invalid_oci"))?,
    )?["imageLayoutVersion"]
        != "1.0.0"
    {
        return Err(error("invalid_oci"));
    }
    let index = json(
        files
            .get("index.json")
            .ok_or_else(|| error("invalid_oci"))?,
    )?;
    if index["schemaVersion"] != 2 || index["manifests"].as_array().is_none_or(|a| a.len() != 1) {
        return Err(error("invalid_oci"));
    }
    let root = &index["manifests"][0];
    let mut pending = vec![(root.clone(), 0usize)];
    let mut visited = BTreeSet::new();
    while let Some((descriptor, depth)) = pending.pop() {
        if depth > 32 {
            return Err(error("invalid_oci"));
        }
        let digest = descriptor["digest"]
            .as_str()
            .ok_or_else(|| error("invalid_oci"))?;
        let hash = digest
            .strip_prefix("sha256:")
            .filter(|h| {
                h.len() == 64
                    && h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(|| error("invalid_oci"))?;
        let data = files
            .get(&format!("blobs/sha256/{hash}"))
            .ok_or_else(|| error("oci_blob_missing"))?;
        if descriptor["size"].as_u64() != Some(data.len() as u64)
            || hex::encode(Sha256::digest(data)) != hash
        {
            return Err(error("oci_digest_mismatch"));
        }
        if !visited.insert(digest.to_owned()) {
            continue;
        }
        match descriptor["mediaType"].as_str().unwrap_or("") {
            "application/vnd.oci.image.manifest.v1+json"
            | "application/vnd.docker.distribution.manifest.v2+json" => {
                let manifest = json(data)?;
                if manifest["schemaVersion"] != 2 {
                    return Err(error("invalid_oci"));
                }
                pending.push((manifest["config"].clone(), depth + 1));
                for layer in manifest["layers"]
                    .as_array()
                    .ok_or_else(|| error("invalid_oci"))?
                {
                    pending.push((layer.clone(), depth + 1));
                }
            }
            "application/vnd.oci.image.index.v1+json"
            | "application/vnd.docker.distribution.manifest.list.v2+json" => {
                let index = json(data)?;
                for manifest in index["manifests"]
                    .as_array()
                    .ok_or_else(|| error("invalid_oci"))?
                {
                    pending.push((manifest.clone(), depth + 1));
                }
            }
            "application/vnd.oci.image.config.v1+json"
            | "application/vnd.docker.container.image.v1+json"
            | "application/vnd.oci.image.layer.v1.tar"
            | "application/vnd.oci.image.layer.v1.tar+gzip"
            | "application/vnd.oci.image.layer.v1.tar+zstd"
            | "application/vnd.docker.image.rootfs.diff.tar.gzip" => {}
            _ => return Err(error("oci_media_type")),
        }
    }
    Ok(root["digest"].as_str().unwrap().into())
}
/// A private worker-local artifact store. Publication revalidates both the archive and OCI graph.
pub struct ArtifactStore {
    root: std::path::PathBuf,
    limit: u64,
}
#[derive(Clone)]
pub struct StoredArtifact {
    pub(crate) key: String,
    pub(crate) digest: String,
    pub(crate) image_digest: String,
    pub(crate) size: u64,
    pub(crate) remote: Option<webdock_object_storage::ObjectReceipt>,
}
impl StoredArtifact {
    pub fn format_version(&self) -> Result<u8> {
        storage_format(&self.key, &self.digest)
    }
    pub fn remote_storage(&self) -> Option<&webdock_object_storage::ObjectReceipt> {
        self.remote.as_ref()
    }
    pub(crate) fn validate_remote(&self) -> Result<()> {
        if let Some(remote) = &self.remote {
            let (scope, _) = self
                .key
                .rsplit_once('/')
                .ok_or_else(|| error("invalid_artifact"))?;
            remote
                .validate(scope, MAX_OUTPUT)
                .map_err(|_| error("artifact_scope_mismatch"))?;
            if self.digest != format!("sha256:{}", remote.sha256) || self.size != remote.size_bytes
            {
                return Err(error("artifact_digest_mismatch"));
            }
        }
        Ok(())
    }
    pub fn storage_key(&self) -> &str {
        &self.key
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn image_digest(&self) -> &str {
        &self.image_digest
    }
    pub fn size(&self) -> u64 {
        self.size
    }
}
impl ArtifactStore {
    pub fn open(root: std::path::PathBuf, limit: u64) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let m = root.symlink_metadata().map_err(io)?;
        if !m.is_dir()
            || m.mode() & 0o077 != 0
            // SAFETY: geteuid has no preconditions or pointer arguments.
            || m.uid() != unsafe { libc::geteuid() }
            || !(MAX_OUTPUT..=200 * 1024 * 1024 * 1024).contains(&limit)
        {
            return Err(error("private_artifact_store_required"));
        }
        Ok(Self {
            root: root.canonicalize().map_err(io)?,
            limit,
        })
    }
    pub fn put(
        &self,
        customer: &str,
        build: &str,
        generation: i32,
        bytes: &[u8],
    ) -> Result<StoredArtifact> {
        use std::os::fd::AsRawFd;
        let store_lock = std::fs::File::open(&self.root).map_err(io)?;
        // Advisory lock serializes quota accounting and durable rename across worker processes.
        // SAFETY: the live File owns this descriptor and flock does not retain it.
        if unsafe { libc::flock(store_lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        crate::contracts::id(customer)?;
        crate::contracts::id(build)?;
        if generation <= 0 || bytes.len() as u64 > MAX_OUTPUT {
            return Err(error("artifact_limit"));
        }
        let archive = Archive::read(bytes, MAX_OUTPUT)?;
        let image_digest =
            crate::evidence::validate_for_store(&archive, customer, build, generation)?;
        let hash = hex::encode(Sha256::digest(bytes));
        let suffix = if archive.files.len() == 1 {
            "tar"
        } else if archive.files.contains_key("frontend.tar") {
            "e2.tar"
        } else {
            "e1.tar"
        };
        let key = format!("{customer}/{build}/{generation}/{hash}.{suffix}");
        let mut dir = self.root.clone();
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
        for component in [customer, build, &generation.to_string()] {
            dir.push(component);
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io(e)),
            }
            let m = dir.symlink_metadata().map_err(io)?;
            // SAFETY: geteuid has no preconditions or pointer arguments.
            if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
                return Err(error("private_artifact_store_required"));
            }
        }
        fn size(path: &Path) -> Result<u64> {
            let mut total = 0u64;
            for entry in path.read_dir().map_err(io)? {
                let e = entry.map_err(io)?;
                let m = e.path().symlink_metadata().map_err(io)?;
                if m.is_dir() {
                    total = total
                        .checked_add(size(&e.path())?)
                        .ok_or_else(|| error("artifact_limit"))?;
                } else if m.is_file() {
                    total = total
                        .checked_add(m.len())
                        .ok_or_else(|| error("artifact_limit"))?;
                } else {
                    return Err(error("invalid_artifact_store"));
                }
            }
            Ok(total)
        }
        let target = self.root.join(&key);
        if target.exists() {
            let existing = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&target)
                .map_err(io)?;
            if existing.metadata().map_err(io)?.len() != bytes.len() as u64
                || hex::encode(Sha256::digest(std::fs::read(&target).map_err(io)?)) != hash
            {
                return Err(error("artifact_collision"));
            }
        } else {
            if size(&self.root)?.saturating_add(bytes.len() as u64) > self.limit {
                return Err(error("artifact_store_full"));
            }
            let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(io)?;
            temp.write_all(bytes).map_err(io)?;
            temp.as_file().sync_all().map_err(io)?;
            temp.persist_noclobber(&target)
                .map_err(|_| error("artifact_collision"))?;
            std::fs::File::open(&dir)
                .map_err(io)?
                .sync_all()
                .map_err(io)?;
        }
        Ok(StoredArtifact {
            key,
            digest: format!("sha256:{hash}"),
            image_digest,
            size: bytes.len() as u64,
            remote: None,
        })
    }
    pub fn read(&self, artifact: &StoredArtifact) -> Result<Vec<u8>> {
        use std::os::unix::fs::OpenOptionsExt;
        safe_path(&artifact.key)?;
        artifact.validate_remote()?;
        let mut cursor = self.root.clone();
        for component in Path::new(&artifact.key).components() {
            cursor.push(component);
            if cursor
                .symlink_metadata()
                .map_err(io)?
                .file_type()
                .is_symlink()
            {
                return Err(error("invalid_artifact_store"));
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(cursor)
            .map_err(io)?;
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() as u64 != artifact.size
            || format!("sha256:{}", hex::encode(Sha256::digest(&bytes))) != artifact.digest
        {
            return Err(error("artifact_digest_mismatch"));
        }
        let archive = Archive::read(&bytes, MAX_OUTPUT)?;
        let content_format = if archive.files.len() == 1 {
            0
        } else if archive.files.contains_key("frontend.tar") {
            2
        } else {
            1
        };
        if artifact.format_version()? != content_format {
            return Err(error("artifact_format_mismatch"));
        }
        let scope = artifact.key.split('/').collect::<Vec<_>>();
        if scope.len() != 4 {
            return Err(error("invalid_artifact"));
        }
        let generation = scope[2]
            .parse::<i32>()
            .map_err(|_| error("invalid_artifact"))?;
        if crate::evidence::validate_for_store(&archive, scope[0], scope[1], generation)?
            != artifact.image_digest
        {
            return Err(error("artifact_digest_mismatch"));
        }
        Ok(bytes)
    }
}
impl ArtifactStore {
    /// Recover a retained artifact after worker restart, verifying the persisted receipt against disk.
    pub fn load(
        &self,
        key: String,
        digest: String,
        size: u64,
        image_digest: String,
    ) -> Result<StoredArtifact> {
        let parts = key.split('/').collect::<Vec<_>>();
        if parts.len() != 4 || size == 0 || size > MAX_OUTPUT {
            return Err(error("invalid_artifact"));
        }
        crate::contracts::id(parts[0])?;
        crate::contracts::id(parts[1])?;
        crate::contracts::id(parts[2])?;
        let hash = parts[3]
            .strip_suffix(".e2.tar")
            .or_else(|| parts[3].strip_suffix(".e1.tar"))
            .or_else(|| parts[3].strip_suffix(".tar"))
            .filter(|h| {
                h.len() == 64
                    && h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or_else(|| error("invalid_artifact"))?;
        if digest != format!("sha256:{hash}") {
            return Err(error("artifact_digest_mismatch"));
        }
        let artifact = StoredArtifact {
            key,
            digest,
            size,
            image_digest,
            remote: None,
        };
        self.read(&artifact)?;
        Ok(artifact)
    }
}
