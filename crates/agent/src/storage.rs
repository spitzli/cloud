//! Separately privileged fixed-size filesystems, retaining the existing pool/volume identity format.
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use webdock_core::id::RecordId;
use webdock_supervisor::{ExecutionError, Result};
pub mod build;
#[cfg(feature = "kubernetes")]
pub mod build_kubernetes;
#[cfg(feature = "kubernetes")]
pub mod build_queue;
pub mod database;
pub mod recovery;
pub mod recovery_io;
pub mod recovery_pool;
const MIN: u64 = 67_108_864;
const MAX: u64 = 1_099_511_627_776;
const RECOVERY_FRAME: usize = 512 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageConfig {
    pub root: PathBuf,
    pub node: String,
    #[serde(rename = "clusterID")]
    pub cluster_id: RecordId,
    #[serde(rename = "poolID")]
    pub pool_id: String,
    pub capacity_bytes: u64,
    pub reserve_bytes: u64,
}
impl StorageConfig {
    pub fn validate(&self) -> Result<()> {
        if !self.root.is_absolute()
            || self.root.as_os_str().len() > 2048
            || self.node.is_empty()
            || self.node.len() > 253
            || !self.node.as_bytes()[0].is_ascii_alphanumeric()
            || !self
                .node
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
            || !valid_uuid(&self.pool_id)
            || !(MIN..=MAX).contains(&self.capacity_bytes)
            || !(MIN..=MAX).contains(&self.reserve_bytes)
        {
            return Err(ExecutionError::Invalid("storage configuration"));
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        require_root()?;
        let config: Self = read_json(path, 65536)?;
        config.validate()?;
        private(&config.root, true)?;
        Ok(config)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageSpec {
    pub volume_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoragePacket {
    #[serde(rename = "appID")]
    pub app_id: RecordId,
    #[serde(rename = "projectID")]
    pub project_id: RecordId,
    #[serde(rename = "clusterID")]
    pub cluster_id: RecordId,
    pub revision: u64,
    pub lease_until: DateTime<Utc>,
    pub spec: StorageSpec,
}
impl StoragePacket {
    pub fn new(
        app: &str,
        project: &str,
        cluster: &str,
        revision: u64,
        bytes: u64,
        lease_until: u64,
    ) -> Result<Self> {
        let packet = Self {
            app_id: RecordId::parse(app).map_err(|_| ExecutionError::Invalid("storage app"))?,
            project_id: RecordId::parse(project)
                .map_err(|_| ExecutionError::Invalid("storage project"))?,
            cluster_id: RecordId::parse(cluster)
                .map_err(|_| ExecutionError::Invalid("storage cluster"))?,
            revision,
            lease_until: DateTime::from_timestamp(
                i64::try_from(lease_until).map_err(|_| ExecutionError::Expired)?,
                0,
            )
            .ok_or(ExecutionError::Expired)?,
            spec: StorageSpec {
                volume_bytes: bytes,
            },
        };
        packet.validate()?;
        Ok(packet)
    }
    pub fn validate(&self) -> Result<()> {
        if self.revision == 0
            || self.revision > 9_007_199_254_740_991
            || !(MIN..=MAX).contains(&self.spec.volume_bytes)
        {
            return Err(ExecutionError::Invalid("storage packet"));
        }
        if self.lease_until <= Utc::now() {
            return Err(ExecutionError::Expired);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeState {
    Initializing,
    Ready,
    Purging,
    Purged,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeRecord {
    #[serde(rename = "appID")]
    pub app_id: RecordId,
    #[serde(rename = "projectID")]
    pub project_id: RecordId,
    #[serde(rename = "clusterID")]
    pub cluster_id: RecordId,
    pub bytes: u64,
    pub uuid: String,
    pub state: VolumeState,
}
impl VolumeRecord {
    fn validate(&self) -> Result<()> {
        if !(MIN..=MAX).contains(&self.bytes) || !valid_uuid(&self.uuid) {
            return Err(ExecutionError::Invalid("volume record"));
        }
        Ok(())
    }
    pub fn authorize(&self, packet: &StoragePacket) -> Result<()> {
        self.validate()?;
        if self.app_id != packet.app_id
            || self.project_id != packet.project_id
            || self.cluster_id != packet.cluster_id
            || self.bytes != packet.spec.volume_bytes
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolState {
    #[serde(rename = "poolID")]
    pool_id: String,
    volumes: BTreeMap<String, VolumeRecord>,
}
#[derive(Clone)]
pub struct StorageGuard {
    deadline: DateTime<Utc>,
    cancelled: Arc<AtomicBool>,
}
impl StorageGuard {
    pub fn new(deadline: DateTime<Utc>, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            deadline,
            cancelled,
        }
    }
    pub fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::SeqCst) || self.deadline <= Utc::now() {
            Err(ExecutionError::Expired)
        } else {
            Ok(())
        }
    }
    fn maintenance() -> Self {
        Self::new(
            Utc::now() + chrono::Duration::seconds(300),
            Arc::new(AtomicBool::new(false)),
        )
    }
}
fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|v| v.hyphenated().to_string() == value)
}
fn require_root() -> Result<()> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err(ExecutionError::Denied);
    }
    Ok(())
}
fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|error| {
        tracing::debug!(%error, "Storage filesystem operation failed");
        ExecutionError::Runtime
    })
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let meta = io(std::fs::symlink_metadata(path))?;
    if !path.is_absolute()
        || io(std::fs::canonicalize(path))? != path
        || meta.uid() != 0
        || meta.mode() & 0o077 != 0
        || (directory && !meta.is_dir())
        || (!directory && (!meta.is_file() || meta.nlink() != 1))
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    private(path, false)?;
    let file = io(OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path))?;
    let mut bytes = Vec::new();
    io(file.take(limit + 1).read_to_end(&mut bytes))?;
    if bytes.len() as u64 > limit {
        return Err(ExecutionError::Invalid("storage metadata limit"));
    }
    serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Invalid("storage metadata"))
}
fn sync_directory(path: &Path) -> Result<()> {
    io(io(File::open(path))?.sync_all())
}
fn write_state(directory: &Path, state: &PoolState) -> Result<()> {
    private(directory, true)?;
    let bytes = serde_json::to_vec(state).map_err(|_| ExecutionError::Runtime)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(ExecutionError::Invalid("storage metadata limit"));
    }
    let mut file = io(tempfile::NamedTempFile::new_in(directory))?;
    io(file.write_all(&bytes))?;
    io(file.as_file().sync_all())?;
    file.persist(directory.join("state.json"))
        .map_err(|_| ExecutionError::Runtime)?;
    sync_directory(directory)
}
fn mkdir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    io(std::fs::DirBuilder::new().mode(0o700).create(path))
}
struct Output {
    code: i32,
    stdout: String,
}
fn run(program: &str, args: &[&str], guard: &StorageGuard, seconds: u64) -> Result<Output> {
    guard.check()?;
    let mut child = io(Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn())?;
    let stdout = child.stdout.take().ok_or(ExecutionError::Runtime)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if guard.check().is_err() || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(ExecutionError::Expired);
        }
        if let Some(status) = io(child.try_wait())? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader
        .join()
        .map_err(|_| ExecutionError::Runtime)?
        .map_err(|_| ExecutionError::Runtime)?;
    if bytes.len() > 1024 * 1024 {
        return Err(ExecutionError::Runtime);
    }
    Ok(Output {
        code: status.code().ok_or(ExecutionError::Runtime)?,
        stdout: String::from_utf8(bytes)
            .map_err(|_| ExecutionError::Runtime)?
            .trim()
            .into(),
    })
}
fn checked(program: &str, args: &[&str], guard: &StorageGuard, seconds: u64) -> Result<String> {
    let output = run(program, args, guard, seconds)?;
    if output.code != 0 {
        tracing::warn!(
            program,
            exit_code = output.code,
            "Storage helper command failed"
        );
        return Err(ExecutionError::Runtime);
    }
    Ok(output.stdout)
}
fn path_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or(ExecutionError::Invalid("storage path"))
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    Apps,
    Database,
    Builds,
}
impl StorageKind {
    fn uid(self) -> u32 {
        match self {
            Self::Apps => 65532,
            Self::Database => 26,
            Self::Builds => 1000,
        }
    }
    fn mount_options(self) -> &'static str {
        match self {
            Self::Builds => "loop,nodev,nosuid",
            Self::Apps | Self::Database => "loop,nodev,nosuid,noexec",
        }
    }
}
// A missing directory is recoverable only before allocation, after exact global absence proof.
fn uncreated_build_loop_devices(
    directory: &Path,
    uuid: &str,
    mounts: &serde_json::Value,
    loops: &serde_json::Value,
) -> Result<Vec<String>> {
    if !directory.is_absolute() || !valid_uuid(uuid) {
        return Err(ExecutionError::Ownership);
    }
    let filesystems = mounts["filesystems"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?;
    if !filesystems.iter().any(|mount| mount["target"] == "/") {
        return Err(ExecutionError::Ownership);
    }
    for mount in filesystems {
        let target = mount["target"].as_str().ok_or(ExecutionError::Ownership)?;
        let target = Path::new(target.strip_suffix(" (deleted)").unwrap_or(target));
        let filesystem = mount["fstype"].as_str().ok_or(ExecutionError::Ownership)?;
        if !target.is_absolute()
            || target
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || mount.get("children").is_some()
            || filesystem.is_empty()
            || target.starts_with(directory)
            || mount["uuid"]
                .as_str()
                .is_some_and(|value| value.eq_ignore_ascii_case(uuid))
            || (filesystem == "ext4"
                && mount["uuid"]
                    .as_str()
                    .is_none_or(|value| !valid_uuid(value)))
        {
            return Err(ExecutionError::Ownership);
        }
    }
    let mut devices = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for device in loops["loopdevices"]
        .as_array()
        .ok_or(ExecutionError::Ownership)?
    {
        let name = device["name"].as_str().ok_or(ExecutionError::Ownership)?;
        let backing = device["back-file"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?;
        let backing = Path::new(backing.strip_suffix(" (deleted)").unwrap_or(backing));
        if !seen.insert(name)
            || !name
                .strip_prefix("/dev/loop")
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|v| v.is_ascii_digit()))
            || !backing.is_absolute()
            || backing.starts_with(directory)
            || backing
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(ExecutionError::Ownership);
        }
        devices.push(name.into());
    }
    Ok(devices)
}

pub struct StoragePool {
    kind: StorageKind,
    config: StorageConfig,
    config_path: PathBuf,
    _lock: File,
    poisoned: bool,
    recovery_trust: Option<(PathBuf, String, recovery_pool::RecoveryStorageTrust)>,
}
impl StoragePool {
    pub fn initialize(config_path: &Path) -> Result<()> {
        let config = StorageConfig::load(config_path)?;
        if io(std::fs::read_dir(&config.root))?.next().is_some() {
            return Err(ExecutionError::Conflict);
        }
        let pool = config.root.join("_pool");
        mkdir(&pool)?;
        write_state(
            &pool,
            &PoolState {
                pool_id: config.pool_id,
                volumes: BTreeMap::new(),
            },
        )?;
        sync_directory(&config.root)
    }
    pub fn open(config_path: &Path) -> Result<Self> {
        Self::open_kind(config_path, StorageKind::Apps)
    }
    pub fn open_kind(config_path: &Path, kind: StorageKind) -> Result<Self> {
        let config = StorageConfig::load(config_path)?;
        let pool = config.root.join("_pool");
        private(&pool, true)?;
        let lock = io(OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(pool.join("service.lock")))?;
        private(&pool.join("service.lock"), false)?;
        lock.try_lock_exclusive()
            .map_err(|_| ExecutionError::Conflict)?;
        let instance = Self {
            kind,
            config,
            config_path: config_path.into(),
            _lock: lock,
            poisoned: false,
            recovery_trust: None,
        };
        instance.state()?;
        let marker = pool.join("service-kind.json");
        if marker.try_exists().map_err(|_| ExecutionError::Runtime)? {
            if read_json::<StorageKind>(&marker, 64)? != kind {
                return Err(ExecutionError::Ownership);
            }
        } else {
            // Validate migrated ready filesystems before binding this pool to a service.
            for record in instance.state()?.volumes.values() {
                if record.state == VolumeState::Ready {
                    instance.mount_record(
                        &instance.config.root.join(record.app_id.as_str()),
                        record,
                        &StorageGuard::maintenance(),
                    )?;
                }
            }
            let mut file = io(tempfile::NamedTempFile::new_in(&pool))?;
            io(file.write_all(&serde_json::to_vec(&kind).map_err(|_| ExecutionError::Runtime)?))?;
            io(file.as_file().sync_all())?;
            file.persist_noclobber(marker)
                .map_err(|_| ExecutionError::Conflict)?;
            sync_directory(&pool)?;
        }
        Ok(instance)
    }
    pub fn configuration(&self) -> &StorageConfig {
        &self.config
    }
    fn state(&self) -> Result<PoolState> {
        if self.poisoned {
            return Err(ExecutionError::Journal);
        }
        if StorageConfig::load(&self.config_path)? != self.config {
            return Err(ExecutionError::Conflict);
        }
        let state: PoolState =
            read_json(&self.config.root.join("_pool/state.json"), 64 * 1024 * 1024)?;
        if state.pool_id != self.config.pool_id {
            return Err(ExecutionError::Ownership);
        }
        for (key, record) in &state.volumes {
            record.validate()?;
            if key != record.app_id.as_str()
                || record.cluster_id != self.config.cluster_id
                || (self.kind == StorageKind::Database && record.app_id != record.project_id)
            {
                return Err(ExecutionError::Ownership);
            }
        }
        for entry in io(std::fs::read_dir(&self.config.root))? {
            let entry = io(entry)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ExecutionError::Ownership)?;
            if name != "_pool" && !state.volumes.contains_key(&name) {
                self.validate_recovery_namespace(&name)?;
            }
        }
        Ok(state)
    }
    fn record(&self, app: &RecordId) -> Result<Option<VolumeRecord>> {
        Ok(self.state()?.volumes.get(app.as_str()).cloned())
    }
    fn persist(&mut self, record: VolumeRecord) -> Result<()> {
        let mut state = self.state()?;
        state.volumes.insert(record.app_id.to_string(), record);
        if write_state(&self.config.root.join("_pool"), &state).is_err() {
            self.poisoned = true;
            return Err(ExecutionError::Journal);
        }
        Ok(())
    }
    fn mounted(&self, directory: &Path, guard: &StorageGuard) -> Result<bool> {
        let target = directory.join("mount");
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ExecutionError::Ownership);
        }
        let output = run(
            "/usr/bin/findmnt",
            &[
                "--json",
                "--mountpoint",
                path_str(&target)?,
                "-o",
                "SOURCE,TARGET,FSTYPE,OPTIONS,UUID",
            ],
            guard,
            10,
        )?;
        if output.code == 1 {
            return Ok(false);
        }
        if output.code != 0 {
            return Err(ExecutionError::Runtime);
        }
        let value: serde_json::Value =
            serde_json::from_str(&output.stdout).map_err(|_| ExecutionError::Runtime)?;
        let mounts = value["filesystems"]
            .as_array()
            .ok_or(ExecutionError::Runtime)?;
        if mounts.len() != 1 || mounts[0]["fstype"] != "ext4" {
            return Err(ExecutionError::Ownership);
        }
        if self.kind == StorageKind::Builds {
            build::validate_mount(&mounts[0], directory, guard)?;
        }
        let source = mounts[0]["source"]
            .as_str()
            .ok_or(ExecutionError::Ownership)?;
        if !source
            .strip_prefix("/dev/loop")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(ExecutionError::Ownership);
        }
        let loops = checked(
            "/usr/sbin/losetup",
            &["--json", "--list", "--output", "NAME,BACK-FILE", source],
            guard,
            10,
        )?;
        let loops: serde_json::Value =
            serde_json::from_str(&loops).map_err(|_| ExecutionError::Runtime)?;
        let loops = loops["loopdevices"]
            .as_array()
            .ok_or(ExecutionError::Runtime)?;
        if loops.len() != 1 || loops[0]["back-file"] != path_str(&directory.join("volume.ext4"))? {
            return Err(ExecutionError::Ownership);
        }
        Ok(true)
    }
    fn image(
        &self,
        directory: &Path,
        record: &VolumeRecord,
        guard: &StorageGuard,
    ) -> Result<PathBuf> {
        let image = directory.join("volume.ext4");
        private(&image, false)?;
        if io(std::fs::metadata(&image))?.len() != record.bytes {
            return Err(ExecutionError::Ownership);
        }
        let uuid = checked(
            "/usr/sbin/blkid",
            &["-p", "-s", "UUID", "-o", "value", path_str(&image)?],
            guard,
            10,
        )?;
        if uuid != record.uuid {
            return Err(ExecutionError::Ownership);
        }
        Ok(image)
    }
    fn mount_record(
        &self,
        directory: &Path,
        record: &VolumeRecord,
        guard: &StorageGuard,
    ) -> Result<PathBuf> {
        if record.state != VolumeState::Ready {
            return Err(ExecutionError::Conflict);
        }
        private(directory, true)?;
        let image = self.image(directory, record, guard)?;
        if !self.mounted(directory, guard)? {
            let target = directory.join("mount");
            match std::fs::symlink_metadata(&target) {
                Ok(meta) => {
                    if !meta.is_dir() || io(std::fs::read_dir(&target))?.next().is_some() {
                        return Err(ExecutionError::Ownership);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => mkdir(&target)?,
                Err(_) => return Err(ExecutionError::Runtime),
            };
            checked(
                "/usr/bin/mount",
                &[
                    "-t",
                    "ext4",
                    "-o",
                    self.kind.mount_options(),
                    path_str(&image)?,
                    path_str(&target)?,
                ],
                guard,
                45,
            )?;
        }
        if !self.mounted(directory, guard)? {
            return Err(ExecutionError::Runtime);
        }
        let data = directory.join("mount/data");
        let metadata = io(std::fs::symlink_metadata(&data))?;
        if !metadata.is_dir()
            || io(std::fs::canonicalize(&data))? != data
            || metadata.uid() != self.kind.uid()
            || metadata.gid() != self.kind.uid()
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(data)
    }
    pub fn ensure(
        &mut self,
        packet: &StoragePacket,
        guard: &StorageGuard,
    ) -> Result<(StorageConfig, PathBuf)> {
        if self.kind == StorageKind::Builds {
            return Err(ExecutionError::Denied);
        }
        self.ensure_reserved(packet, guard, 0)
    }
    pub(super) fn ensure_reserved(
        &mut self,
        packet: &StoragePacket,
        guard: &StorageGuard,
        already_reserved: u64,
    ) -> Result<(StorageConfig, PathBuf)> {
        packet.validate()?;
        guard.check()?;
        if packet.cluster_id != self.config.cluster_id
            || (self.kind == StorageKind::Database && packet.app_id != packet.project_id)
        {
            return Err(ExecutionError::Ownership);
        }
        let directory = self.config.root.join(packet.app_id.as_str());
        if let Some(record) = self.record(&packet.app_id)? {
            record.authorize(packet)?;
            return Ok((
                self.config.clone(),
                self.mount_record(&directory, &record, guard)?,
            ));
        }
        if packet.revision != 1 {
            return Err(ExecutionError::Ownership);
        }
        let used = self
            .state()?
            .volumes
            .values()
            .filter(|r| r.state != VolumeState::Purged)
            .try_fold(0u64, |total, r| total.checked_add(r.bytes))
            .ok_or(ExecutionError::Runtime)?
            .checked_add(self.recovery_reserved_bytes()?)
            .and_then(|used| used.checked_sub(already_reserved))
            .ok_or(ExecutionError::Runtime)?;
        let available =
            rustix::fs::statvfs(&self.config.root).map_err(|_| ExecutionError::Runtime)?;
        let free = available
            .f_bavail
            .checked_mul(available.f_frsize)
            .ok_or(ExecutionError::Runtime)?;
        if used
            .checked_add(packet.spec.volume_bytes)
            .is_none_or(|total| total > self.config.capacity_bytes)
            || free < packet.spec.volume_bytes + self.config.reserve_bytes
        {
            return Err(ExecutionError::Runtime);
        }
        let mut record = VolumeRecord {
            app_id: packet.app_id.clone(),
            project_id: packet.project_id.clone(),
            cluster_id: packet.cluster_id.clone(),
            bytes: packet.spec.volume_bytes,
            uuid: uuid::Uuid::new_v4().hyphenated().to_string(),
            state: VolumeState::Initializing,
        };
        self.persist(record.clone())?;
        guard.check()?;
        mkdir(&directory)?;
        let image = directory.join("volume.ext4");
        let file = io(OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&image))?;
        guard.check()?;
        rustix::fs::fallocate(&file, rustix::fs::FallocateFlags::empty(), 0, record.bytes)
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
                &record.uuid,
                "-E",
                "nodiscard,lazy_itable_init=0,lazy_journal_init=0",
                path_str(&image)?,
            ],
            guard,
            45,
        )?;
        let target = directory.join("mount");
        guard.check()?;
        mkdir(&target)?;
        checked(
            "/usr/bin/mount",
            &[
                "-t",
                "ext4",
                "-o",
                self.kind.mount_options(),
                path_str(&image)?,
                path_str(&target)?,
            ],
            guard,
            45,
        )?;
        if !self.mounted(&directory, guard)? {
            return Err(ExecutionError::Runtime);
        }
        let data = target.join("data");
        guard.check()?;
        mkdir(&data)?;
        io(std::fs::set_permissions(
            &data,
            std::fs::Permissions::from_mode(0o770),
        ))?;
        io(std::os::unix::fs::chown(
            &data,
            Some(self.kind.uid()),
            Some(self.kind.uid()),
        ))?;
        checked("/usr/bin/sync", &["-f", path_str(&target)?], guard, 45)?;
        sync_directory(&directory)?;
        sync_directory(&self.config.root)?;
        guard.check()?;
        record.state = VolumeState::Ready;
        self.persist(record.clone())?;
        Ok((
            self.config.clone(),
            self.mount_record(&directory, &record, guard)?,
        ))
    }
    pub fn retained(&self, packet: &StoragePacket) -> Result<()> {
        if self.kind == StorageKind::Builds {
            return Err(ExecutionError::Denied);
        }
        self.retained_inner(packet)
    }
    fn retained_inner(&self, packet: &StoragePacket) -> Result<()> {
        packet.validate()?;
        if packet.cluster_id != self.config.cluster_id
            || (self.kind == StorageKind::Database && packet.app_id != packet.project_id)
        {
            return Err(ExecutionError::Ownership);
        }
        if let Some(record) = self.record(&packet.app_id)? {
            record.authorize(packet)?;
        }
        Ok(())
    }
    /// Inspect an existing allocation without mounting, repairing, or deleting anything.
    pub fn volume_state(
        &self,
        packet: &StoragePacket,
        guard: &StorageGuard,
    ) -> Result<VolumeState> {
        if self.kind == StorageKind::Builds {
            return Err(ExecutionError::Denied);
        }
        self.volume_state_inner(packet, guard)
    }
    fn volume_state_inner(
        &self,
        packet: &StoragePacket,
        guard: &StorageGuard,
    ) -> Result<VolumeState> {
        self.retained_inner(packet)?;
        guard.check()?;
        let record = self
            .record(&packet.app_id)?
            .ok_or(ExecutionError::Ownership)?;
        record.authorize(packet)?;
        let directory = self.config.root.join(packet.app_id.as_str());
        private(&directory, true)?;
        match record.state {
            VolumeState::Ready => {
                self.image(&directory, &record, guard)?;
                if !self.mounted(&directory, guard)? {
                    return Err(ExecutionError::Ownership);
                }
                let data = io(std::fs::symlink_metadata(directory.join("mount/data")))?;
                if !data.is_dir() || data.uid() != self.kind.uid() || data.gid() != self.kind.uid()
                {
                    return Err(ExecutionError::Ownership);
                }
            }
            VolumeState::Purged => {
                if io(std::fs::read_dir(&directory))?.next().is_some() {
                    return Err(ExecutionError::Ownership);
                }
            }
            VolumeState::Initializing | VolumeState::Purging => {
                return Err(ExecutionError::Conflict);
            }
        }
        guard.check()?;
        Ok(record.state)
    }
    pub fn purge(&mut self, packet: &StoragePacket, guard: &StorageGuard) -> Result<()> {
        if self.kind == StorageKind::Builds {
            return Err(ExecutionError::Denied);
        }
        self.purge_record(packet, guard)
    }
    fn purge_record(&mut self, packet: &StoragePacket, guard: &StorageGuard) -> Result<()> {
        self.retained_inner(packet)?;
        guard.check()?;
        let Some(mut record) = self.record(&packet.app_id)? else {
            return Ok(());
        };
        if record.state == VolumeState::Purged {
            return Ok(());
        }
        let directory = self.config.root.join(packet.app_id.as_str());
        if self.kind == StorageKind::Builds
            && record.state == VolumeState::Initializing
            && std::fs::symlink_metadata(&directory)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            private(&self.config.root, true)?;
            let mounts = checked(
                "/usr/bin/findmnt",
                &["--json", "--list", "-o", "TARGET,FSTYPE,UUID"],
                guard,
                10,
            )?;
            let loops = checked(
                "/usr/sbin/losetup",
                &["--json", "--list", "--output", "NAME,BACK-FILE"],
                guard,
                10,
            )?;
            let mounts = serde_json::from_str(&mounts).map_err(|_| ExecutionError::Runtime)?;
            let loops = serde_json::from_str(&loops).map_err(|_| ExecutionError::Runtime)?;
            for device in uncreated_build_loop_devices(&directory, &record.uuid, &mounts, &loops)? {
                // Unmounted or renamed/deleted backing files still retain a UUID through their loop.
                let uuid = checked(
                    "/usr/sbin/blkid",
                    &["-p", "-s", "UUID", "-o", "value", &device],
                    guard,
                    10,
                )?;
                if !valid_uuid(&uuid) || uuid.eq_ignore_ascii_case(&record.uuid) {
                    return Err(ExecutionError::Ownership);
                }
            }
            let current = self
                .record(&packet.app_id)?
                .ok_or(ExecutionError::Ownership)?;
            current.authorize(packet)?;
            if current.uuid != record.uuid
                || current.state != VolumeState::Initializing
                || !std::fs::symlink_metadata(&directory)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(ExecutionError::Ownership);
            }
            guard.check()?;
            record.state = VolumeState::Purged;
            return self.persist(record);
        }
        private(&directory, true)?;
        for entry in io(std::fs::read_dir(&directory))? {
            let name = io(entry)?.file_name();
            if name != "volume.ext4" && name != "mount" {
                return Err(ExecutionError::Ownership);
            }
        }
        let image = directory.join("volume.ext4");
        if record.state == VolumeState::Ready {
            self.image(&directory, &record, guard)?;
        }
        if std::fs::symlink_metadata(&image).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ExecutionError::Ownership);
        }
        if record.state != VolumeState::Purging {
            record.state = VolumeState::Purging;
            self.persist(record.clone())?;
        }
        if self.mounted(&directory, guard)? {
            checked(
                "/usr/bin/umount",
                &[path_str(&directory.join("mount"))?],
                guard,
                45,
            )?;
        }
        if image.try_exists().map_err(|_| ExecutionError::Runtime)? {
            private(&image, false)?;
            let output = checked(
                "/usr/sbin/losetup",
                &["--json", "--list", "--associated", path_str(&image)?],
                guard,
                10,
            )?;
            let loops: serde_json::Value =
                serde_json::from_str(&output).map_err(|_| ExecutionError::Runtime)?;
            if !loops["loopdevices"]
                .as_array()
                .ok_or(ExecutionError::Runtime)?
                .is_empty()
            {
                return Err(ExecutionError::Conflict);
            }
            guard.check()?;
            io(std::fs::remove_file(&image))?;
        }
        let target = directory.join("mount");
        if target.try_exists().map_err(|_| ExecutionError::Runtime)? {
            io(std::fs::remove_dir(&target))?;
        }
        sync_directory(&directory)?;
        guard.check()?;
        record.state = VolumeState::Purged;
        self.persist(record)
    }
    pub fn restore(&self) -> Result<()> {
        // A build allocation needs fresh signed authority and node/PVC reconciliation.
        // Never remount canceled or expired attempts through the legacy restore path.
        if self.kind == StorageKind::Builds {
            return Ok(());
        }
        let guard = StorageGuard::maintenance();
        for record in self.state()?.volumes.values() {
            if record.state == VolumeState::Ready
                && self
                    .mount_record(
                        &self.config.root.join(record.app_id.as_str()),
                        record,
                        &guard,
                    )
                    .is_err()
            {
                tracing::warn!(app_id=%record.app_id,"Retained volume needs reconciliation");
            }
        }
        Ok(())
    }
    pub fn capability(&self, nodes: &[String]) -> Result<u64> {
        if nodes != [self.config.node.clone()] {
            return Err(ExecutionError::Ownership);
        }
        let guard = StorageGuard::maintenance();
        for record in self.state()?.volumes.values() {
            if !matches!(record.state, VolumeState::Ready | VolumeState::Purged) {
                return Err(ExecutionError::Journal);
            }
            if record.state == VolumeState::Ready {
                let directory = self.config.root.join(record.app_id.as_str());
                self.image(&directory, record, &guard)?;
                if !self.mounted(&directory, &guard)? {
                    return Err(ExecutionError::Runtime);
                }
            }
        }
        Ok(self.config.capacity_bytes)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcRequest {
    action: String,
    value: serde_json::Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpcResponse {
    ok: bool,
    value: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageCapability {
    pub capacity_bytes: u64,
    pub ready: bool,
}
fn dispatch(
    pool: &mut StoragePool,
    request: RpcRequest,
    cancelled: Arc<AtomicBool>,
) -> Result<serde_json::Value> {
    if pool.kind == StorageKind::Builds {
        return Err(ExecutionError::Unsupported(
            "builds require signed storage authority",
        ));
    }
    if pool.kind == StorageKind::Database {
        return database::dispatch(pool, request, cancelled);
    }
    match request.action.as_str() {
        "info" => {
            if !request.value.is_null() {
                return Err(ExecutionError::Invalid("storage info"));
            }
            pool.state()?;
            serde_json::to_value(pool.configuration()).map_err(|_| ExecutionError::Runtime)
        }
        "recovery.record" | "recovery.install-ce" => {
            let value: recovery_pool::RecoveryVolumeRequest = serde_json::from_value(request.value)
                .map_err(|_| ExecutionError::Invalid("recovery volume"))?;
            let guard = StorageGuard::new(Utc::now() + chrono::Duration::seconds(300), cancelled);
            let result = if request.action == "recovery.record" {
                pool.recovery_volume_record(&value, &guard)?
            } else {
                pool.install_recovery_volume(&value, &guard)?
            };
            serde_json::to_value(result).map_err(|_| ExecutionError::Runtime)
        }
        #[cfg(feature = "docker")]
        "recovery.reserve" => {
            recovery_pool::reservation_marker("helper", "enter", "none");
            let result = (|| {
                let request: recovery_pool::RecoveryReserveRequest =
                    serde_json::from_value(request.value)
                        .map_err(|_| ExecutionError::Invalid("recovery reservation"))?;
                pool.reserve_recovery(&request, cancelled)
            })();
            recovery_pool::reservation_marker(
                "helper",
                "result",
                recovery_pool::reservation_result_variant(&result),
            );
            serde_json::to_value(result?).map_err(|_| ExecutionError::Runtime)
        }
        "recovery.pool-info" => {
            if !request.value.is_null() {
                return Err(ExecutionError::Invalid("recovery pool info"));
            }
            serde_json::to_value(pool.recovery_pool_identity()?)
                .map_err(|_| ExecutionError::Runtime)
        }
        "capability" => {
            let nodes: Vec<String> = serde_json::from_value(request.value)
                .map_err(|_| ExecutionError::Invalid("storage nodes"))?;
            if nodes.len() > 200 || nodes.iter().any(|n| n.len() > 253) {
                return Err(ExecutionError::Invalid("storage nodes"));
            }
            let capacity = pool.capability(&nodes);
            Ok(
                serde_json::json!({"capacityBytes":capacity.as_ref().copied().unwrap_or(0),"ready":capacity.is_ok()}),
            )
        }
        "ensure" | "retained" | "purge" | "state" => {
            let packet: StoragePacket = serde_json::from_value(request.value)
                .map_err(|_| ExecutionError::Invalid("storage packet"))?;
            packet.validate()?;
            let guard = StorageGuard::new(packet.lease_until, cancelled);
            match request.action.as_str() {
                "ensure" => serde_json::to_value(pool.ensure(&packet, &guard)?)
                    .map_err(|_| ExecutionError::Runtime),
                "retained" => {
                    pool.retained(&packet)?;
                    Ok(serde_json::Value::Null)
                }
                "purge" => {
                    pool.purge(&packet, &guard)?;
                    Ok(serde_json::Value::Null)
                }
                "state" => serde_json::to_value(pool.volume_state(&packet, &guard)?)
                    .map_err(|_| ExecutionError::Runtime),
                _ => unreachable!(),
            }
        }
        "application.snapshot"
        | "application.install"
        | "application.record"
        | "application.staging" => {
            let packet: recovery_io::ApplicationRecoveryPacket =
                serde_json::from_value(request.value)
                    .map_err(|_| ExecutionError::Invalid("application recovery packet"))?;
            packet.source.validate()?;
            let guard = StorageGuard::new(packet.source.lease_until, cancelled);
            let value = match request.action.as_str() {
                "application.snapshot" => {
                    serde_json::to_value(pool.snapshot_application(&packet, &guard)?)
                }
                "application.install" => {
                    serde_json::to_value(pool.install_application(&packet, &guard)?)
                }
                "application.record" => {
                    if pool.recovery_peer_enabled() {
                        pool.recovery_source_authorized(&packet, &guard)?;
                    }
                    pool.volume_state(&packet.source, &guard)?;
                    serde_json::to_value(
                        pool.record(&packet.source.app_id)?
                            .ok_or(ExecutionError::Ownership)?,
                    )
                }
                "application.staging" => {
                    serde_json::to_value(pool.application_recovery_staging(&packet, &guard)?)
                }
                _ => unreachable!(),
            };
            value.map_err(|_| ExecutionError::Runtime)
        }
        _ => Err(ExecutionError::Unsupported("storage action")),
    }
}
async fn connection(
    stream: tokio::net::UnixStream,
    pool: Arc<std::sync::Mutex<StoragePool>>,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let peer_uid = stream
        .peer_cred()
        .map_err(|_| ExecutionError::Denied)?
        .uid();
    let recovery_peer = pool
        .lock()
        .map_err(|_| ExecutionError::Journal)?
        .recovery_peer_enabled();
    if peer_uid != 0 && !(peer_uid == 65532 && recovery_peer) {
        return Err(ExecutionError::Denied);
    }
    let (read, mut write) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read);
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        (&mut reader)
            .take((RECOVERY_FRAME + 1) as u64)
            .read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| ExecutionError::Expired)?
    .map_err(|_| ExecutionError::Runtime)?;
    if bytes.len() > RECOVERY_FRAME || !bytes.ends_with(b"\n") {
        return Err(ExecutionError::Invalid("storage frame"));
    }
    let request: RpcRequest =
        serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Invalid("storage request"))?;
    let recovery_action = recovery_pool::recovery_action(&request.action);
    if !recovery_action && bytes.len() > 4096 {
        return Err(ExecutionError::Invalid("legacy storage frame"));
    }
    if peer_uid == 65532 && !(recovery_action || request.action == "info") {
        return Err(ExecutionError::Denied);
    }
    let response_limit = if recovery_action {
        RECOVERY_FRAME
    } else {
        8192
    };
    if shutdown.load(Ordering::SeqCst) {
        return Err(ExecutionError::Expired);
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let work_cancel = cancelled.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        let mut pool = pool.lock().map_err(|_| ExecutionError::Journal)?;
        dispatch(&mut pool, request, work_cancel)
    });
    let mut extra = [0u8; 1];
    let result = loop {
        tokio::select! {
            result=&mut worker=>break result.map_err(|_|ExecutionError::Runtime)?,
            _=reader.read(&mut extra)=>{cancelled.store(true,Ordering::SeqCst);let _=worker.await;return Err(ExecutionError::Expired);},
            _=tokio::time::sleep(Duration::from_millis(50))=>if shutdown.load(Ordering::SeqCst){cancelled.store(true,Ordering::SeqCst);let _=worker.await;return Err(ExecutionError::Expired);},
        }
    };
    let response = match result {
        Ok(value) => RpcResponse {
            ok: true,
            value,
            error: None,
        },
        Err(ExecutionError::Unsupported(_)) => RpcResponse {
            ok: false,
            value: serde_json::Value::Null,
            error: Some("not_supported".into()),
        },
        Err(error) => {
            tracing::debug!(%error, "Storage request refused");
            RpcResponse {
                ok: false,
                value: serde_json::Value::Null,
                error: None,
            }
        }
    };
    let mut bytes = serde_json::to_vec(&response).map_err(|_| ExecutionError::Runtime)?;
    bytes.push(b'\n');
    if bytes.len() > response_limit {
        return Err(ExecutionError::Runtime);
    }
    tokio::time::timeout(Duration::from_secs(10), write.write_all(&bytes))
        .await
        .map_err(|_| ExecutionError::Expired)?
        .map_err(|_| ExecutionError::Runtime)
}
struct SocketOwner {
    path: PathBuf,
    inode: u64,
    device: u64,
}
impl Drop for SocketOwner {
    fn drop(&mut self) {
        if let Ok(meta) = std::fs::symlink_metadata(&self.path)
            && meta.ino() == self.inode
            && meta.dev() == self.device
            && meta.file_type().is_socket()
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
/// Root-only Unix RPC, compatible with existing app storage clients. Never opens a TCP listener.
pub async fn serve(
    config_path: &Path,
    socket_path: &Path,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    serve_kind(config_path, socket_path, StorageKind::Apps, shutdown).await
}
pub async fn serve_kind(
    config_path: &Path,
    socket_path: &Path,
    kind: StorageKind,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    serve_kind_with_recovery(config_path, socket_path, kind, None, shutdown).await
}
pub async fn serve_kind_with_recovery(
    config_path: &Path,
    socket_path: &Path,
    kind: StorageKind,
    recovery_trust: Option<&Path>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    require_root()?;
    let mut pool = StoragePool::open_kind(config_path, kind)?;
    if let Some(path) = recovery_trust {
        pool = pool.with_recovery_trust(path)?;
    }
    pool.restore()?;
    let recovery_socket = recovery_trust.is_some();
    let socket_parent = socket_path
        .parent()
        .ok_or(ExecutionError::Invalid("storage socket"))?;
    if recovery_socket {
        pool.install_recovery_socket_permissions(socket_parent)?;
    } else {
        private(socket_parent, true)?;
    }
    match std::fs::symlink_metadata(socket_path) {
        Ok(meta) => {
            if !meta.file_type().is_socket()
                || meta.uid() != 0
                || if recovery_socket {
                    meta.gid() != 65532 || meta.mode() & 0o777 != 0o660
                } else {
                    meta.mode() & 0o077 != 0
                }
            {
                return Err(ExecutionError::Ownership);
            }
            match tokio::net::UnixStream::connect(socket_path).await {
                Ok(_) => return Err(ExecutionError::Conflict),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
                Err(_) => return Err(ExecutionError::Conflict),
            }
            let current = io(std::fs::symlink_metadata(socket_path))?;
            if current.ino() != meta.ino() || current.dev() != meta.dev() {
                return Err(ExecutionError::Conflict);
            }
            io(std::fs::remove_file(socket_path))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ExecutionError::Runtime),
    }
    let listener = io(tokio::net::UnixListener::bind(socket_path))?;
    if recovery_socket {
        io(std::os::unix::fs::chown(socket_path, Some(0), Some(65532)))?;
    }
    io(std::fs::set_permissions(
        socket_path,
        std::fs::Permissions::from_mode(if recovery_socket { 0o660 } else { 0o600 }),
    ))?;
    let meta = io(std::fs::symlink_metadata(socket_path))?;
    let _socket = SocketOwner {
        path: socket_path.into(),
        inode: meta.ino(),
        device: meta.dev(),
    };
    let pool = Arc::new(std::sync::Mutex::new(pool));
    let stopping = Arc::new(AtomicBool::new(false));
    let permits = Arc::new(tokio::sync::Semaphore::new(8));
    let mut handlers = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _=&mut shutdown=>break,
            accepted=listener.accept()=>{
                let (stream,_)=accepted.map_err(|_|ExecutionError::Runtime)?;
                let Ok(permit)=permits.clone().try_acquire_owned() else{drop(stream);continue};let pool=pool.clone();let stop=stopping.clone();handlers.spawn(async move{let _permit=permit;let _=connection(stream,pool,stop).await;});
            },
            _=handlers.join_next(),if !handlers.is_empty()=>{},
        }
    }
    stopping.store(true, Ordering::SeqCst);
    drop(listener);
    while handlers.join_next().await.is_some() {}
    Ok(())
}
#[derive(Clone)]
pub struct StorageClient {
    socket: PathBuf,
}
impl StorageClient {
    pub async fn install_recovery_volume(
        &self,
        request: &recovery_pool::RecoveryVolumeRequest,
    ) -> Result<recovery_pool::RecoveryVolumeRecord> {
        self.call(
            "recovery.install-ce",
            serde_json::to_value(request).map_err(|_| ExecutionError::Ownership)?,
        )
        .await
    }
    pub async fn recovery_volume_record(
        &self,
        request: &recovery_pool::RecoveryVolumeRequest,
    ) -> Result<recovery_pool::RecoveryVolumeRecord> {
        self.call(
            "recovery.record",
            serde_json::to_value(request).map_err(|_| ExecutionError::Ownership)?,
        )
        .await
    }
    pub async fn reserve_recovery(
        &self,
        request: &recovery_pool::RecoveryReserveRequest,
    ) -> Result<recovery_pool::RecoveryReservationReceipt> {
        self.call(
            "recovery.reserve",
            serde_json::to_value(request).map_err(|_| ExecutionError::Ownership)?,
        )
        .await
    }
    pub async fn recovery_pool_identity(&self) -> Result<recovery_pool::RecoveryPoolIdentity> {
        self.call("recovery.pool-info", serde_json::Value::Null)
            .await
    }
    pub async fn snapshot_application(
        &self,
        packet: &recovery_io::ApplicationRecoveryPacket,
    ) -> Result<recovery_io::ApplicationRecoveryResult> {
        self.call(
            "application.snapshot",
            serde_json::to_value(packet)
                .map_err(|_| ExecutionError::Invalid("application recovery packet"))?,
        )
        .await
    }
    pub async fn install_application(
        &self,
        packet: &recovery_io::ApplicationRecoveryPacket,
    ) -> Result<recovery_io::ApplicationRecoveryResult> {
        self.call(
            "application.install",
            serde_json::to_value(packet)
                .map_err(|_| ExecutionError::Invalid("application recovery packet"))?,
        )
        .await
    }
    pub async fn application_record(
        &self,
        packet: &recovery_io::ApplicationRecoveryPacket,
    ) -> Result<VolumeRecord> {
        self.call(
            "application.record",
            serde_json::to_value(packet)
                .map_err(|_| ExecutionError::Invalid("application recovery packet"))?,
        )
        .await
    }
    pub async fn application_staging(
        &self,
        packet: &recovery_io::ApplicationRecoveryPacket,
    ) -> Result<PathBuf> {
        self.call(
            "application.staging",
            serde_json::to_value(packet)
                .map_err(|_| ExecutionError::Invalid("application recovery packet"))?,
        )
        .await
    }
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        action: &str,
        value: serde_json::Value,
    ) -> Result<T> {
        use std::os::unix::fs::FileTypeExt;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        let parent = self.socket.parent().ok_or(ExecutionError::Ownership)?;
        let meta = io(std::fs::symlink_metadata(&self.socket))?;
        if !meta.file_type().is_socket()
            || meta.uid() != 0
            || io(std::fs::canonicalize(&self.socket))? != self.socket
        {
            return Err(ExecutionError::Ownership);
        }
        let recovery_socket = meta.gid() == 65532 && meta.mode() & 0o777 == 0o660;
        if recovery_socket {
            recovery_pool::recovery_socket_parent(parent, true)?;
        } else {
            private(parent, true)?;
            if meta.mode() & 0o077 != 0 {
                return Err(ExecutionError::Ownership);
            }
        }
        if rustix::process::geteuid().as_raw() == 65532
            && !(recovery_socket && (recovery_pool::recovery_action(action) || action == "info"))
        {
            return Err(ExecutionError::Denied);
        }
        let request_limit = if recovery_pool::recovery_action(action) {
            RECOVERY_FRAME
        } else {
            4096
        };
        let response_limit = if recovery_pool::recovery_action(action) {
            RECOVERY_FRAME
        } else {
            8192
        };
        let mut request = serde_json::to_vec(&serde_json::json!({"action":action,"value":value}))
            .map_err(|_| ExecutionError::Invalid("storage request"))?;
        request.push(b'\n');
        if request.len() > request_limit {
            return Err(ExecutionError::Invalid("storage frame"));
        }
        let work = async {
            let mut stream = tokio::net::UnixStream::connect(&self.socket)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if stream
                .peer_cred()
                .map_err(|_| ExecutionError::Denied)?
                .uid()
                != 0
            {
                return Err(ExecutionError::Denied);
            }
            stream
                .write_all(&request)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            let mut reader = tokio::io::BufReader::new(stream).take((response_limit + 1) as u64);
            let mut bytes = Vec::new();
            reader
                .read_until(b'\n', &mut bytes)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            if bytes.len() > response_limit || !bytes.ends_with(b"\n") {
                return Err(ExecutionError::Invalid("storage response"));
            }
            let response: RpcResponse = serde_json::from_slice(&bytes)
                .map_err(|_| ExecutionError::Invalid("storage response"))?;
            if !response.ok {
                return Err(if response.error.as_deref() == Some("not_supported") {
                    ExecutionError::Unsupported("storage RPC")
                } else {
                    ExecutionError::Runtime
                });
            }
            serde_json::from_value(response.value)
                .map_err(|_| ExecutionError::Invalid("storage response"))
        };
        tokio::time::timeout(Duration::from_secs(100), work)
            .await
            .map_err(|_| ExecutionError::Expired)?
    }
    pub async fn information(&self) -> Result<StorageConfig> {
        let config: StorageConfig = self.call("info", serde_json::Value::Null).await?;
        config.validate()?;
        private(&config.root, true)?;
        Ok(config)
    }
    pub async fn capability(&self, nodes: &[String]) -> Result<StorageCapability> {
        self.call(
            "capability",
            serde_json::to_value(nodes).map_err(|_| ExecutionError::Invalid("nodes"))?,
        )
        .await
    }
    pub async fn ensure(&self, packet: &StoragePacket) -> Result<(StorageConfig, PathBuf)> {
        packet.validate()?;
        let (config, path): (StorageConfig, PathBuf) = self
            .call(
                "ensure",
                serde_json::to_value(packet)
                    .map_err(|_| ExecutionError::Invalid("storage packet"))?,
            )
            .await?;
        config.validate()?;
        if config.cluster_id != packet.cluster_id
            || path != config.root.join(packet.app_id.as_str()).join("mount/data")
        {
            return Err(ExecutionError::Ownership);
        }
        Ok((config, path))
    }
    pub async fn retained(&self, packet: &StoragePacket) -> Result<()> {
        packet.validate()?;
        self.call(
            "retained",
            serde_json::to_value(packet).map_err(|_| ExecutionError::Invalid("storage packet"))?,
        )
        .await
    }
    pub async fn volume_state(&self, packet: &StoragePacket) -> Result<VolumeState> {
        packet.validate()?;
        self.call(
            "state",
            serde_json::to_value(packet).map_err(|_| ExecutionError::Invalid("storage packet"))?,
        )
        .await
    }
    pub async fn purge(&self, packet: &StoragePacket) -> Result<()> {
        packet.validate()?;
        self.call(
            "purge",
            serde_json::to_value(packet).map_err(|_| ExecutionError::Invalid("storage packet"))?,
        )
        .await
    }
}

#[cfg(test)]
mod initializing_build_retirement_tests {
    use super::*;

    fn owned_pool(kind: StorageKind) -> (tempfile::TempDir, PathBuf, StoragePool, StoragePacket) {
        require_root().expect("owned user/mount namespace required");
        let base = tempfile::Builder::new()
            .prefix("webdock-initializing-retirement-")
            .tempdir()
            .unwrap();
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = base.path().join("volumes");
        mkdir(&root).unwrap();
        let config = StorageConfig {
            root,
            node: "owned-node".into(),
            cluster_id: RecordId::parse("3").unwrap(),
            pool_id: "123e4567-e89b-42d3-a456-426614174000".into(),
            capacity_bytes: 4 * MIN,
            reserve_bytes: MIN,
        };
        let path = base.path().join("config.json");
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        output
            .write_all(&serde_json::to_vec(&config).unwrap())
            .unwrap();
        drop(output);
        StoragePool::initialize(&path).unwrap();
        let pool = StoragePool::open_kind(&path, kind).unwrap();
        let packet = StoragePacket::new(
            "1",
            if kind == StorageKind::Database {
                "1"
            } else {
                "2"
            },
            "3",
            1,
            MIN,
            (Utc::now() + chrono::Duration::minutes(5)).timestamp() as u64,
        )
        .unwrap();
        (base, path, pool, packet)
    }
    fn journal_initializing(pool: &mut StoragePool, packet: &StoragePacket, state: VolumeState) {
        pool.persist(VolumeRecord {
            app_id: packet.app_id.clone(),
            project_id: packet.project_id.clone(),
            cluster_id: packet.cluster_id.clone(),
            bytes: packet.spec.volume_bytes,
            uuid: uuid::Uuid::new_v4().to_string(),
            state,
        })
        .unwrap();
    }
    fn charged(pool: &StoragePool) -> u64 {
        pool.state()
            .unwrap()
            .volumes
            .values()
            .filter(|record| record.state != VolumeState::Purged)
            .map(|record| record.bytes)
            .sum()
    }
    #[test]
    #[ignore = "owned local user/mount namespace only; no loop or mount mutations"]
    fn journal_before_mkdir_cancellation_survives_restart_and_retires_only_builds() {
        let (_base, path, mut pool, packet) = owned_pool(StorageKind::Builds);
        let key = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
        let reservation = webdock_agent_protocol::build_storage::BuildStorageReservation {
            version: 1,
            audience: "webdock-build-storage-v1".into(),
            volume_id: "1".into(),
            customer_id: "10".into(),
            project_id: "2".into(),
            build_id: "9".into(),
            generation: 1,
            target: webdock_agent_protocol::build_storage::BuildStorageTarget {
                cluster_id: "3".into(),
                node: pool.config.node.clone(),
                pool_id: pool.config.pool_id.clone(),
            },
            volume_bytes: MIN,
            action: webdock_agent_protocol::build_storage::BuildStorageAction::Reserve,
            epoch: 1,
            expires_at: Utc::now() + chrono::Duration::minutes(5),
        };
        let signed = webdock_agent_protocol::build_storage::SignedBuildStorageReservation::sign(
            reservation.clone(),
            &key,
        )
        .unwrap();
        signed
            .verify(&key.verifying_key(), &reservation.target, Utc::now())
            .unwrap();
        let mut ledger = build::BuildStorageLedger::default();
        ledger.apply_verified(&reservation).unwrap();
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(pool.config.root.join("_pool/builds.json"))
            .unwrap();
        output
            .write_all(&serde_json::to_vec(&ledger).unwrap())
            .unwrap();
        output.sync_all().unwrap();
        drop(output);
        sync_directory(&pool.config.root.join("_pool")).unwrap();
        // Exactly the journal/check boundary in ensure_reserved; cancellation prevents mkdir.
        journal_initializing(&mut pool, &packet, VolumeState::Initializing);
        let cancelled = Arc::new(AtomicBool::new(true));
        assert!(
            StorageGuard::new(Utc::now() + chrono::Duration::minutes(1), cancelled)
                .check()
                .is_err()
        );
        assert!(!pool.config.root.join("1").exists());
        assert_eq!(charged(&pool), MIN);
        drop(pool);
        let mut restarted = StoragePool::open_kind(&path, StorageKind::Builds).unwrap();
        let guard = StorageGuard::maintenance();
        assert!(restarted.ensure_reserved(&packet, &guard, 0).is_err());
        let mut retirement = reservation.clone();
        retirement.action = webdock_agent_protocol::build_storage::BuildStorageAction::Retire;
        retirement.epoch = 2;
        let trust = build::BuildStorageTrust {
            version: 1,
            target: retirement.target.clone(),
            public_key: key
                .verifying_key()
                .to_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        };
        let retired = restarted
            .apply_build_storage(
                &webdock_agent_protocol::build_storage::SignedBuildStorageReservation::sign(
                    retirement, &key,
                )
                .unwrap(),
                &trust,
                &guard,
            )
            .unwrap();
        assert_eq!(retired.phase, build::BuildStoragePhase::Retired);
        assert!(
            restarted
                .apply_build_storage(&signed, &trust, &guard)
                .is_err()
        );
        restarted.purge_record(&packet,&guard).expect("authenticated retirement after exact consumer absence must recover pre-mkdir build allocation");
        assert_eq!(charged(&restarted), 0);
        assert_eq!(
            restarted.record(&packet.app_id).unwrap().unwrap().state,
            VolumeState::Purged
        );
        assert!(!restarted.config.root.join("1").exists());
        restarted.purge_record(&packet, &guard).unwrap();
        drop(restarted);
        let restarted = StoragePool::open_kind(&path, StorageKind::Builds).unwrap();
        assert_eq!(charged(&restarted), 0);
    }
    #[test]
    fn missing_initialization_inventory_rejects_own_uuid_targets_orphans_and_uncertainty() {
        let directory = Path::new("/owned-pool/1");
        let uuid = "123e4567-e89b-42d3-a456-426614174000";
        let base = serde_json::json!({"filesystems":[{"target":"/","fstype":"tmpfs","uuid":null}]});
        let empty = serde_json::json!({"loopdevices":[]});
        assert!(
            uncreated_build_loop_devices(directory, uuid, &base, &empty)
                .unwrap()
                .is_empty()
        );
        for mount in [
            serde_json::json!({"target":"/unrelated","fstype":"ext4","uuid":uuid}),
            serde_json::json!({"target":"/owned-pool/1","fstype":"tmpfs","uuid":null}),
            serde_json::json!({"target":"/owned-pool/1/mount (deleted)","fstype":"tmpfs","uuid":null}),
            serde_json::json!({"target":"/elsewhere/../owned-pool/1/mount","fstype":"tmpfs","uuid":null}),
            serde_json::json!({"target":"/unrelated","fstype":"ext4","uuid":null}),
            serde_json::json!({"target":"/unrelated","fstype":"tmpfs","uuid":null,"children":[]}),
            serde_json::json!({"target":null,"fstype":"tmpfs","uuid":null}),
        ] {
            let mut mounts = base.clone();
            mounts["filesystems"].as_array_mut().unwrap().push(mount);
            assert!(uncreated_build_loop_devices(directory, uuid, &mounts, &empty).is_err());
        }
        for loop_device in [
            serde_json::json!({"name":"/dev/loop1","back-file":"/owned-pool/1/volume.ext4"}),
            serde_json::json!({"name":"/dev/loop1","back-file":"/owned-pool/1/volume.ext4 (deleted)"}),
            serde_json::json!({"name":"/dev/loop1","back-file":null}),
            serde_json::json!({"name":"/dev/loop1","back-file":"/other/../owned-pool/1/volume.ext4"}),
            serde_json::json!({"name":"not-a-loop","back-file":"/unrelated/volume.ext4"}),
        ] {
            assert!(
                uncreated_build_loop_devices(
                    directory,
                    uuid,
                    &base,
                    &serde_json::json!({"loopdevices":[loop_device]})
                )
                .is_err()
            );
        }
        assert!(
            uncreated_build_loop_devices(
                directory,
                uuid,
                &serde_json::json!({"filesystems":[]}),
                &empty
            )
            .is_err()
        );
        assert!(
            uncreated_build_loop_devices(
                directory,
                uuid,
                &base,
                &serde_json::json!({"loopdevices":null})
            )
            .is_err()
        );
        let unrelated = serde_json::json!({"loopdevices":[{"name":"/dev/loop12","back-file":"/owned-pool/11/volume.ext4"}]});
        assert_eq!(
            uncreated_build_loop_devices(directory, uuid, &base, &unrelated).unwrap(),
            vec!["/dev/loop12"]
        );
    }
    #[test]
    #[ignore = "owned local user/mount namespace only; no loop or mount mutations"]
    fn missing_ready_purging_apps_database_and_symlink_keep_charge() {
        for (kind, state) in [
            (StorageKind::Builds, VolumeState::Ready),
            (StorageKind::Builds, VolumeState::Purging),
            (StorageKind::Apps, VolumeState::Initializing),
            (StorageKind::Database, VolumeState::Initializing),
        ] {
            let (_base, _path, mut pool, packet) = owned_pool(kind);
            journal_initializing(&mut pool, &packet, state);
            assert!(
                pool.purge_record(&packet, &StorageGuard::maintenance())
                    .is_err()
            );
            assert_eq!(charged(&pool), MIN);
            assert_eq!(pool.record(&packet.app_id).unwrap().unwrap().state, state);
        }
        let (base, _path, mut pool, packet) = owned_pool(StorageKind::Builds);
        journal_initializing(&mut pool, &packet, VolumeState::Initializing);
        let foreign = base.path().join("foreign");
        mkdir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, pool.config.root.join("1")).unwrap();
        assert!(
            pool.purge_record(&packet, &StorageGuard::maintenance())
                .is_err()
        );
        assert_eq!(charged(&pool), MIN);
        assert!(foreign.is_dir());
    }
    #[test]
    #[ignore = "owned local user/mount namespace only; no loop or mount mutations"]
    fn cancelled_or_foreign_retirement_keeps_initializing_charge_and_existing_parent_is_retained() {
        let (_base, _path, mut pool, packet) = owned_pool(StorageKind::Builds);
        journal_initializing(&mut pool, &packet, VolumeState::Initializing);
        let cancelled = StorageGuard::new(
            Utc::now() + chrono::Duration::minutes(1),
            Arc::new(AtomicBool::new(true)),
        );
        assert!(pool.purge_record(&packet, &cancelled).is_err());
        assert_eq!(charged(&pool), MIN);
        let mut foreign = packet.clone();
        foreign.project_id = RecordId::parse("99").unwrap();
        assert!(
            pool.purge_record(&foreign, &StorageGuard::maintenance())
                .is_err()
        );
        assert_eq!(charged(&pool), MIN);
        assert!(pool.purge(&packet, &StorageGuard::maintenance()).is_err());
        assert_eq!(charged(&pool), MIN);
        let directory = pool.config.root.join("1");
        mkdir(&directory).unwrap();
        pool.purge_record(&packet, &StorageGuard::maintenance())
            .unwrap();
        assert_eq!(charged(&pool), 0);
        assert!(
            directory.is_dir(),
            "normal purge must retain the allocation parent"
        );
    }
    #[test]
    #[ignore = "owned local user/mount namespace only; no loop or mount mutations"]
    fn an_active_mounted_filesystem_uuid_keeps_the_missing_build_charged() {
        let (_base, _path, mut pool, packet) = owned_pool(StorageKind::Builds);
        journal_initializing(&mut pool, &packet, VolumeState::Initializing);
        let mounts = checked(
            "/usr/bin/findmnt",
            &["--json", "--list", "-o", "TARGET,FSTYPE,UUID"],
            &StorageGuard::maintenance(),
            10,
        )
        .unwrap();
        let mounts: serde_json::Value = serde_json::from_str(&mounts).unwrap();
        let uuid = mounts["filesystems"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|mount| mount["uuid"].as_str().filter(|value| valid_uuid(value)))
            .expect("owned local fixture needs one visible filesystem UUID");
        let mut record = pool.record(&packet.app_id).unwrap().unwrap();
        record.uuid = uuid.into();
        pool.persist(record).unwrap();
        assert!(
            pool.purge_record(&packet, &StorageGuard::maintenance())
                .is_err()
        );
        assert_eq!(charged(&pool), MIN);
        assert_eq!(
            pool.record(&packet.app_id).unwrap().unwrap().state,
            VolumeState::Initializing
        );
    }
}
