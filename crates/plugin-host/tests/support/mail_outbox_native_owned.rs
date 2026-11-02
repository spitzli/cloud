//! Resources for THIS opted-in fixture only; never enumerate or adopt existing services.
use super::{TestResult, checked};
use axum::Router;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_rustls::{TlsAcceptor, rustls};
use webdock_plugin_host::provisioning::tls::{RuntimeIdentity, issue_runtime_identity};

pub const PG_IMAGE: &str = "docker.io/library/postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911";
// Canonical qualification of the UNPUBLISHED local wd-mail-outbox-oci RepoDigest.
pub const MAIL_IMAGE: &str = "docker.io/library/wd-mail-outbox-oci@sha256:db333f4acc7de26f8e1c784110d744656366b68dcd0d3a42eccd0fe67c98f6e8";
pub const MAIL_ELF_SHA: &str = "e68f12bfad2b631843d368c66d4d6c112dc02f36a072fd2a0c29e4d943c10c1d";
pub const GATEWAY_ELF_SHA: &str =
    "c12cc2d3ac3eb00d2317845eb9aa018a926e8073601909e9f29827329ffe2f75";
const SOCKET: &str = "unix:///var/run/docker.sock";
const OUTPUT_LIMIT: usize = 262_144;
pub fn certificate_rejected(error: &std::io::Error) -> bool {
    matches!(
        error
            .get_ref()
            .and_then(|cause| cause.downcast_ref::<rustls::Error>()),
        Some(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer | rustls::CertificateError::BadSignature
        ))
    )
}
pub async fn assert_postgres_rejects_ca(port: u16, wrong_ca: &str) -> TestResult {
    use tokio::io::AsyncWriteExt;
    checked(timeout(Duration::from_secs(5),async{
        let mut stream=checked(TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await,"wrong-CA probe TCP unavailable; not trust-denial proof")?;
        checked(stream.write_all(&[0,0,0,8,4,210,22,47]).await,"owned PG SSLRequest failed")?;
        let mut response=[0];
        checked(stream.read_exact(&mut response).await,"owned PG SSL response unavailable")?;
        if response!=*b"S"{return Err("owned PostgreSQL did not admit TLS negotiation");}
        let (_,pem)=checked(x509_parser::pem::parse_x509_pem(wrong_ca.as_bytes()),"wrong-CA probe certificate invalid")?;
        let mut roots=rustls::RootCertStore::empty();
        checked(roots.add(rustls::pki_types::CertificateDer::from(pem.contents)),"wrong-CA probe trust invalid")?;
        let config=checked(rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions(),"wrong-CA probe TLS config failed")?.with_root_certificates(roots).with_no_client_auth();
        let connector=tokio_rustls::TlsConnector::from(Arc::new(config));
        match connector.connect(rustls::pki_types::ServerName::try_from("localhost").map_err(|_|"owned PG name invalid")?,stream).await{
            Err(error) if certificate_rejected(&error)=>Ok(()),
            _=>Err("wrong-CA proof requires typed issuer/signature rejection; transport failure is not proof"),
        }
    }).await,"wrong-CA probe timed out; not trust-denial proof")?
}
pub async fn stop_slot<P>(
    slot: &mut Option<P>,
    uncertain: &mut bool,
    stop: impl for<'a> AsyncFnOnce(&'a mut P) -> TestResult,
) -> TestResult {
    if let Some(process) = slot.as_mut() {
        if let Err(error) = stop(process).await {
            *uncertain = true;
            return Err(error);
        }
        slot.take();
    }
    Ok(())
}
pub fn finish_private_directory(
    directory: &mut Option<tempfile::TempDir>,
    other_clean: bool,
    uncertain: bool,
    slots_present: bool,
) -> TestResult {
    if !other_clean || uncertain || slots_present {
        return Err("owned fixture cleanup uncertain");
    }
    if let Some(current) = directory.as_ref() {
        let path = current.path().to_path_buf();
        checked(
            std::fs::remove_dir_all(&path),
            "owned private directory removal failed",
        )?;
        if path.exists() {
            return Err("owned private directory remained");
        }
        if let Some(current) = directory.take() {
            let _ = current.keep();
        }
    }
    Ok(())
}
// One fixture owns the temporary panic hook; caught panics must not print private values first.
pub static CASE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;
pub struct PanicPrivacy(Option<PanicHook>);
impl PanicPrivacy {
    pub fn new() -> Self {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        Self(Some(previous))
    }
}
impl Drop for PanicPrivacy {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take()
            && !std::thread::panicking()
        {
            std::panic::set_hook(previous);
        }
    }
}
pub fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
pub fn stage_elf(source: &Path, target: &Path, expected: &str, bytes: u64) -> TestResult<PathBuf> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let metadata = checked(
        std::fs::symlink_metadata(source),
        "packaged ELF metadata unavailable",
    )?;
    if !metadata.is_file() || metadata.len() != bytes {
        return Err("packaged ELF must be the exact regular artifact");
    }
    let data = checked(std::fs::read(source), "packaged ELF read failed")?;
    if !data.starts_with(b"\x7fELF") || hex::encode(Sha256::digest(&data)) != expected {
        return Err("packaged ELF SHA256 mismatch; root must recheck source compatibility");
    }
    private(target, &data)?;
    checked(
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o500)),
        "owned ELF copy mode failed",
    )?;
    if hex::encode(Sha256::digest(checked(
        std::fs::read(target),
        "owned ELF copy verification failed",
    )?)) != expected
    {
        return Err("owned ELF copy SHA256 mismatch");
    }
    Ok(target.to_path_buf())
}
pub fn private(path: &Path, bytes: impl AsRef<[u8]>) -> TestResult {
    use std::io::Write;
    let mut file = checked(
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path),
        "private fixture file creation failed",
    )?;
    checked(
        file.write_all(bytes.as_ref()),
        "private fixture file write failed",
    )?;
    checked(file.sync_all(), "private fixture file sync failed")
}
pub fn replace_private(path: &Path, bytes: impl AsRef<[u8]>) -> TestResult {
    let temporary = path.with_extension(format!("{}.new", uuid::Uuid::new_v4().simple()));
    private(&temporary, bytes)?;
    checked(
        std::fs::rename(temporary, path),
        "private fixture file replacement failed",
    )
}
pub fn scrub(value: &mut String) {
    value.replace_range(.., &"\0".repeat(value.len()));
    value.clear();
}
pub struct Pki {
    pub ca: String,
    key: String,
}
impl Pki {
    pub fn write_key(&self, path: &Path) -> TestResult {
        private(path, &self.key)
    }
    pub fn new() -> TestResult<Self> {
        let key = checked(KeyPair::generate(), "fixture CA key generation failed")?;
        let mut params = checked(
            CertificateParams::new(Vec::<String>::new()),
            "fixture CA parameters failed",
        )?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = checked(params.self_signed(&key), "fixture CA signing failed")?.pem();
        Ok(Self {
            ca,
            key: key.serialize_pem(),
        })
    }
    pub fn leaf(&self) -> TestResult<RuntimeIdentity> {
        checked(
            issue_runtime_identity(&self.ca, &self.key, "localhost"),
            "fixture leaf signing failed",
        )
    }
}
impl Drop for Pki {
    fn drop(&mut self) {
        scrub(&mut self.key);
    }
}

async fn bounded_read(mut input: impl AsyncRead + Unpin) -> TestResult<Vec<u8>> {
    let mut result = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = checked(
            input.read(&mut buffer).await,
            "owned command output read failed",
        )?;
        if count == 0 {
            return Ok(result);
        }
        if result.len() + count > OUTPUT_LIMIT {
            result.fill(0);
            return Err("owned command output limit exceeded");
        }
        result.extend_from_slice(&buffer[..count]);
    }
}
pub fn docker_command() -> Command {
    let mut command = Command::new("docker");
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .args(["--host", SOCKET])
        .kill_on_drop(true);
    command
}
pub async fn docker(args: &[&str], password: Option<&str>) -> TestResult<Vec<u8>> {
    let mut command = docker_command();
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(password) = password {
        command.env("POSTGRES_PASSWORD", password);
    }
    let mut child = checked(command.spawn(), "owned Docker command could not start")?;
    let stdout = child.stdout.take().ok_or("owned Docker stdout absent")?;
    let stderr = child.stderr.take().ok_or("owned Docker stderr absent")?;
    let result = timeout(Duration::from_secs(60), async {
        let (output, mut errors, status) =
            tokio::try_join!(bounded_read(stdout), bounded_read(stderr), async {
                checked(child.wait().await, "owned Docker command wait failed")
            })?;
        errors.fill(0);
        if status.success() {
            Ok(output)
        } else {
            Err("owned Docker command failed; private diagnostics withheld")
        }
    })
    .await;
    match result {
        Ok(Ok(output)) => Ok(output),
        _ => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err("owned Docker command failed or exceeded deadline; private diagnostics withheld")
        }
    }
}
fn text(bytes: Vec<u8>) -> TestResult<String> {
    checked(String::from_utf8(bytes), "owned Docker output invalid")
}
pub struct Pg {
    pub name: String,
    pub volume: String,
    pub id: Option<String>,
    pub image_id: Option<String>,
    run: String,
    pub url: String,
    pub port: u16,
    pub pki: Pki,
    pub ca_path: PathBuf,
    directory: PathBuf,
    volume_attempted: bool,
    container_attempted: bool,
}
impl Pg {
    pub fn plan(directory: &Path, run: &str, purpose: &str) -> TestResult<Self> {
        let pki = Pki::new()?;
        let leaf = pki.leaf()?;
        let ca_path = directory.join(format!("{purpose}-pg-ca.crt"));
        private(&ca_path, &pki.ca)?;
        private(
            &directory.join(format!("{purpose}-server.crt")),
            &leaf.certificate_pem,
        )?;
        private(
            &directory.join(format!("{purpose}-server.key")),
            &leaf.private_key_pem,
        )?;
        Ok(Self {
            name: format!("webdock-task4-{purpose}-{run}"),
            volume: format!("webdock-task4-{purpose}-{run}"),
            run: run.into(),
            id: None,
            image_id: None,
            url: String::new(),
            port: 0,
            pki,
            ca_path,
            directory: directory.to_path_buf(),
            volume_attempted: false,
            container_attempted: false,
        })
    }
    pub fn journal(&self) -> Value {
        json!({"name":self.name,"volume":self.volume,"id":self.id,"image":PG_IMAGE,"imageID":self.image_id,"owner":self.run,"volumeAttempted":self.volume_attempted,"containerAttempted":self.container_attempted,"outstanding":self.outstanding()})
    }
    pub async fn start(&mut self, purpose: &str, journal: &Path) -> TestResult {
        self.volume_attempted = true;
        replace_private(
            journal,
            serde_json::to_vec(&self.journal()).map_err(|_| "ownership journal encode failed")?,
        )?;
        docker(
            &[
                "volume",
                "create",
                "--label",
                &format!("webdock.task4={}", self.run),
                &self.volume,
            ],
            None,
        )
        .await?;
        let label = text(
            docker(
                &[
                    "volume",
                    "inspect",
                    "--format",
                    "{{index .Labels \"webdock.task4\"}}",
                    &self.volume,
                ],
                None,
            )
            .await?,
        )?;
        if label.trim() != self.run {
            return Err("fresh volume ownership mismatch; refusing container creation");
        }
        let password = secret();
        private(
            &self.directory.join(format!("{purpose}-pg.password")),
            &password,
        )?;
        self.container_attempted = true;
        replace_private(
            journal,
            serde_json::to_vec(&self.journal()).map_err(|_| "ownership journal encode failed")?,
        )?;
        // Root-only bootstrap copies private TLS into the container; host directory stays0700.
        let script = "set -eu\nmkdir -p /tmp/task4-tls\ncp /tmp/task4-server.crt /tmp/task4-tls/server.crt\ncp /tmp/task4-server.key /tmp/task4-tls/server.key\nchown postgres:postgres /tmp/task4-tls/server.*\nchmod 0600 /tmp/task4-tls/server.key\nexec docker-entrypoint.sh postgres -c listen_addresses='*' -c ssl=on -c ssl_cert_file=/tmp/task4-tls/server.crt -c ssl_key_file=/tmp/task4-tls/server.key -c log_statement=none -c log_min_error_statement=panic";
        let output = docker(
            &[
                "create",
                "--pull=never",
                "--name",
                &self.name,
                "--label",
                &format!("webdock.task4={}", self.run),
                "--env",
                "POSTGRES_PASSWORD",
                "--env",
                "POSTGRES_DB=webdock_task4",
                "--publish",
                "127.0.0.1::5432",
                "--mount",
                &format!(
                    "type=volume,source={},target=/var/lib/postgresql",
                    self.volume
                ),
                "--entrypoint",
                "sh",
                PG_IMAGE,
                "-c",
                script,
            ],
            Some(&password),
        )
        .await?;
        let id = text(output)?.trim().to_owned();
        if id.len() != 64 || !id.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err("owned container ID invalid");
        }
        self.id = Some(id.clone());
        replace_private(
            journal,
            serde_json::to_vec(&self.journal()).map_err(|_| "ownership journal encode failed")?,
        )?;
        // Copy only these two exact owned files; never expose the directory containing other masters.
        docker(
            &[
                "cp",
                &self
                    .directory
                    .join(format!("{purpose}-server.crt"))
                    .to_string_lossy(),
                &format!("{id}:/tmp/task4-server.crt"),
            ],
            None,
        )
        .await?;
        docker(
            &[
                "cp",
                &self
                    .directory
                    .join(format!("{purpose}-server.key"))
                    .to_string_lossy(),
                &format!("{id}:/tmp/task4-server.key"),
            ],
            None,
        )
        .await?;
        self.verify().await?;
        self.image_id = Some(
            text(docker(&["inspect", "--format", "{{.Image}}", &id], None).await?)?
                .trim()
                .to_owned(),
        );
        replace_private(
            journal,
            serde_json::to_vec(&self.journal()).map_err(|_| "ownership journal encode failed")?,
        )?;
        docker(&["start", &id], None).await?;
        let address = text(docker(&["port", &id, "5432/tcp"], None).await?)?;
        self.port = checked(
            address
                .trim()
                .strip_prefix("127.0.0.1:")
                .ok_or("owned PostgreSQL loopback port absent")?
                .parse(),
            "owned PostgreSQL port invalid",
        )?;
        let mut url = checked(
            url::Url::parse("postgresql://postgres@localhost/webdock_task4"),
            "fixture URL invalid",
        )?;
        url.set_port(Some(self.port))
            .map_err(|_| "fixture port invalid")?;
        url.set_password(Some(&password))
            .map_err(|_| "fixture password encoding failed")?;
        self.url = url.to_string();
        private(
            &self.directory.join(format!("{purpose}-admin.url")),
            &self.url,
        )?;
        for _ in 0..150 {
            if webdock_plugin_kit::postgres::connect(&self.url, Some(self.pki.ca.as_bytes()))
                .await
                .is_ok()
            {
                return Ok(());
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("owned TLS PostgreSQL readiness deadline")
    }
    async fn inspect(&self, template: &str) -> TestResult<String> {
        let id = self.id.as_deref().unwrap_or(&self.name);
        Ok(
            text(docker(&["inspect", "--format", template, id], None).await?)?
                .trim()
                .into(),
        )
    }
    pub async fn verify(&self) -> TestResult {
        if self
            .inspect("{{index .Config.Labels \"webdock.task4\"}}")
            .await?
            != self.run
            || self.inspect("{{.Config.Image}}").await? != PG_IMAGE
        {
            return Err("owned container identity mismatch; retained for manual review");
        }
        if let Some(expected) = &self.image_id
            && self.inspect("{{.Image}}").await? != *expected
        {
            return Err("owned container image ID changed; retained for manual review");
        }
        let mounts: Value = checked(
            serde_json::from_str(&self.inspect("{{json .Mounts}}").await?),
            "owned mount observation invalid",
        )?;
        let volumes = mounts.as_array().ok_or("owned mount observation invalid")?;
        if volumes.len() != 1
            || volumes[0]["Type"] != "volume"
            || volumes[0]["Name"] != self.volume
            || volumes[0]["Destination"] != "/var/lib/postgresql"
        {
            return Err("owned container mount identity mismatch; retained for manual review");
        }
        Ok(())
    }
    pub async fn connection(&self, role: &str, password: &str) -> TestResult<DatabaseConnection> {
        let mut url = checked(url::Url::parse(&self.url), "fixture PostgreSQL URL invalid")?;
        url.set_username(role)
            .map_err(|_| "fixture role URL invalid")?;
        url.set_password(Some(password))
            .map_err(|_| "fixture role password encoding failed")?;
        url.query_pairs_mut()
            .append_pair("sslmode", "verify-full")
            .append_pair("sslrootcert", &self.ca_path.to_string_lossy());
        let mut options = ConnectOptions::new(url.to_string());
        options
            .max_connections(1)
            .min_connections(1)
            .sqlx_logging(false)
            .connect_timeout(Duration::from_secs(5));
        checked(
            Database::connect(options).await,
            "owned restricted TLS pool connection failed",
        )
    }
    pub async fn owner(&self) -> TestResult<DatabaseConnection> {
        let url = checked(url::Url::parse(&self.url), "fixture PostgreSQL URL invalid")?;
        self.connection(
            "postgres",
            url.password().ok_or("fixture owner password absent")?,
        )
        .await
    }
    pub async fn cleanup(&mut self) -> TestResult {
        if self.container_attempted {
            if self.id.is_none() {
                let id = self.inspect("{{.Id}}").await?;
                if id.len() != 64 || !id.bytes().all(|v| v.is_ascii_hexdigit()) {
                    return Err("partial owned container identity unresolved");
                }
                self.id = Some(id);
            }
            self.verify().await?;
            let id = self.id.as_deref().ok_or("owned container ID absent")?;
            docker(&["rm", "--force", "--volumes", id], None).await?;
            if !text(
                docker(
                    &[
                        "ps",
                        "--all",
                        "--quiet",
                        "--no-trunc",
                        "--filter",
                        &format!("id={id}"),
                    ],
                    None,
                )
                .await?,
            )?
            .trim()
            .is_empty()
            {
                return Err("owned container remained after removal");
            }
            self.container_attempted = false;
            self.id = None;
        }
        if self.volume_attempted {
            let label = text(
                docker(
                    &[
                        "volume",
                        "inspect",
                        "--format",
                        "{{index .Labels \"webdock.task4\"}}",
                        &self.volume,
                    ],
                    None,
                )
                .await?,
            )?;
            if label.trim() != self.run {
                return Err("owned volume identity mismatch; retained for manual review");
            }
            docker(&["volume", "rm", &self.volume], None).await?;
            if !text(
                docker(
                    &[
                        "volume",
                        "ls",
                        "--quiet",
                        "--filter",
                        &format!("name=^{}$", self.volume),
                    ],
                    None,
                )
                .await?,
            )?
            .trim()
            .is_empty()
            {
                return Err("owned volume remained after removal");
            }
            self.volume_attempted = false;
        }
        scrub(&mut self.url);
        Ok(())
    }
    pub fn outstanding(&self) -> bool {
        self.container_attempted || self.volume_attempted
    }
}

pub struct Process {
    child: Child,
    exe: PathBuf,
    logs: Option<JoinHandle<TestResult<Vec<u8>>>>,
    pub pid: u32,
    journal: PathBuf,
    journal_ok: bool,
    exit_success: Option<bool>,
    sha256: String,
    stop_verified: bool,
}
impl Process {
    pub fn start(binary: &Path, config: &Path) -> TestResult<Self> {
        let exe = checked(
            std::fs::canonicalize(binary),
            "owned executable unavailable",
        )?;
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut file = checked(std::fs::File::open(&exe), "owned executable hashing failed")?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let count = checked(file.read(&mut buffer), "owned executable hashing failed")?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        let journal = config.with_extension(format!(
            "{}.process.json",
            exe.file_name()
                .and_then(|v| v.to_str())
                .ok_or("owned executable name invalid")?
        ));
        let sha256 = hex::encode(hash.finalize());
        replace_private(
            &journal,
            serde_json::to_vec(
                &json!({"phase":"planned","binary":exe,"sha256":sha256,"config":config}),
            )
            .map_err(|_| "owned process journal encode failed")?,
        )?;
        let mut child = checked(
            Command::new(&exe)
                .arg(config)
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .kill_on_drop(true)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn(),
            "owned Mail/gateway process failed to start",
        )?;
        let pid = child.id().ok_or("owned process PID absent")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("owned child diagnostic pipe absent")?;
        let logs = tokio::spawn(bounded_read(stderr));
        let journal_ok = replace_private(
            &journal,
            serde_json::to_vec(
                &json!({"phase":"started","pid":pid,"binary":exe,"sha256":sha256,"config":config}),
            )
            .map_err(|_| "owned process journal encode failed")?,
        )
        .is_ok();
        Ok(Self {
            child,
            exe,
            logs: Some(logs),
            pid,
            journal,
            journal_ok,
            exit_success: None,
            sha256,
            stop_verified: false,
        })
    }
    pub fn running(&mut self) -> TestResult<bool> {
        match checked(self.child.try_wait(), "owned child status unavailable")? {
            Some(status) => {
                self.exit_success = Some(status.success());
                Ok(false)
            }
            None => Ok(true),
        }
    }
    pub fn exited_successfully(&self) -> bool {
        self.exit_success == Some(true)
    }
    pub fn ownership(&self) -> Value {
        json!({"pid":self.pid,"binary":self.exe,"sha256":self.sha256,"exitObserved":self.exit_success.is_some(),"journalOK":self.journal_ok,"stopVerified":self.stop_verified})
    }
    pub fn signal(&mut self, signal: rustix::process::Signal) -> TestResult {
        if !self.running()? {
            return Ok(());
        }
        let observed = checked(
            std::fs::read_link(format!("/proc/{}/exe", self.pid)),
            "owned child executable identity unavailable",
        )?;
        if observed != self.exe {
            return Err("owned process executable identity changed; refusing signal");
        }
        let pid = rustix::process::Pid::from_raw(
            i32::try_from(self.pid).map_err(|_| "owned PID invalid")?,
        )
        .ok_or("owned PID invalid")?;
        checked(
            rustix::process::kill_process(pid, signal),
            "owned process signal failed",
        )
    }
    pub async fn stop(&mut self, force: bool, secrets: &[String]) -> TestResult {
        if self.running()? {
            self.signal(if force {
                rustix::process::Signal::KILL
            } else {
                rustix::process::Signal::TERM
            })?;
            match timeout(Duration::from_secs(55), self.child.wait()).await {
                Ok(status) => {
                    self.exit_success = Some(checked(status, "owned child reap failed")?.success());
                }
                Err(_) => {
                    self.signal(rustix::process::Signal::KILL)?;
                    self.exit_success = Some(
                        checked(self.child.wait().await, "owned child reap failed")?.success(),
                    );
                }
            }
        }
        let Some(logs) = self.logs.take() else {
            return if self.stop_verified {
                Ok(())
            } else {
                Err("previous owned process finalization incomplete; handle retained")
            };
        };
        let mut logs = checked(
            checked(logs.await, "owned diagnostic task failed")?,
            "owned diagnostics exceeded limit",
        )?;
        let leaked = secrets
            .iter()
            .filter(|s| !s.is_empty())
            .any(|s| logs.windows(s.len()).any(|w| w == s.as_bytes()));
        logs.fill(0);
        if leaked {
            return Err("owned child leaked private material; diagnostics withheld");
        }
        self.journal_ok &= replace_private(
            &self.journal,
            serde_json::to_vec(
                &json!({"phase":"reaped","pid":self.pid,"binary":self.exe,"sha256":self.sha256}),
            )
            .map_err(|_| "owned process journal encode failed")?,
        )
        .is_ok();
        if !self.journal_ok {
            return Err("owned process ownership journal incomplete");
        }
        self.stop_verified = true;
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(logs) = &self.logs {
            logs.abort();
        }
        self.child.start_kill().ok();
    }
}

struct TlsListener {
    tcp: TcpListener,
    tls: TlsAcceptor,
}
impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = std::net::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            if let Ok((stream, peer)) = self.tcp.accept().await
                && let Ok(Ok(stream)) =
                    timeout(Duration::from_secs(5), self.tls.accept(stream)).await
            {
                return (stream, peer);
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}
pub struct Server {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: JoinHandle<TestResult>,
}
impl Server {
    pub fn task(stop: tokio::sync::oneshot::Sender<()>, task: JoinHandle<TestResult>) -> Self {
        Self {
            stop: Some(stop),
            task,
        }
    }
    pub fn https(
        listener: TcpListener,
        router: Router,
        identity: &RuntimeIdentity,
    ) -> TestResult<Self> {
        let (_, pem) = checked(
            x509_parser::pem::parse_x509_pem(identity.certificate_pem.as_bytes()),
            "owned HTTP certificate invalid",
        )?;
        let key = checked(
            KeyPair::from_pem(&identity.private_key_pem),
            "owned HTTP key invalid",
        )?;
        let config = checked(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions(),
            "owned HTTP TLS config failed",
        )?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(pem.contents)],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        );
        let listener = TlsListener {
            tcp: listener,
            tls: TlsAcceptor::from(Arc::new(checked(config, "owned HTTP TLS identity failed")?)),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            checked(
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await,
                "owned HTTPS listener failed",
            )
        });
        Ok(Self::task(stop, task))
    }
    pub async fn stop(&mut self) -> TestResult {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match timeout(Duration::from_secs(10), &mut self.task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(_))) => Err("owned server reported failure; cleanup uncertain"),
            Ok(Err(_)) => Err("owned server task did not finish successfully; cleanup uncertain"),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                Err("owned server drain uncertain")
            }
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}

pub fn passwords() -> BTreeMap<String, String> {
    webdock_persistence::roles::LOGIN_ROLES
        .iter()
        .map(|name| ((*name).into(), secret()))
        .collect()
}
