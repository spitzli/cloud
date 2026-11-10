//! File-only boundary for a disposable container. No platform, Git or registry authority.
use crate::{
    Result,
    artifact::{
        Archive, MAX_ENTRIES, MAX_OUTPUT, MAX_RETAINED_IMAGE, MAX_SOURCE, safe_path, validate_oci,
    },
    error,
    execution::{BuildOutput, Limits, MAX_LOG, run_bounded_observed},
    io,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Strategy {
    Auto,
    Dockerfile,
    Railpack,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerBuildRequest {
    /// Exact commit and archive digest admitted by the trusted RepositoryProvider.
    pub source_sha: String,
    pub archive_sha256: String,
    pub root_directory: String,
    /// Relative to root_directory; Dockerfile paths are also relative to root_directory.
    pub context_directory: String,
    pub dockerfile: Option<String>,
    pub strategy: Strategy,
    pub build_args: BTreeMap<String, String>,
    pub limits: Limits,
}
fn hex_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn commit(s: &str) -> bool {
    [40, 64].contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(s: &str, directory: bool) -> Result<()> {
    if directory && s == "." {
        Ok(())
    } else {
        safe_path(s)
    }
}
pub const PACKAGE_PROXY_URL: &str = "http://10.83.0.1:3128";
const PROXY_ENV: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];
fn proxy_control(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY" | "FTP_PROXY"
    )
}
fn environment(values: &BTreeMap<String, String>) -> Result<()> {
    if values.len() > 32
        || serde_json::to_vec(values)
            .map_err(|_| error("invalid_environment"))?
            .len()
            > 16_000
    {
        return Err(error("invalid_environment"));
    }
    for (key, value) in values {
        if proxy_control(key)
            || key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
            || [
                "SPITZLI_",
                "WEBDOCK_",
                "AWS_",
                "AZURE_",
                "GOOGLE_",
                "DOCKER_",
                "BUILDKIT_",
                "GIT_",
                "LD_",
                "DYLD_",
                "XDG_",
                "RAILPACK_",
                "MISE_",
                "VERCEL_",
            ]
            .iter()
            .any(|p| key.starts_with(p))
            || [
                "PATH",
                "HOME",
                "SHELL",
                "BASH_ENV",
                "ENV",
                "NODE_OPTIONS",
                "PYTHONPATH",
                "PYTHONHOME",
                "RUBYOPT",
                "PERL5OPT",
                "TMPDIR",
                "TMP",
                "TEMP",
                "SSL_CERT_FILE",
                "SSL_CERT_DIR",
            ]
            .contains(&key.as_str())
            || value.len() > 4096
            || value.contains('\0')
        {
            return Err(error("invalid_environment"));
        }
    }
    Ok(())
}
impl ContainerBuildRequest {
    pub fn validate(&self) -> Result<()> {
        if !commit(&self.source_sha)
            || !self
                .archive_sha256
                .strip_prefix("sha256:")
                .is_some_and(hex_digest)
        {
            return Err(error("invalid_source_identity"));
        }
        relative(&self.root_directory, true)?;
        relative(&self.context_directory, true)?;
        if let Some(path) = &self.dockerfile {
            relative(path, false)?;
        }
        let l = &self.limits;
        if !(1..=8).contains(&l.cpu)
            || !(512..=16384).contains(&l.memory_mi_b)
            || !(10_000_000..=32 * 1024 * 1024 * 1024).contains(&l.disk_bytes)
            || !(30..=1800).contains(&l.duration_seconds)
        {
            return Err(error("invalid_limits"));
        }
        environment(&self.build_args)
    }
}
/// Metadata is supplied independently by trusted admission, never read from source contents.
pub struct SourceArchive {
    source_sha: String,
    archive_sha256: String,
    bytes: Vec<u8>,
}
impl SourceArchive {
    pub fn from_bytes(source_sha: String, archive_sha256: String, bytes: Vec<u8>) -> Result<Self> {
        if !commit(&source_sha)
            || !archive_sha256
                .strip_prefix("sha256:")
                .is_some_and(hex_digest)
        {
            return Err(error("invalid_source_identity"));
        }
        if bytes.len() as u64 > MAX_SOURCE + MAX_ENTRIES as u64 * 1024 + 10240 {
            return Err(error("archive_limit"));
        }
        if archive_sha256 != format!("sha256:{}", hex::encode(Sha256::digest(&bytes))) {
            return Err(error("source_digest_mismatch"));
        }
        Ok(Self {
            source_sha,
            archive_sha256,
            bytes,
        })
    }
    pub fn from_file(path: &Path, source_sha: String, archive_sha256: String) -> Result<Self> {
        let bytes = read_input_file(path, MAX_SOURCE + MAX_ENTRIES as u64 * 1024 + 10240)?;
        Self::from_bytes(source_sha, archive_sha256, bytes)
    }
}
pub fn read_input_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(error("invalid_input_path"));
    }
    let components: Vec<_> = path.components().collect();
    let mut directory = std::fs::File::open("/").map_err(io)?;
    let mut file = None;
    for (index, component) in components.iter().enumerate().skip(1) {
        let std::path::Component::Normal(name) = component else {
            return Err(error("invalid_input_path"));
        };
        let name =
            std::ffi::CString::new(name.as_bytes()).map_err(|_| error("invalid_input_path"))?;
        let last = index == components.len() - 1;
        let flags = libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if last {
                libc::O_RDONLY | libc::O_NONBLOCK
            } else {
                libc::O_PATH | libc::O_DIRECTORY
            };
        // SAFETY: directory is open; name is NUL terminated; a successful fd is owned below.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
        // SAFETY: openat returned a fresh descriptor and ownership transfers once.
        let opened = unsafe { std::fs::File::from_raw_fd(fd) };
        if last {
            file = Some(opened)
        } else {
            directory = opened
        }
    }
    let file = file.ok_or_else(|| error("invalid_input_path"))?;
    let metadata = file.metadata().map_err(io)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(error("invalid_input_file"));
    }
    if metadata.len() > limit {
        return Err(error("artifact_limit"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() as u64 > limit {
        return Err(error("artifact_limit"));
    }
    Ok(bytes)
}
/// Secrets are separate from serializable requests and build arguments; never Debug-print them.
#[derive(Default)]
pub struct BuildSecrets {
    values: BTreeMap<String, String>,
}
impl BuildSecrets {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self> {
        environment(&values)?;
        Ok(Self { values })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PinnedTool {
    pub path: PathBuf,
    pub sha256: String,
}
impl PinnedTool {
    fn validate(&self) -> Result<()> {
        if !self.path.is_absolute() || !hex_digest(&self.sha256) {
            return Err(error("invalid_runner_tools"));
        }
        Ok(())
    }
    fn verify(&self) -> Result<()> {
        self.validate()?;
        let meta = self.path.symlink_metadata().map_err(io)?;
        if !meta.is_file() || meta.mode() & 0o022 != 0 || meta.mode() & 0o111 == 0 {
            return Err(error("untrusted_runner_tool"));
        }
        let bytes = read_input_file(&self.path, 256 * 1024 * 1024)?;
        if hex::encode(Sha256::digest(bytes)) != self.sha256 {
            return Err(error("runner_tool_digest_mismatch"));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RailpackTool {
    pub cli: PinnedTool,
    pub version: String,
    pub frontend: String,
    pub builder_image: String,
    pub runtime_image: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedTools {
    pub buildctl: PinnedTool,
    pub railpack: RailpackTool,
    pub buildkit_socket: PathBuf,
    #[serde(default, rename = "proxyURL")]
    pub proxy_url: Option<String>,
}
impl TrustedTools {
    pub fn validate(&self) -> Result<()> {
        self.buildctl.validate()?;
        if self
            .proxy_url
            .as_deref()
            .is_some_and(|proxy| proxy != PACKAGE_PROXY_URL)
        {
            return Err(error("invalid_package_proxy"));
        }
        self.railpack.cli.validate()?;
        let version = &self.railpack.version;
        if version.split('.').count() != 3
            || !version
                .split('.')
                .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(error("invalid_railpack_version"));
        }
        let prefix = format!("ghcr.io/railwayapp/railpack-frontend:v{version}@sha256:");
        if !self
            .railpack
            .frontend
            .strip_prefix(&prefix)
            .is_some_and(hex_digest)
        {
            return Err(error("railpack_frontend_mismatch"));
        }
        for (repo, image) in [
            ("railpack-builder", &self.railpack.builder_image),
            ("railpack-runtime", &self.railpack.runtime_image),
        ] {
            let prefix = format!("ghcr.io/railwayapp/{repo}:mise-2026.9.15@sha256:");
            if !image.strip_prefix(&prefix).is_some_and(hex_digest) {
                return Err(error("railpack_base_mismatch"));
            }
        }
        if !self.buildkit_socket.is_absolute()
            || self
                .buildkit_socket
                .to_str()
                .is_none_or(|s| s.contains([',', '\0', '\n']) || s.split('/').any(|p| p == ".."))
        {
            return Err(error("invalid_buildkit_socket"));
        }
        Ok(())
    }
}
pub struct PreparedBuild {
    work: tempfile::TempDir,
    context: PathBuf,
    dockerfile: PathBuf,
    strategy: Strategy,
    request_digest: [u8; 32],
}
impl PreparedBuild {
    pub fn prepare(request: &ContainerBuildRequest, source: SourceArchive) -> Result<Self> {
        request.validate()?;
        if request.source_sha != source.source_sha {
            return Err(error("source_sha_mismatch"));
        }
        if request.archive_sha256 != source.archive_sha256 {
            return Err(error("source_digest_mismatch"));
        }
        let archive = Archive::read(&source.bytes, MAX_SOURCE.min(request.limits.disk_bytes))?;
        let work = tempfile::tempdir().map_err(io)?;
        std::fs::set_permissions(work.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(io)?;
        let source_dir = work.path().join("source");
        private_dir(&source_dir)?;
        archive.extract_build_context(&source_dir)?;
        let root = source_dir.join(&request.root_directory);
        let context = root.join(&request.context_directory);
        checked_path(&source_dir, &root, true)?;
        checked_path(&source_dir, &context, true)?;
        let dockerfile = root.join(request.dockerfile.as_deref().unwrap_or("Dockerfile"));
        let exists = dockerfile.try_exists().map_err(io)?;
        // An explicit Dockerfile path is a Dockerfile override in automatic mode.
        let strategy = match request.strategy {
            Strategy::Auto if request.dockerfile.is_some() || exists => Strategy::Dockerfile,
            Strategy::Auto => Strategy::Railpack,
            other => other,
        };
        if strategy == Strategy::Dockerfile {
            checked_path(&source_dir, &dockerfile, false)?;
            validate_dockerfile(&read_input_file(&dockerfile, 1024 * 1024)?)?;
        }
        private_dir(&work.path().join("plan"))?;
        private_dir(&work.path().join("secrets"))?;
        private_dir(&work.path().join("home"))?;
        Ok(Self {
            work,
            context,
            dockerfile,
            strategy,
            request_digest: Sha256::digest(
                serde_json::to_vec(request).map_err(|_| error("invalid_request"))?,
            )
            .into(),
        })
    }
    pub fn strategy(&self) -> Strategy {
        self.strategy
    }
    /// Constructs argv without executing source. Tools and socket come only from operator config.
    pub fn commands(
        &self,
        request: &ContainerBuildRequest,
        tools: &TrustedTools,
        secrets: &BuildSecrets,
    ) -> Result<Vec<Command>> {
        request.validate()?;
        tools.validate()?;
        if self.request_digest
            != <[u8; 32]>::from(Sha256::digest(
                serde_json::to_vec(request).map_err(|_| error("invalid_request"))?,
            ))
        {
            return Err(error("build_request_changed"));
        }
        environment(&secrets.values)?;
        if secrets
            .values
            .keys()
            .any(|k| request.build_args.contains_key(k))
        {
            return Err(error("build_secret_argument_conflict"));
        }
        let mut commands = Vec::new();
        if self.strategy == Strategy::Railpack {
            let mut prepare = self.command(&tools.railpack.cli.path, tools);
            prepare
                .arg("prepare")
                .arg("--plan-out")
                .arg(self.work.path().join("plan/railpack-plan.json"))
                .arg("--info-out")
                .arg(self.work.path().join("plan/railpack-info.json"));
            for (key, value) in request.build_args.iter().chain(secrets.values.iter()) {
                prepare.arg("--env").arg(key).env(key, value);
            }
            prepare.arg(&self.context);
            commands.push(prepare);
        }
        let mut build = self.command(&tools.buildctl.path, tools);
        build
            .arg("--addr")
            .arg(format!("unix://{}", tools.buildkit_socket.display()))
            .args(["build", "--progress", "plain", "--frontend"]);
        if self.strategy == Strategy::Railpack {
            build
                .arg("gateway.v0")
                .arg("--opt")
                .arg(format!("source={}", tools.railpack.frontend))
                .args(["--opt", "filename=railpack-plan.json"]);
            build.arg("--local").arg(format!(
                "dockerfile={}",
                self.work.path().join("plan").display()
            ));
            // Railpack environments are secret mounts in its generated plan, including ordinary values.
            for (key, value) in &request.build_args {
                build
                    .arg("--secret")
                    .arg(format!("id={key},env={key}"))
                    .env(key, value);
            }
        } else {
            build.arg("dockerfile.v0").arg("--local").arg(format!(
                "dockerfile={}",
                self.dockerfile
                    .parent()
                    .ok_or_else(|| error("invalid_dockerfile"))?
                    .display()
            ));
            build.arg("--opt").arg(format!(
                "filename={}",
                self.dockerfile
                    .file_name()
                    .ok_or_else(|| error("invalid_dockerfile"))?
                    .to_string_lossy()
            ));
            for (key, value) in &request.build_args {
                build.arg("--opt").arg(format!("build-arg:{key}={value}"));
            }
        }
        for key in secrets.values.keys() {
            build.arg("--secret").arg(format!(
                "id={key},src={}",
                self.work.path().join("secrets").join(key).display()
            ));
        }
        build
            .arg("--local")
            .arg(format!("context={}", self.context.display()))
            .arg("--output")
            .arg(format!(
                "type=oci,dest={}",
                self.work.path().join("image.tar").display()
            ));
        if let Some(proxy) = &tools.proxy_url {
            for name in PROXY_ENV {
                let value = if name.eq_ignore_ascii_case("NO_PROXY") {
                    ""
                } else {
                    proxy
                };
                build.env(name, value);
                if self.strategy == Strategy::Dockerfile {
                    build.arg("--opt").arg(format!("build-arg:{name}={value}"));
                } else {
                    build.arg("--secret").arg(format!("id={name},env={name}"));
                }
            }
        }
        commands.push(build);
        Ok(commands)
    }
    fn command(&self, tool: &Path, tools: &TrustedTools) -> Command {
        let mut command = Command::new(tool);
        command
            .stdin(std::process::Stdio::null())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", self.work.path().join("home"))
            .env("TMPDIR", self.work.path())
            .env("CI", "1")
            .current_dir(&self.context);
        if let Some(proxy) = &tools.proxy_url {
            for name in PROXY_ENV {
                command.env(
                    name,
                    if name.eq_ignore_ascii_case("NO_PROXY") {
                        ""
                    } else {
                        proxy
                    },
                );
            }
        }
        command
    }
}
fn validate_dockerfile(bytes: &[u8]) -> Result<()> {
    if bytes.len() > 1024 * 1024 || bytes.contains(&0) {
        return Err(error("invalid_dockerfile"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| error("invalid_dockerfile"))?;
    for line in text.lines() {
        let line = line.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if let Some(comment) = line.strip_prefix('#')
            && let Some((key, _)) = comment.split_once('=')
        {
            let key: String = key
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '\u{feff}')
                .collect();
            if key.eq_ignore_ascii_case("syntax") || key.eq_ignore_ascii_case("check") {
                return Err(error("unsupported_dockerfile_directive"));
            }
        }
    }
    Ok(())
}
fn pin_platform_bases(plan: &mut serde_json::Value, builder: &str, runtime: &str) -> Result<()> {
    fn replace(image: &mut serde_json::Value, builder: &str, runtime: &str) -> Result<()> {
        let value = image
            .as_str()
            .ok_or_else(|| error("invalid_railpack_plan"))?;
        let replacement = match value {
            "ghcr.io/railwayapp/railpack-builder:mise-2026.9.15" => Some(builder),
            "ghcr.io/railwayapp/railpack-runtime:mise-2026.9.15" => Some(runtime),
            value if value == builder || value == runtime => None,
            value
                if value.starts_with("ghcr.io/railwayapp/railpack-builder:")
                    || value.starts_with("ghcr.io/railwayapp/railpack-runtime:") =>
            {
                return Err(error("railpack_base_mismatch"));
            }
            _ => None,
        };
        if let Some(value) = replacement {
            *image = serde_json::json!(value);
        }
        Ok(())
    }
    if let Some(steps) = plan["steps"].as_array_mut() {
        for step in steps {
            if let Some(inputs) = step["inputs"].as_array_mut() {
                for input in inputs {
                    if let Some(image) = input.get_mut("image") {
                        replace(image, builder, runtime)?;
                    }
                }
            }
        }
    }
    if let Some(image) = plan.pointer_mut("/deploy/base/image") {
        replace(image, builder, runtime)?;
    }
    if let Some(inputs) = plan["deploy"]["inputs"].as_array_mut() {
        for input in inputs {
            if let Some(image) = input.get_mut("image") {
                replace(image, builder, runtime)?;
            }
        }
    }
    Ok(())
}

fn inject_proxy_secrets(plan: &mut serde_json::Value) -> Result<()> {
    if !plan.is_object() {
        return Err(error("invalid_railpack_plan"));
    }
    for variables in plan["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| &step["variables"])
        .chain(std::iter::once(&plan["deploy"]["variables"]))
    {
        if variables
            .as_object()
            .is_some_and(|variables| variables.keys().any(|key| proxy_control(key)))
        {
            return Err(error("proxy_override_denied"));
        }
    }
    let secrets = plan
        .as_object_mut()
        .unwrap()
        .entry("secrets")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| error("invalid_railpack_plan"))?;
    for name in PROXY_ENV {
        if !secrets.iter().any(|secret| secret.as_str() == Some(name)) {
            secrets.push((*name).into());
        }
    }
    // v0.40.1 core/generate/context.go emits this exact named apt command.
    // Upgrade only its Debian repository transport; retain APT signatures and
    // TLS validation. Other customer commands and repository URLs stay intact.
    const APT_HTTPS: &str = "sh -ec 'for file in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do if [ -f \"$file\" ]; then sed -i \"s|http://deb.debian.org/|https://deb.debian.org/|g\" \"$file\"; fi; done'";
    for step in plan["steps"].as_array_mut().into_iter().flatten() {
        if let Some(commands) = step["commands"].as_array_mut()
            && let Some(index) = commands.iter().position(|command| {
                command["cmd"].as_str().is_some_and(|cmd| {
                    cmd.starts_with("sh -c 'apt-get update && apt-get install -y ")
                }) && command["customName"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("install apt packages: "))
            })
        {
            commands.insert(index,serde_json::json!({"cmd":APT_HTTPS,"customName":"use verified HTTPS Debian package sources"}));
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir(path).map_err(io)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io)
}
fn checked_path(base: &Path, path: &Path, directory: bool) -> Result<()> {
    let relative = path
        .strip_prefix(base)
        .map_err(|_| error("invalid_build_path"))?;
    let mut current = base.to_path_buf();
    for part in relative.components() {
        match part {
            std::path::Component::Normal(p) => current.push(p),
            std::path::Component::CurDir => continue,
            _ => return Err(error("invalid_build_path")),
        }
        if current
            .symlink_metadata()
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            return Err(error("invalid_build_path"));
        }
    }
    let meta = path.symlink_metadata().map_err(io)?;
    if if directory {
        !meta.is_dir()
    } else {
        !meta.is_file()
    } {
        return Err(error("invalid_build_path"));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Source,
    Strategy,
    Prepare,
    Build,
    Export,
    Complete,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunnerEvent {
    pub phase: Phase,
    pub logs: String,
}
/// This guard is accidental-host-execution protection; the executor owns runtime isolation.
pub fn require_container() -> Result<()> {
    if !(Path::new("/.dockerenv").is_file() || Path::new("/run/.containerenv").is_file()) {
        return Err(error("build_container_required"));
    }
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        return Err(error("nonroot_build_container_required"));
    }
    Ok(())
}
fn disk_usage(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut count = 0usize;
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in path.read_dir().map_err(io)? {
            let path = entry.map_err(io)?.path();
            let meta = path.symlink_metadata().map_err(io)?;
            count += 1;
            if count > MAX_ENTRIES + 128 {
                return Err(error("disk_entry_limit"));
            }
            if meta.is_dir() {
                pending.push(path)
            } else if meta.is_file() {
                total = total
                    .checked_add(meta.len())
                    .ok_or_else(|| error("disk_limit"))?
            } else {
                return Err(error("invalid_build_output"));
            }
        }
    }
    Ok(total)
}
/// Hold enough bytes to decide every possible secret match before releasing a chunk.
struct StreamingRedactor {
    values: Vec<Vec<u8>>,
    pending: Vec<u8>,
    longest: usize,
}
impl StreamingRedactor {
    fn new(mut values: Vec<Vec<u8>>) -> Self {
        values.retain(|v| !v.is_empty());
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        values.dedup();
        let longest = values.first().map_or(0, Vec::len);
        Self {
            values,
            pending: Vec::new(),
            longest,
        }
    }
    fn push(&mut self, bytes: &[u8], finish: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let end = if finish {
            self.pending.len()
        } else {
            self.pending
                .len()
                .saturating_sub(self.longest.saturating_sub(1))
        };
        let mut text = String::new();
        let mut index = 0;
        while index < end {
            let tail = &self.pending[index..];
            if let Some(value) = self.values.iter().find(|v| tail.starts_with(v)) {
                text.push_str("[REDACTED]");
                index += value.len();
                continue;
            }
            // A process can terminate while emitting a secret; do not release its trailing prefix.
            if finish && self.values.iter().any(|v| v.starts_with(tail)) {
                text.push_str("[REDACTED]");
                index = self.pending.len();
                break;
            }
            let width = match tail[0] {
                0..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 1,
            };
            if tail.len() < width {
                if !finish {
                    break;
                }
                text.push('�');
                index = self.pending.len();
                break;
            }
            match std::str::from_utf8(&tail[..width]) {
                Ok(s) => {
                    text.push_str(s);
                    index += width
                }
                Err(_) => {
                    text.push('�');
                    index += 1
                }
            }
        }
        self.pending.drain(..index);
        text
    }
}
fn emit_log(
    phase: Phase,
    text: String,
    logs: &mut String,
    emit: &mut impl FnMut(RunnerEvent) -> Result<()>,
) -> Result<()> {
    if logs.len() + text.len() > MAX_LOG {
        return Err(error("log_limit"));
    }
    logs.push_str(&text);
    let mut tail = text.as_str();
    while !tail.is_empty() {
        let mut end = tail.len().min(8192);
        while !tail.is_char_boundary(end) {
            end -= 1;
        }
        emit(RunnerEvent {
            phase,
            logs: tail[..end].into(),
        })?;
        tail = &tail[end..];
    }
    Ok(())
}
/// CPU/memory and aggregate BuildKit disk are enforced by the disposable executor's cgroup/volume.
/// Event consumers must persist output before deleting a Job; cancellation is a local callback.
pub fn run(
    request: &ContainerBuildRequest,
    source: SourceArchive,
    tools: &TrustedTools,
    secrets: BuildSecrets,
    emit: &mut impl FnMut(RunnerEvent) -> Result<()>,
    cancel: &mut impl FnMut() -> Result<()>,
) -> Result<BuildOutput> {
    require_container()?;
    tools.validate()?;
    tools.buildctl.verify()?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(request.limits.duration_seconds);
    emit(RunnerEvent {
        phase: Phase::Source,
        logs: String::new(),
    })?;
    let prepared = PreparedBuild::prepare(request, source)?;
    if prepared.strategy == Strategy::Railpack {
        tools.railpack.cli.verify()?;
    }
    emit(RunnerEvent {
        phase: Phase::Strategy,
        logs: match prepared.strategy {
            Strategy::Dockerfile => "Dockerfile selected",
            _ => "Railpack selected",
        }
        .into(),
    })?;
    for (key, value) in &secrets.values {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(prepared.work.path().join("secrets").join(key))
            .map_err(io)?;
        file.write_all(value.as_bytes()).map_err(io)?;
    }
    let mut logs = String::new();
    for (index, mut command) in prepared
        .commands(request, tools, &secrets)?
        .into_iter()
        .enumerate()
    {
        let phase = if prepared.strategy == Strategy::Railpack && index == 0 {
            Phase::Prepare
        } else {
            Phase::Build
        };
        emit(RunnerEvent {
            phase,
            logs: String::new(),
        })?;
        let stopped = std::cell::Cell::new(false);
        let mut check = || {
            if let Err(error) = cancel() {
                stopped.set(true);
                return Err(error);
            }
            if disk_usage(prepared.work.path())? > request.limits.disk_bytes {
                return Err(error("disk_limit"));
            }
            Ok(())
        };
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| error("execution_timeout"))?;
        let mut redactor = StreamingRedactor::new(
            request
                .build_args
                .values()
                .chain(secrets.values.values())
                .map(|v| v.as_bytes().to_vec())
                .collect(),
        );
        let result = run_bounded_observed(
            &mut command,
            remaining,
            MAX_OUTPUT.min(request.limits.disk_bytes),
            &mut check,
            &mut |bytes| {
                let text = redactor.push(bytes, false);
                let result = emit_log(phase, text, &mut logs, emit);
                if result.is_err() {
                    stopped.set(true);
                }
                result
            },
        );
        if let Err(mut e) = result {
            if !stopped.get()
                && let Err(observer_error) =
                    emit_log(phase, redactor.push(&[], true), &mut logs, emit)
            {
                e = observer_error;
            }
            e.logs = logs;
            return Err(e);
        }
        emit_log(phase, redactor.push(&[], true), &mut logs, emit)?;
        if matches!(phase, Phase::Prepare) {
            let info = read_input_file(
                &prepared.work.path().join("plan/railpack-info.json"),
                1024 * 1024,
            )?;
            let info: serde_json::Value =
                serde_json::from_slice(&info).map_err(|_| error("invalid_railpack_info"))?;
            if info["railpackVersion"] != tools.railpack.version {
                return Err(error("railpack_version_mismatch"));
            }
            if info["success"] != true {
                return Err(error("railpack_prepare_failed"));
            }
            let plan = read_input_file(
                &prepared.work.path().join("plan/railpack-plan.json"),
                MAX_SOURCE.min(request.limits.disk_bytes),
            )?;
            let mut plan: serde_json::Value =
                serde_json::from_slice(&plan).map_err(|_| error("invalid_railpack_plan"))?;
            if !plan.is_object() {
                return Err(error("invalid_railpack_plan"));
            }
            pin_platform_bases(
                &mut plan,
                &tools.railpack.builder_image,
                &tools.railpack.runtime_image,
            )?;
            if tools.proxy_url.is_some() {
                inject_proxy_secrets(&mut plan)?;
            }
            let bytes = serde_json::to_vec(&plan).map_err(|_| error("invalid_railpack_plan"))?;
            if bytes.len() as u64 > MAX_SOURCE.min(request.limits.disk_bytes) {
                return Err(error("railpack_plan_limit"));
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(prepared.work.path().join("plan/railpack-plan.json"))
                .map_err(io)?;
            file.write_all(&bytes).map_err(io)?;
        }
    }
    emit(RunnerEvent {
        phase: Phase::Export,
        logs: String::new(),
    })?;
    cancel()?;
    let artifact = read_input_file(
        &prepared.work.path().join("image.tar"),
        MAX_RETAINED_IMAGE.min(request.limits.disk_bytes),
    )?;
    let image_digest = validate_oci(&artifact)?;
    if Instant::now() >= deadline {
        return Err(error("execution_timeout"));
    }
    emit(RunnerEvent {
        phase: Phase::Complete,
        logs: String::new(),
    })?;
    Ok(BuildOutput {
        digest: format!("sha256:{}", hex::encode(Sha256::digest(&artifact))),
        artifact,
        image_digest,
        logs,
    })
}

#[cfg(test)]
mod live_log_tests {
    #[test]
    fn platform_base_pins_touch_only_generated_image_slots_and_reject_version_drift() {
        let builder = "ghcr.io/railwayapp/railpack-builder:mise-2026.9.15";
        let runtime = "ghcr.io/railwayapp/railpack-runtime:mise-2026.9.15";
        let bp = format!("{builder}@sha256:{}", "a".repeat(64));
        let rp = format!("{runtime}@sha256:{}", "b".repeat(64));
        let mut plan = serde_json::json!({"steps":[{"inputs":[{"image":builder},{"image":"docker.io/library/alpine:3"}],"commands":[{"cmd":format!("echo {builder}")}]}],"deploy":{"base":{"image":runtime},"inputs":[{"image":builder}]}});
        super::pin_platform_bases(&mut plan, &bp, &rp).unwrap();
        assert_eq!(plan["steps"][0]["inputs"][0]["image"], bp);
        assert_eq!(plan["deploy"]["base"]["image"], rp);
        assert_eq!(
            plan["steps"][0]["inputs"][1]["image"],
            "docker.io/library/alpine:3"
        );
        assert_eq!(
            plan["steps"][0]["commands"][0]["cmd"],
            format!("echo {builder}")
        );
        plan["deploy"]["base"]["image"] =
            serde_json::json!("ghcr.io/railwayapp/railpack-runtime:mise-unknown");
        assert!(super::pin_platform_bases(&mut plan, &bp, &rp).is_err());
    }
    use super::*;
    #[test]
    fn redacts_fragmented_secrets_and_preserves_split_utf8_without_prefix_leaks() {
        let mut redactor =
            StreamingRedactor::new(vec!["päss🔒".as_bytes().to_vec(), b"secret-value".to_vec()]);
        let raw = "start café päss🔒 secret-value done".as_bytes();
        let mut output = String::new();
        for byte in raw {
            let part = redactor.push(&[*byte], false);
            assert!(!part.contains("päss🔒"));
            assert!(!part.contains("secret-value"));
            output.push_str(&part);
        }
        output.push_str(&redactor.push(&[], true));
        assert_eq!(output, "start café [REDACTED] [REDACTED] done");
        let mut redactor = StreamingRedactor::new(vec![b"secret-value".to_vec()]);
        let mut tail = redactor.push(b"tail secret-", false);
        tail.push_str(&redactor.push(&[], true));
        assert_eq!(tail, "tail [REDACTED]");
    }
    #[test]
    fn railpack_proxy_uses_temporary_secret_environments_without_final_image_variables() {
        let mut plan = serde_json::json!({"steps":[{"name":"build","variables":{"NODE_ENV":"production"}}],"deploy":{"variables":{"APP":"value"}},"secrets":["CUSTOMER_TOKEN"]});
        inject_proxy_secrets(&mut plan).unwrap();
        assert!(
            plan["secrets"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("HTTPS_PROXY"))
        );
        assert_eq!(
            plan["steps"][0]["variables"],
            serde_json::json!({"NODE_ENV":"production"})
        );
        assert_eq!(
            plan["deploy"]["variables"],
            serde_json::json!({"APP":"value"})
        );
        let mut hostile = serde_json::json!({"steps":[{"name":"build","variables":{"http_proxy":"http://evil.example"}}],"deploy":{}});
        assert!(inject_proxy_secrets(&mut hostile).is_err());
    }
    #[test]
    fn qualified_gateway_upgrades_only_generated_debian_apt_steps_to_https() {
        let generated = serde_json::json!({"cmd":"sh -c 'apt-get update && apt-get install -y libatomic1'","customName":"install apt packages: libatomic1"});
        let customer = serde_json::json!({"cmd":"npm run build"});
        let customer_apt = serde_json::json!({"cmd":"sh -c 'apt-get update && apt-get install -y customer-package'"});
        let mut plan = serde_json::json!({"steps":[{"name":"runtime-packages","commands":[generated.clone()]},{"name":"build","commands":[customer.clone()]}],"deploy":{}});
        plan["steps"][1]["commands"]
            .as_array_mut()
            .unwrap()
            .push(customer_apt.clone());
        inject_proxy_secrets(&mut plan).unwrap();
        let commands = plan["steps"][0]["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 2);
        let preparation = commands[0]["cmd"].as_str().unwrap();
        assert!(preparation.contains("http://deb.debian.org/|https://deb.debian.org/"));
        assert!(preparation.contains("/etc/apt/sources.list.d/*.sources"));
        assert!(!preparation.contains("AllowUnauthenticated"));
        assert_eq!(commands[1], generated);
        assert_eq!(
            plan["steps"][1]["commands"],
            serde_json::json!([customer, customer_apt])
        );
    }
}
