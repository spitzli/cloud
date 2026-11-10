//! Customer commands may only run inside the verified VM. The host invokes QEMU only.
use crate::{
    Result,
    artifact::{Archive, MAX_OUTPUT, MAX_SOURCE},
    error, io,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub const MAX_LOG: usize = 24_000;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub cpu: u32,
    pub memory_mi_b: u32,
    pub disk_bytes: u64,
    pub duration_seconds: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GuestRequest {
    pub recipe: String,
    pub root_directory: String,
    pub build_environment: BTreeMap<String, String>,
    pub limits: Limits,
}
impl GuestRequest {
    pub fn validate(&self) -> Result<()> {
        if !["dockerfile", crate::frontend::RECIPE].contains(&self.recipe.as_str()) {
            return Err(error("recipe_unavailable"));
        }
        if self.root_directory != "." {
            crate::artifact::safe_path(&self.root_directory)?;
        }
        let l = &self.limits;
        if !(1..=8).contains(&l.cpu)
            || !(512..=16384).contains(&l.memory_mi_b)
            || !(10_000_000..=32 * 1024 * 1024 * 1024).contains(&l.disk_bytes)
            || !(30..=1800).contains(&l.duration_seconds)
        {
            return Err(error("invalid_limits"));
        }
        if self.build_environment.len() > 32
            || serde_json::to_vec(&self.build_environment)
                .map_err(|_| error("invalid_environment"))?
                .len()
                > 16000
        {
            return Err(error("invalid_environment"));
        }
        for (k, v) in &self.build_environment {
            if k.is_empty()
                || k.len() > 128
                || !k.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                })
                || ["VERCEL_", "AWS_", "SPITZLI_", "WEBDOCK_", "DOCKER_", "GIT_"]
                    .iter()
                    .any(|p| k.starts_with(p))
                || ["PATH", "HOME", "NODE_OPTIONS", "LD_PRELOAD", "PYTHONPATH"]
                    .contains(&k.as_str())
                || v.len() > 4096
                || v.contains('\0')
            {
                return Err(error("invalid_environment"));
            }
        }
        Ok(())
    }
    pub fn redact(&self, logs: &str) -> String {
        let mut text = logs.to_owned();
        let mut values: Vec<_> = self
            .build_environment
            .values()
            .filter(|v| !v.is_empty())
            .collect();
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        for v in values {
            text = text.replace(v, "[REDACTED]");
        }
        text.chars()
            .scan(0usize, |n, c| {
                *n += c.len_utf8();
                (*n <= MAX_LOG).then_some(c)
            })
            .collect()
    }
}
/// Administrator-selected immutable image; its digest is checked at every execution.
#[derive(Clone)]
pub struct VmImage {
    path: PathBuf,
    sha256: String,
    kernel: Option<(PathBuf, String)>,
    initrd: Option<(PathBuf, String)>,
    frontend_output: bool,
}
fn verified_file(path: &Path, digest: &str) -> Result<u64> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io)?;
    let m = file.metadata().map_err(io)?;
    if !m.is_file() || m.mode() & 0o022 != 0 || m.nlink() != 1 || m.len() == 0 {
        return Err(error("invalid_image"));
    }
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(io)?;
    if hex::encode(hash.finalize()) != digest {
        return Err(error("image_digest_mismatch"));
    }
    Ok(m.len())
}
impl VmImage {
    pub fn identity(&self) -> crate::evidence::MachineIdentity {
        crate::evidence::MachineIdentity {
            image_sha256: self.sha256.clone(),
            kernel_sha256: self.kernel.as_ref().map(|(_, hash)| hash.clone()),
            initrd_sha256: self.initrd.as_ref().map(|(_, hash)| hash.clone()),
        }
    }
    pub fn new(path: PathBuf, sha256: String) -> Result<Self> {
        verified_file(&path, &sha256)?;
        Ok(Self {
            path,
            sha256,
            kernel: None,
            initrd: None,
            frontend_output: false,
        })
    }
    /// Explicit operator assertion about this immutable image; old pins keep Dockerfile-only admission.
    pub fn with_frontend_output(mut self) -> Self {
        self.frontend_output = true;
        self
    }
    pub fn supported_recipes(&self) -> &'static [&'static str] {
        if self.frontend_output {
            &["dockerfile", crate::frontend::RECIPE]
        } else {
            &["dockerfile"]
        }
    }
    /// Explicit boot artifacts are independently pinned; no caller-supplied kernel arguments.
    pub fn with_boot(
        mut self,
        kernel: PathBuf,
        kernel_sha: String,
        initrd: PathBuf,
        initrd_sha: String,
    ) -> Result<Self> {
        verified_file(&kernel, &kernel_sha)?;
        verified_file(&initrd, &initrd_sha)?;
        self.kernel = Some((kernel, kernel_sha));
        self.initrd = Some((initrd, initrd_sha));
        Ok(self)
    }
    pub fn execute(
        &self,
        job: &GuestRequest,
        source: &[u8],
        scratch: &Path,
        mut lease: impl FnMut() -> Result<()>,
    ) -> Result<BuildOutput> {
        job.validate()?;
        if !self.supported_recipes().contains(&job.recipe.as_str()) {
            return Err(error("recipe_unavailable"));
        }
        lease()?;
        let image_size = verified_file(&self.path, &self.sha256)?;
        if image_size > job.limits.disk_bytes {
            return Err(error("image_exceeds_disk_budget"));
        }
        use std::os::unix::ffi::OsStrExt;
        let scratch_name = std::ffi::CString::new(scratch.as_os_str().as_bytes())
            .map_err(|_| error("invalid_scratch"))?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: valid NUL-terminated path and writable statvfs storage; read only after success.
        if unsafe { libc::statvfs(scratch_name.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        // SAFETY: statvfs returned success and initialized the entire structure.
        let stat = unsafe { stat.assume_init() };
        let block = stat.f_frsize;
        if block == 0 || stat.f_bavail.saturating_mul(block) < job.limits.disk_bytes {
            return Err(error("insufficient_disk_space"));
        }
        let file_limit =
            MAX_OUTPUT.max(image_size + (64 * 1024 * 1024u64).max(image_size.div_ceil(32)));
        if 2 * file_limit + 5 * MAX_OUTPUT + 2 * MAX_SOURCE + 4 * 50_000 * block
            > job.limits.disk_bytes
        {
            return Err(error("image_exceeds_disk_budget"));
        }
        let staging = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(scratch)
            .map_err(io)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staging.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(io)?;
        let source = Archive::read(source, MAX_SOURCE)?;
        if job.root_directory != "."
            && !source
                .files()
                .keys()
                .any(|p| p.starts_with(&format!("{}/", job.root_directory)))
        {
            return Err(error("missing_root"));
        }
        let request =
            source.guest_request(serde_json::to_vec(job).map_err(|_| error("invalid_job"))?)?;
        let input = staging.path().join("input.tar");
        let output = staging.path().join("output.tar");
        std::fs::write(&input, request.encode()?).map_err(io)?;
        let mut command = Command::new("/usr/bin/qemu-system-x86_64");
        command.args([
            "-enable-kvm",
            "-machine",
            "q35",
            "-cpu",
            "host",
            "-smp",
            &job.limits.cpu.to_string(),
            "-m",
            &job.limits.memory_mi_b.to_string(),
            "-nodefaults",
            "-no-reboot",
            "-display",
            "none",
            "-monitor",
            "none",
            "-serial",
            "none",
            "-nic",
            "none",
        ]);
        // QEMU's comma-delimited options have their own escaping independent of shell quoting.
        let qpath = |p: &Path| p.to_string_lossy().replace(',', ",,");
        command.args([
            "-drive",
            &format!(
                "file={},format=raw,if=none,id=root,readonly=off,snapshot=on",
                qpath(&self.path)
            ),
            "-device",
            "virtio-blk-pci,drive=root,bootindex=1",
            "-drive",
            &format!(
                "file={},format=raw,if=none,id=input,readonly=on",
                qpath(&input)
            ),
            "-device",
            "virtio-blk-pci,drive=input,serial=webdock-input",
            "-device",
            "virtio-serial-pci",
            "-chardev",
            &format!("file,id=result,path={}", qpath(&output)),
            "-device",
            "virtserialport,chardev=result,name=webdock.result",
        ]);
        if let Some((kernel, hash)) = &self.kernel {
            verified_file(kernel, hash)?;
            command.arg("-kernel").arg(kernel).args(["-append","root=/dev/vda rw rootfstype=ext4 modules=virtio_pci,virtio_blk,ext4 init=/init quiet"]);
        }
        if let Some((initrd, hash)) = &self.initrd {
            verified_file(initrd, hash)?;
            command.arg("-initrd").arg(initrd);
        }
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", staging.path());
        run_bounded(
            &mut command,
            Duration::from_secs(job.limits.duration_seconds),
            file_limit,
            &mut lease,
        )?;
        let output_meta = output.symlink_metadata().map_err(io)?;
        if !output_meta.is_file() || output_meta.len() > MAX_OUTPUT {
            return Err(error("artifact_limit"));
        }
        let result = decode_guest_output(job, &std::fs::read(output).map_err(io)?)?;
        lease()?;
        Ok(result)
    }
}

/// Validate the recipe-specific guest protocol before trusted evidence is constructed.
pub fn decode_guest_output(job: &GuestRequest, bytes: &[u8]) -> Result<BuildOutput> {
    job.validate()?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(error("artifact_limit"));
    }
    let result = Archive::read(bytes, MAX_OUTPUT)?;
    let frontend = job.recipe == crate::frontend::RECIPE;
    if result.files().keys().any(|name| {
        !(["status.json", "build.log", "artifact/image.tar"].contains(&name.as_str())
            || frontend && name == "artifact/frontend.tar")
    }) {
        return Err(error("invalid_result"));
    }
    let status: serde_json::Value = serde_json::from_slice(
        result
            .files()
            .get("status.json")
            .ok_or_else(|| error("invalid_result"))?,
    )
    .map_err(|_| error("invalid_result"))?;
    let logs = job.redact(&String::from_utf8_lossy(
        result
            .files()
            .get("build.log")
            .map(Vec::as_slice)
            .unwrap_or_default(),
    ));
    if status != serde_json::json!({"success":true}) {
        return Err(crate::Error {
            code: "build_failed",
            logs,
        });
    }
    let image = result
        .files()
        .get("artifact/image.tar")
        .ok_or_else(|| error("artifact_missing"))?;
    let image_digest = crate::artifact::validate_oci(image)?;
    let mut files = BTreeMap::from([("image.tar".into(), image.clone())]);
    if frontend {
        let output = result
            .files()
            .get("artifact/frontend.tar")
            .ok_or_else(|| error("frontend_output_missing"))?;
        crate::frontend::validate_output(output)?;
        files.insert("frontend.tar".into(), output.clone());
    }
    let artifact = Archive::from_files(files)?.encode()?;
    if artifact.len() as u64 > MAX_OUTPUT {
        return Err(error("artifact_limit"));
    }
    Ok(BuildOutput {
        digest: format!("sha256:{}", hex::encode(Sha256::digest(&artifact))),
        artifact,
        image_digest,
        logs,
    })
}
pub struct BuildOutput {
    pub artifact: Vec<u8>,
    pub digest: String,
    pub image_digest: String,
    pub logs: String,
}
/// Process groups, pipe bytes, file growth, elapsed time and authority are all bounded.
/// Only the guest invokes this with customer-controlled build input.
pub fn run_bounded(
    command: &mut Command,
    timeout: Duration,
    file_limit: u64,
    lease: &mut impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    run_bounded_observed(command, timeout, file_limit, lease, &mut |_| Ok(()))
}
/// The observer receives bounded raw bytes; callers must redact before persisting or displaying.
/// Observer failure uses the same complete process-group shutdown as timeout/lease failure.
pub fn run_bounded_observed(
    command: &mut Command,
    timeout: Duration,
    file_limit: u64,
    lease: &mut impl FnMut() -> Result<()>,
    observer: &mut impl FnMut(&[u8]) -> Result<()>,
) -> Result<Vec<u8>> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // SAFETY: async-signal-safe libc calls only between fork and exec; no allocator or locks.
    let parent_pid = unsafe { libc::getpid() };
    // SAFETY: the callback uses only async-signal-safe libc calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                || libc::getppid() != parent_pid
            {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let limit = libc::rlimit {
                rlim_cur: file_limit,
                rlim_max: file_limit,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(io)?;
    let pid = child.id() as i32;
    struct Kill(i32);
    impl Drop for Kill {
        fn drop(&mut self) {
            // SAFETY: kill takes a process-group identifier and no borrowed memory.
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    let guard = Kill(pid);
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);
    let handles: Vec<_> = [
        Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ]
    .into_iter()
    .map(|mut reader| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
    })
    .collect();
    drop(tx);
    let deadline = Instant::now() + timeout;
    let mut next_lease = Instant::now();
    let mut logs = Vec::new();
    let mut disconnected = false;
    let result = (|| {
        loop {
            if Instant::now() >= deadline {
                return Err(error("execution_timeout"));
            }
            if Instant::now() >= next_lease {
                lease()?;
                next_lease = Instant::now() + Duration::from_secs(5);
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(bytes) => {
                    let available = MAX_LOG.saturating_sub(logs.len());
                    logs.extend_from_slice(&bytes[..bytes.len().min(available)]);
                    if bytes.len() > available {
                        return Err(error("log_limit"));
                    }
                    observer(&bytes)?;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
                Err(_) => {}
            }
            if let Some(status) = child.try_wait().map_err(io)?
                && disconnected
            {
                if !status.success() {
                    return Err(error("execution_failed"));
                }
                lease()?;
                return Ok(());
            }
        }
    })();
    drop(guard);
    let _ = child.wait();
    drop(rx);
    for handle in handles {
        let _ = handle.join();
    }
    result.map_err(|mut e| {
        e.logs = String::from_utf8_lossy(&logs).into();
        e
    })?;
    Ok(logs)
}
