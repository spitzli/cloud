//! Fresh fixture setup is NOT Console installation/deployment proof. Never run without parent go.
use crate::catcher::{Catcher, Counts, Limits, Mode};
use futures_util::FutureExt;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::Notify,
    time::{sleep, timeout},
};
use webdock_control_plane::{
    http::{Services, build_app_with_domains},
    mail::{MailEndpoints, MailProviderPolicy},
    plugin_config::RuntimeConnections,
    plugins::PluginEndpoints,
};
use webdock_identity::{CookiePolicy, IdentityService};
use webdock_mail::{
    MailDependencies,
    keys::{KeyCommand, NativeKeys},
    managed_control::ManagedControl,
    sending::{Sending, SendingConfig},
    submissions::Submissions,
};
use webdock_mail_protocol::{
    native_control::{Control, ControlError, ControlServer, NativeControlClient},
    sha256,
};
use webdock_plugin_host::{FileBindings, HostService, ManagedBindings};
use webdock_plugin_runtime::{PeerIdentity, PinnedMutualTls};
#[path = "mail_outbox_native_installer.rs"]
mod installer;
#[path = "mail_outbox_native_owned.rs"]
mod owned;
pub(super) use owned::Server as SourceTestServer;
use owned::{Pg, Pki, Process, Server, private, replace_private, secret};
pub(super) use owned::{certificate_rejected, finish_private_directory, stop_slot};

pub type TestResult<T = ()> = Result<T, &'static str>;
pub async fn wait_for_fence_witness<T>(
    mut request: std::pin::Pin<&mut impl std::future::Future<Output = TestResult<T>>>,
    mut probe: impl AsyncFnMut() -> TestResult<bool>,
    limit: Duration,
) -> TestResult {
    checked(timeout(limit,async{
        loop{
            tokio::select!{
                biased;
                _=&mut request=>return Err("incoming request completed before held-fence witness"),
                _=sleep(Duration::from_millis(10))=>{},
            }
            if probe().await?{return Ok(());}
        }
    }).await,"bounded positive fence-witness deadline")?
}
pub fn fixture_core_configuration(
    auth: &str,
    console: &str,
) -> Result<webdock_core::config::PlatformConfig, webdock_core::ContractError> {
    webdock_core::config::PlatformConfig::from_map(&HashMap::from([
        ("WEBDOCK_AUTH_ORIGIN".into(), auth.into()),
        ("WEBDOCK_CONSOLE_ORIGIN".into(), console.into()),
        ("WEBDOCK_FEATURES".into(), "{\"mail\":\"enabled\"}".into()),
    ]))
}
pub fn fixture_platform_id(run: &str) -> TestResult<String> {
    if run.len() != 32
        || !run
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("invalid owned fixture run ID");
    }
    Ok(format!("task4-{}", &run[..18]))
}
pub fn cleanup_after_case(result: &Result<TestResult, Box<dyn std::any::Any + Send>>) -> bool {
    matches!(result, Ok(Ok(())))
}
pub fn checked<T, E>(result: Result<T, E>, failure: &'static str) -> TestResult<T> {
    result.map_err(|_| failure)
}
pub fn job_id(receipt: &Value) -> TestResult<String> {
    receipt["id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 19 && id.bytes().all(|v| v.is_ascii_digit()))
        .map(str::to_owned)
        .ok_or("public receipt lacks canonical job ID")
}
pub async fn sql(db: &impl ConnectionTrait, text: &str, values: Vec<sea_orm::Value>) -> TestResult {
    if values.is_empty() {
        checked(
            db.execute_unprepared(text).await,
            "owned fixture SQL failed; private details withheld",
        )?;
        return Ok(());
    }
    checked(
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            text,
            values,
        ))
        .await,
        "owned fixture SQL failed; private details withheld",
    )?;
    Ok(())
}
async fn json_row(
    db: &impl ConnectionTrait,
    text: &str,
    values: Vec<sea_orm::Value>,
) -> TestResult<Value> {
    let row = checked(
        db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            text,
            values,
        ))
        .await,
        "owned fixture observation failed",
    )?
    .ok_or("owned fixture observation absent")?;
    checked(
        row.try_get("", "value"),
        "owned fixture observation invalid",
    )
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Native,
    Action,
}
impl Source {
    pub const ALL: [Self; 2] = [Self::Native, Self::Action];
    pub fn changed_input_status(self) -> u16 {
        match self {
            Self::Native => 400,
            Self::Action => 409,
        }
    }
    pub fn sender_address(self) -> &'static str {
        match self {
            Self::Native => "sender@native.example.test",
            Self::Action => "sender@action.example.test",
        }
    }
    pub fn transition_domain(self) -> &'static str {
        self.sender_address()
            .split_once('@')
            .expect("fixture sender has domain")
            .1
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AttemptTotals {
    pub data: usize,
    pub complete: usize,
    pub stored: usize,
}
impl AttemptTotals {
    pub const NONE: Self = Self {
        data: 0,
        complete: 0,
        stored: 0,
    };
    pub const ONE_STORED: Self = Self {
        data: 1,
        complete: 1,
        stored: 1,
    };
    pub fn rejected(mode: Mode) -> TestResult<Self> {
        if !matches!(
            mode,
            Mode::RejectBeforeData | Mode::RejectAfterData | Mode::DisconnectBeforeData
        ) {
            return Err("rejection totals require an explicit rejecting mode");
        }
        Ok(Self {
            data: 1,
            complete: usize::from(mode == Mode::RejectAfterData),
            stored: 0,
        })
    }
}
pub async fn observe_attempts(
    baseline: Counts,
    expected: AttemptTotals,
    mut counts: impl FnMut() -> Counts,
    observation: impl std::future::Future<Output = ()>,
) -> TestResult {
    let totals = |count: Counts| AttemptTotals {
        data: count.data_attempts,
        complete: count.complete_data,
        stored: count.stored,
    };
    if totals(baseline) != expected || totals(counts()) != expected {
        return Err("SMTP totals changed before observation or violate exact mode expectation");
    }
    observation.await;
    if totals(counts()) != expected {
        return Err("SMTP totals changed during observation");
    }
    Ok(())
}
pub fn require_control_denial(
    result: Result<Value, ControlError>,
    expected_status: u16,
) -> TestResult {
    match result {
        Err(error) if error.status == expected_status => Ok(()),
        _ => Err(
            "expected specific NativeControl denial; infrastructure or wrong-class failure is not proof",
        ),
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    LoseBeginReply = 1,
    LoseFinishReply = 2,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Revocation {
    Cancel,
    Key,
    Actor,
    Grant,
    CurrentPeer,
    Payload,
    TransportAccount,
    PrivatePolicy,
}
impl Revocation {
    pub fn for_source(source: Source) -> Vec<Self> {
        let mut values = vec![
            Self::Cancel,
            Self::CurrentPeer,
            Self::Payload,
            Self::TransportAccount,
            Self::PrivatePolicy,
        ];
        if source == Source::Native {
            values.push(Self::Key);
        } else {
            values.extend([Self::Actor, Self::Grant]);
        }
        values
    }
}
/// Failure injection only: every positive response and ACK-loss transaction is real Core.
pub struct Faults {
    backend: Mutex<Option<Arc<ManagedControl>>>,
    hold: AtomicBool,
    held: AtomicBool,
    release: Notify,
    fault: AtomicU8,
    fired: AtomicBool,
    transitions: AtomicUsize,
    transition_denials: AtomicUsize,
    transition_allows: AtomicUsize,
    gate_epoch: AtomicUsize,
    authorization: Mutex<Option<Value>>,
}
impl Faults {
    fn new() -> Self {
        Self {
            backend: Mutex::new(None),
            hold: AtomicBool::new(false),
            held: AtomicBool::new(false),
            release: Notify::new(),
            fault: AtomicU8::new(0),
            fired: AtomicBool::new(false),
            transitions: AtomicUsize::new(0),
            transition_denials: AtomicUsize::new(0),
            transition_allows: AtomicUsize::new(0),
            gate_epoch: AtomicUsize::new(0),
            authorization: Mutex::new(None),
        }
    }
    pub fn arm(&self, fault: Fault) {
        self.fired.store(false, Ordering::SeqCst);
        self.fault.store(fault as u8, Ordering::SeqCst);
    }
    pub fn hold_claims(&self) {
        self.hold.store(true, Ordering::SeqCst);
    }
    pub fn release(&self) {
        self.hold.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
    }
}
#[async_trait::async_trait]
impl Control for Faults {
    async fn call(
        &self,
        peer: &PeerIdentity,
        generation: u64,
        command: Value,
    ) -> Result<Value, ControlError> {
        let action = command["action"].as_str().unwrap_or_default().to_owned();
        if action == "send.claim" {
            let epoch = self.gate_epoch.load(Ordering::SeqCst);
            while self.hold.load(Ordering::SeqCst) {
                let released = self.release.notified();
                self.held.store(true, Ordering::SeqCst);
                if !self.hold.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
            self.held.store(false, Ordering::SeqCst);
            if epoch != self.gate_epoch.load(Ordering::SeqCst) {
                return Err(ControlError::unavailable());
            }
        }
        let backend = self
            .backend
            .lock()
            .map_err(|_| ControlError::unavailable())?
            .clone()
            .ok_or_else(ControlError::unavailable)?;
        let result = Control::call(&*backend, peer, generation, command).await;
        if action == "send.checkTransition" {
            self.transitions.fetch_add(1, Ordering::SeqCst);
            // Blocked-transition503 is not an authorization403. Callers also require a
            // successful same-function counterfactual; an arbitrary backend error is not proof.
            if result.as_ref().is_err_and(|error| error.status == 503) {
                self.transition_denials.fetch_add(1, Ordering::SeqCst);
            }
            if result.as_ref().is_ok_and(|value| value["allowed"] == true) {
                self.transition_allows.fetch_add(1, Ordering::SeqCst);
            }
        }
        let result = result?;
        if action == "send.begin" && result["leaseToken"].is_string() {
            *self
                .authorization
                .lock()
                .map_err(|_| ControlError::unavailable())? = Some(result.clone());
        }
        let fault = if action == "send.begin" {
            1
        } else if action == "send.finish" {
            2
        } else {
            0
        };
        if fault != 0
            && self
                .fault
                .compare_exchange(fault, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            self.fired.store(true, Ordering::SeqCst);
            return Err(ControlError::unavailable());
        }
        Ok(result)
    }
}

struct Core {
    host: Arc<HostService>,
    keys: Arc<NativeKeys>,
    queue: Arc<Submissions>,
    identity: Arc<IdentityService>,
}
pub struct Fixture {
    directory: Option<tempfile::TempDir>,
    run: String,
    platform: Pg,
    metadata: Pg,
    owner: Option<DatabaseConnection>,
    pools: Vec<DatabaseConnection>,
    meta: Option<tokio_postgres::Client>,
    core: Option<Core>,
    servers: Vec<Server>,
    mail: Option<Process>,
    gateway: Option<Process>,
    binary: PathBuf,
    gateway_binary: PathBuf,
    manifest_raw: String,
    config: Value,
    pub catcher: Catcher,
    pub faults: Arc<Faults>,
    source: Source,
    http: Option<reqwest::Client>,
    core_origin: String,
    auth_origin: String,
    gateway_origin: String,
    callback_origin: String,
    runtime_origin: String,
    cookie: String,
    native_key: String,
    native_key_id: String,
    domain_id: String,
    retained_key: String,
    session_token: String,
    rls_secret: String,
    install_secret: String,
    gateway_secret: String,
    invocation_key: ed25519_dalek::SigningKey,
    issuer: ed25519_dalek::SigningKey,
    pki: Pki,
    core_identity: webdock_plugin_host::provisioning::tls::RuntimeIdentity,
    runtime_identity: webdock_plugin_host::provisioning::tls::RuntimeIdentity,
    gateway_identity: webdock_plugin_host::provisioning::tls::RuntimeIdentity,
    control: Option<NativeControlClient>,
    cleanup_complete: bool,
    process_cleanup_uncertain: bool,
}
impl Fixture {
    /// Explicit public paths only; never fall back to existing project/operator environment.
    async fn planned(source: Source, mode: Mode) -> TestResult<Self> {
        if std::env::var("WEBDOCK_MANAGED_MAIL_OUTBOX_DOCKER_TEST").as_deref() != Ok("1") {
            return Err("explicit Task4 Docker opt-in required");
        }
        let binary = PathBuf::from(
            std::env::var_os("WEBDOCK_MAIL_RUNTIME_BINARY")
                .ok_or("absolute actual Mail binary required")?,
        );
        let gateway_binary = PathBuf::from(
            std::env::var_os("WEBDOCK_MAIL_GATEWAY_BINARY")
                .ok_or("absolute actual gateway binary required")?,
        );
        if !binary.is_absolute()
            || !binary.is_file()
            || !gateway_binary.is_absolute()
            || !gateway_binary.is_file()
        {
            return Err("explicit absolute Mail/gateway binary files required");
        }
        let directory = checked(
            tempfile::Builder::new()
                .prefix("webdock-mail-outbox-task4-")
                .tempdir_in("/tmp"),
            "owned directory creation failed",
        )?;
        checked(
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)),
            "owned directory permissions failed",
        )?;
        let run = uuid::Uuid::new_v4().simple().to_string();
        private(
            &directory.path().join("ownership.json"),
            serde_json::to_vec(&json!({"run":run,"phase":"planned","fixture":"task4-source-v1","image":owned::MAIL_IMAGE,"runtimeElfSha256":owned::MAIL_ELF_SHA,"gatewayElfSha256":owned::GATEWAY_ELF_SHA}))
                .map_err(|_| "ownership encode failed")?,
        )?;
        use std::os::unix::fs::DirBuilderExt;
        let tools = directory.path().join("tools");
        checked(
            std::fs::DirBuilder::new().mode(0o700).create(&tools),
            "owned fixture tools creation failed",
        )?;
        let binary = owned::stage_elf(
            &binary,
            &tools.join("webdock-mail"),
            owned::MAIL_ELF_SHA,
            24_999_272,
        )?;
        let gateway_binary = owned::stage_elf(
            &gateway_binary,
            &tools.join("webdock-mail-gateway"),
            owned::GATEWAY_ELF_SHA,
            9_989_392,
        )?;
        let mut manifest: Value = checked(
            serde_json::from_slice(include_bytes!("../../../../plugins/mail/manifest.json")),
            "base Mail manifest invalid",
        )?;
        manifest["image"] = json!(owned::MAIL_IMAGE);
        let manifest_raw = checked(
            serde_json::to_string(&manifest),
            "actual OCI-bound manifest encode failed",
        )?;
        let platform = Pg::plan(directory.path(), &run, "platform")?;
        let metadata = Pg::plan(directory.path(), &run, "metadata")?;
        let pki = Pki::new()?;
        let core_identity = pki.leaf()?;
        let runtime_identity = pki.leaf()?;
        let gateway_identity = pki.leaf()?;
        let catcher = checked(
            Catcher::start(
                mode,
                Limits {
                    body_bytes: 4 * 1024 * 1024,
                    line_bytes: 16384,
                    messages: 16,
                    lifetime: Duration::from_secs(20),
                    ..Limits::default()
                },
            )
            .await,
            "owned SMTP catcher start failed",
        )?;
        let key_bytes = checked(hex::decode(secret()), "fixture key decode failed")?;
        let invocation_key = ed25519_dalek::SigningKey::from_bytes(&checked(
            key_bytes.try_into(),
            "fixture key length failed",
        )?);
        let key_bytes = checked(hex::decode(secret()), "fixture key decode failed")?;
        let issuer = ed25519_dalek::SigningKey::from_bytes(&checked(
            key_bytes.try_into(),
            "fixture key length failed",
        )?);
        Ok(Self {
            directory: Some(directory),
            run,
            platform,
            metadata,
            owner: None,
            pools: Vec::new(),
            meta: None,
            core: None,
            servers: Vec::new(),
            mail: None,
            gateway: None,
            binary,
            gateway_binary,
            manifest_raw,
            config: Value::Null,
            catcher,
            faults: Arc::new(Faults::new()),
            source,
            http: None,
            core_origin: String::new(),
            auth_origin: String::new(),
            gateway_origin: String::new(),
            callback_origin: String::new(),
            runtime_origin: String::new(),
            cookie: String::new(),
            native_key: String::new(),
            native_key_id: String::new(),
            domain_id: String::new(),
            retained_key: secret(),
            session_token: secret(),
            rls_secret: secret(),
            install_secret: secret(),
            gateway_secret: secret(),
            invocation_key,
            issuer,
            pki,
            core_identity,
            runtime_identity,
            gateway_identity,
            control: None,
            cleanup_complete: false,
            process_cleanup_uncertain: false,
        })
    }
    pub async fn run(
        source: Source,
        mode: Mode,
        case: impl for<'a> AsyncFnOnce(&'a mut Fixture) -> TestResult,
    ) -> TestResult {
        let _case = owned::CASE_LOCK.lock().await;
        let _privacy = owned::PanicPrivacy::new();
        let mut f = Self::planned(source, mode).await?;
        let result = std::panic::AssertUnwindSafe(async {
            f.initialize(false).await?;
            case(&mut f).await
        })
        .catch_unwind()
        .await;
        if !cleanup_after_case(&result) {
            if let Err(payload) = &result {
                self_private_panic(&f.path("panic-diagnostic.txt"), payload.as_ref())?;
            }
            let failure = match &result {
                Ok(Err(error)) => *error,
                _ => "owned acceptance panic; private payload withheld",
            };
            f.preserve_failure(failure).await?;
            return Err(failure);
        }
        let cleanup = f.cleanup().await;
        if cleanup.is_err() {
            return Err(
                "owned cleanup uncertain; private ownership journal retained; do not claim cleanup0",
            );
        }
        match result {
            Ok(result) => result,
            Err(_) => {
                Err("acceptance panicked; private panic payload withheld after owned cleanup")
            }
        }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory
            .as_ref()
            .expect("owned directory lifecycle")
            .path()
            .join(name)
    }
    fn db(&self) -> TestResult<&DatabaseConnection> {
        self.owner.as_ref().ok_or("owned platform pool absent")
    }
    fn metadata_client(&self) -> TestResult<&tokio_postgres::Client> {
        self.meta.as_ref().ok_or("owned metadata pool absent")
    }
    fn execution_receipt(
        &self,
        outcome: &str,
        resources: Value,
        failure: Option<&str>,
    ) -> TestResult {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/sdd/2026-10-24-native-mail-outbox/task-4-execution-receipts");
        if !directory.exists() {
            checked(
                std::fs::DirBuilder::new().mode(0o700).create(&directory),
                "private receipt directory creation failed",
            )?;
        }
        let metadata = checked(
            std::fs::symlink_metadata(&directory),
            "private receipt directory unavailable",
        )?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err("private receipt directory ownership mismatch");
        }
        let counts = self.catcher.counts();
        let data = json!({"run":self.run,"case":std::thread::current().name().unwrap_or("owned-case"),"source":if self.source==Source::Native{"native"}else{"action"},"outcome":outcome,"failure":failure,"resources":resources,"custodyDirectory":self.directory.as_ref().map(|d|d.path()),"processCleanupUncertain":self.process_cleanup_uncertain,"counts":{"dataAttempts":counts.data_attempts,"completeData":counts.complete_data,"stored":counts.stored,"active":counts.active,"blocked":counts.blocked},"image":owned::MAIL_IMAGE,"runtimeElfSha256":owned::MAIL_ELF_SHA,"gatewayElfSha256":owned::GATEWAY_ELF_SHA});
        replace_private(
            &directory.join(format!("{}.json", self.run)),
            checked(serde_json::to_vec(&data), "private receipt encoding failed")?,
        )
    }
    async fn preserve_failure(&mut self, failure: &'static str) -> TestResult {
        self.faults.release();
        let secrets = self.secrets();
        let _ = stop_slot(
            &mut self.gateway,
            &mut self.process_cleanup_uncertain,
            async |process| process.stop(true, &secrets).await,
        )
        .await;
        let _ = stop_slot(
            &mut self.mail,
            &mut self.process_cleanup_uncertain,
            async |process| process.stop(true, &secrets).await,
        )
        .await;
        self.control = None;
        self.http = None;
        for server in &mut self.servers {
            let _ = server.stop().await;
        }
        self.servers.clear();
        self.catcher.close().await;
        self.execution_receipt("failed_preserved",json!({"platform":self.platform.journal(),"metadata":self.metadata.journal(),"mail":self.mail.as_ref().map(Process::ownership),"gateway":self.gateway.as_ref().map(Process::ownership)}),Some(failure))?;
        // No PG removal, reset, credential rotation or private-tree deletion on failure.
        Ok(())
    }
    async fn initialize(&mut self, prepare_only: bool) -> TestResult {
        private(
            &self.path("diagnostic-custody.json"),
            checked(
                serde_json::to_vec(
                    &json!({"rlsSecret":self.rls_secret,"installSecret":self.install_secret,"retainedKey":self.retained_key,"sessionToken":self.session_token,"gatewaySecret":self.gateway_secret,"invocationKey":hex::encode(self.invocation_key.to_bytes()),"leaseIssuerKey":hex::encode(self.issuer.to_bytes())}),
                ),
                "private custody encoding failed",
            )?,
        )?;
        self.platform
            .start("platform", &self.path("platform-ownership.json"))
            .await?;
        self.metadata
            .start("metadata", &self.path("metadata-ownership.json"))
            .await?;
        self.owner = Some(self.platform.owner().await?);
        let passwords = owned::passwords();
        checked(
            webdock_persistence::roles::provision_roles(self.db()?, "webdock_task4", &passwords)
                .await,
            "fresh restricted role bootstrap failed",
        )?;
        checked(
            webdock_persistence::migrations::bootstrap(
                self.db()?,
                "webdock_task4",
                &self.rls_secret,
            )
            .await,
            "fresh native bootstrap failed; final migration must be frozen",
        )?;
        self.seed_platform(prepare_only).await?;
        let auth = self
            .platform
            .connection(
                "webdock_auth_bootstrap",
                &passwords["webdock_auth_bootstrap"],
            )
            .await?;
        let business = self
            .platform
            .connection("webdock_auth_runtime", &passwords["webdock_auth_runtime"])
            .await?;
        let worker = self
            .platform
            .connection(
                "webdock_worker_runtime",
                &passwords["webdock_worker_runtime"],
            )
            .await?;
        let resolver = self
            .platform
            .connection(
                "webdock_worker_runtime",
                &passwords["webdock_worker_runtime"],
            )
            .await?;
        // Production allocator contract: never borrow the pool holding business transactions.
        let allocation = self
            .platform
            .connection("webdock_auth_runtime", &passwords["webdock_auth_runtime"])
            .await?;
        self.pools = vec![
            auth.clone(),
            business.clone(),
            worker.clone(),
            resolver.clone(),
            allocation.clone(),
        ];
        let cookies = checked(
            CookiePolicy::new(false, secret()),
            "fixture cookie policy failed",
        )?;
        self.cookie = format!(
            "{}={}",
            cookies.name(),
            checked(
                cookies.sign(&self.session_token),
                "fixture cookie signing failed"
            )?
        );
        let identity = Arc::new(IdentityService::new(auth.clone(), cookies));
        let ids = Arc::new(webdock_persistence::IdAllocator::new(
            allocation,
            checked(
                webdock_ids::Identity::platform(0, 2),
                "fixture allocator identity failed",
            )?,
        ));
        let callback_listener = checked(
            TcpListener::bind("127.0.0.1:0").await,
            "owned callback bind failed",
        )?;
        self.callback_origin = format!(
            "https://localhost:{}",
            checked(
                callback_listener.local_addr(),
                "owned callback address absent"
            )?
            .port()
        );
        let enrolled = if prepare_only {
            json!({"runtimes":{}})
        } else {
            json!({"runtimes":{"mail":{"generation":1,"namespace":"task4-owned","service":"mail","credentialSha256":self.runtime_identity.certificate_sha256,"peers":[{"kind":"mutualTls","certificateSha256":self.runtime_identity.certificate_sha256}]}}})
        };
        private(
            &self.path("bindings.json"),
            serde_json::to_vec(&enrolled).map_err(|_| "fixture bindings encode failed")?,
        )?;
        let bindings = Arc::new(checked(
            ManagedBindings::new(
                resolver,
                self.rls_secret.clone(),
                Arc::new(FileBindings::new(self.path("bindings.json"))),
            )
            .await,
            "real managed bindings construction failed",
        )?);
        let host = checked(
            HostService::new(
                business.clone(),
                worker.clone(),
                self.rls_secret.clone(),
                ids.clone(),
                bindings,
                self.invocation_key.clone(),
            )
            .await,
            "real Host construction failed",
        )?;
        let mut host = checked(
            host.with_native_mail_control(true)
                .with_session_authority(auth.clone(), identity.clone(), None)
                .await,
            "real Host session authority failed",
        )?;
        let installer_catalog = if prepare_only {
            let catalog = self.install_catalog()?;
            host = checked(
                host.with_installation(
                    Arc::new(checked(
                        webdock_plugin_host::Catalog::from_bytes(
                            &serde_json::to_vec(&catalog)
                                .map_err(|_| "installer catalog encode failed")?,
                        ),
                        "signed installer catalog invalid",
                    )?),
                    &self.install_secret,
                ),
                "real installation service construction failed",
            )?;
            Some(catalog)
        } else {
            None
        };
        let host = Arc::new(host);
        let deps = MailDependencies {
            auth: auth.clone(),
            business,
            worker,
            identity: identity.clone(),
            browser: None,
            ids,
        };
        let sender = Arc::new(checked(
            Sending::new(
                deps.clone(),
                SendingConfig {
                    enabled: true,
                    rls_secret: self.rls_secret.clone(),
                    signing_key: self.issuer.to_bytes(),
                    worker_id: "legacy-excluded".into(),
                    worker_generation: 99,
                    worker_credential_sha256: sha256(secret().as_bytes()),
                    incoming: None,
                },
                Arc::new(MailProviderPolicy::new(host.clone())),
            )
            .await,
            "real canonical sender construction failed",
        )?);
        let queue = Arc::new(checked(
            Submissions::new(deps.clone(), &self.rls_secret, true).await,
            "real canonical queue construction failed",
        )?);
        let keys = Arc::new(checked(
            NativeKeys::new(deps, &self.rls_secret, true).await,
            "real native key service construction failed",
        )?);
        let managed = Arc::new(ManagedControl::new(
            sender.clone(),
            queue.clone(),
            host.clone(),
        ));
        *self
            .faults
            .backend
            .lock()
            .map_err(|_| "fixture fault mutex failed")? = Some(managed.clone());
        for (name, value) in [
            ("ca.crt", self.pki.ca.as_str()),
            ("core.crt", self.core_identity.certificate_pem.as_str()),
            ("core.key", self.core_identity.private_key_pem.as_str()),
            (
                "runtime.crt",
                self.runtime_identity.certificate_pem.as_str(),
            ),
            (
                "runtime.key",
                self.runtime_identity.private_key_pem.as_str(),
            ),
            (
                "gateway.crt",
                self.gateway_identity.certificate_pem.as_str(),
            ),
            (
                "gateway.key",
                self.gateway_identity.private_key_pem.as_str(),
            ),
        ] {
            private(&self.path(name), value)?;
        }
        private(&self.path("gateway.secret"), &self.gateway_secret)?;
        private(
            &self.path("invocation.hex"),
            hex::encode(self.invocation_key.to_bytes()),
        )?;
        private(&self.path("manifest.json"), self.manifest_raw.as_bytes())?;
        let listener = callback_listener;
        let mut server = checked(
            tonic::transport::Server::builder().tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tonic::transport::Identity::from_pem(
                        &self.core_identity.certificate_pem,
                        &self.core_identity.private_key_pem,
                    ))
                    .client_ca_root(tonic::transport::Certificate::from_pem(&self.pki.ca)),
            ),
            "real callback TLS configuration failed",
        )?;
        let stream = futures_util::stream::unfold(listener, |listener| async {
            let next = listener.accept().await.map(|(stream, _)| stream);
            Some((next, listener))
        });
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let faults = self.faults.clone();
        let authority = sender.clone();
        let rpc = host.clone();
        self.servers.push(Server::task(
            stop,
            tokio::spawn(async move {
                checked(
                    server
                        .add_service(rpc.rpc())
                        .add_service(authority.service())
                        .add_service(ControlServer::service(faults))
                        .serve_with_incoming_shutdown(stream, async {
                            let _ = stopped.await;
                        })
                        .await,
                    "owned callback listener failed",
                )
            }),
        ));
        // A second real listener gives test drivers actual current-peer control without worker gates.
        let listener = checked(
            TcpListener::bind("127.0.0.1:0").await,
            "owned direct control bind failed",
        )?;
        let origin = format!(
            "https://localhost:{}",
            checked(listener.local_addr(), "owned direct control address absent")?.port()
        );
        let mut server = checked(
            tonic::transport::Server::builder().tls_config(
                tonic::transport::ServerTlsConfig::new()
                    .identity(tonic::transport::Identity::from_pem(
                        &self.core_identity.certificate_pem,
                        &self.core_identity.private_key_pem,
                    ))
                    .client_ca_root(tonic::transport::Certificate::from_pem(&self.pki.ca)),
            ),
            "real direct control TLS configuration failed",
        )?;
        let stream = futures_util::stream::unfold(listener, |listener| async {
            let next = listener.accept().await.map(|(stream, _)| stream);
            Some((next, listener))
        });
        let (stop, stopped) = tokio::sync::oneshot::channel();
        self.servers.push(Server::task(
            stop,
            tokio::spawn(async move {
                checked(
                    server
                        .add_service(managed.service())
                        .serve_with_incoming_shutdown(stream, async {
                            let _ = stopped.await;
                        })
                        .await,
                    "owned direct control listener failed",
                )
            }),
        ));
        self.control = Some(checked(
            NativeControlClient::new(
                checked(
                    webdock_plugin_runtime::pinned_mtls_channel(
                        origin,
                        "localhost".into(),
                        self.runtime_tls(),
                    )
                    .await,
                    "real driver pinned control connection failed",
                )?,
                1,
            ),
            "real driver native control client failed",
        )?);
        if !prepare_only {
            self.prepare_metadata().await?;
        }
        let runtime_port = dynamic_port()?;
        self.runtime_origin = format!("https://localhost:{runtime_port}");
        private(&self.path("connections.json"),serde_json::to_vec(&json!({"runtimes":{"mail":{"generation":1,"transport":{"kind":"mutualTls","origin":self.runtime_origin,"domain":"localhost","certificate":self.path("core.crt"),"privateKey":self.path("core.key"),"authority":self.path("ca.crt"),"serverCertificateSha256":self.runtime_identity.certificate_sha256}}}})).map_err(|_|"runtime connection encode failed")?)?;
        let endpoints = Arc::new(PluginEndpoints::new(
            host.clone(),
            RuntimeConnections::new(self.path("connections.json")),
            None,
        ));
        let mail_endpoints = Arc::new(
            checked(
                MailEndpoints::sending_only(keys.clone(), &self.gateway_secret),
                "real Core Mail endpoint construction failed",
            )?
            .with_queue(queue.clone())
            .with_sender(sender),
        );
        let listener = checked(
            TcpListener::bind("127.0.0.1:0").await,
            "owned Core HTTPS bind failed",
        )?;
        self.core_origin = format!(
            "https://localhost:{}",
            checked(listener.local_addr(), "owned Core address absent")?.port()
        );
        let auth_listener = checked(
            TcpListener::bind("127.0.0.1:0").await,
            "owned Core Auth HTTPS bind failed",
        )?;
        self.auth_origin = format!(
            "https://localhost:{}",
            checked(auth_listener.local_addr(), "owned Core Auth address absent")?.port()
        );
        let platform = checked(
            fixture_core_configuration(&self.auth_origin, &self.core_origin),
            "fixture Core configuration failed",
        )?;
        let router = build_app_with_domains(
            platform,
            auth,
            Services {
                identity: Some(identity.clone()),
                plugins: Some(endpoints),
                mail: Some(mail_endpoints),
                ..Default::default()
            },
        );
        self.servers.push(Server::https(
            auth_listener,
            router.clone(),
            &self.core_identity,
        )?);
        self.servers
            .push(Server::https(listener, router, &self.core_identity)?);
        self.http = Some(checked(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .add_root_certificate(checked(
                    reqwest::Certificate::from_pem(self.pki.ca.as_bytes()),
                    "fixture HTTP trust invalid",
                )?)
                .timeout(Duration::from_secs(55))
                .build(),
            "fixture HTTP client creation failed",
        )?);
        self.core = Some(Core {
            host,
            keys,
            queue,
            identity,
        });
        if let Some(catalog) = installer_catalog {
            return self.prepare_installer_material(catalog).await;
        }
        self.create_native_key().await?;
        self.configure_mail(runtime_port)?;
        self.start_processes().await?;
        Ok(())
    }
    fn runtime_tls(&self) -> PinnedMutualTls {
        PinnedMutualTls {
            certificate: self.runtime_identity.certificate_pem.as_bytes().to_vec(),
            private_key: self.runtime_identity.private_key_pem.as_bytes().to_vec(),
            authority: self.pki.ca.as_bytes().to_vec(),
            server_certificate_sha256: self.core_identity.certificate_sha256.clone(),
        }
    }
    async fn seed_platform(&self, prepare_only: bool) -> TestResult {
        let tx = checked(
            self.db()?.begin().await,
            "owned fixture seed transaction failed",
        )?;
        sql(&tx,"SELECT set_config('webdock.organization_allocation','{\"customer_id\":\"123\",\"organization_id\":\"124\"}',true)",vec![]).await?;
        sql(
            &tx,
            "INSERT INTO webdock_admin.customers(id,name) VALUES('123','Task4 isolated customer')",
            vec![],
        )
        .await?;
        checked(
            tx.commit().await,
            "owned fixture customer allocation failed",
        )?;
        sql(self.db()?,"INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}'); INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES('123','Task4','Isolated fixture','{\"mailMessages\":100}','{}',1,'fixture')",vec![]).await?;
        sql(self.db()?,"INSERT INTO webdock_auth.\"user\"(id,name,email,\"emailVerified\",role,\"twoFactorEnabled\",\"mustChangePassword\") VALUES('task4-actor','Task4','task4@example.invalid',true,'operator',true,false); INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES('345','Task4','task4@example.invalid','operator','task4-actor')",vec![]).await?;
        sql(self.db()?,"INSERT INTO webdock_auth.session(id,token,\"userId\",\"expiresAt\",\"updatedAt\") VALUES('task4-session',$1,'task4-actor',now()+interval '1 hour',now())",vec![self.session_token.clone().into()]).await?;
        if prepare_only {
            sql(self.db()?,"INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence,verified,ownership,runtime_installation,runtime_kind) VALUES('42','Task4 prepare-only target','fixture','eu','explicit isolated fixture enrollment',true,'platform',$1,'kubernetes')",vec![fixture_platform_id(&self.run)?.into()]).await?;
            let credential = sha256(secret().as_bytes());
            sql(self.db()?,"INSERT INTO webdock_auth.hosting_agent(cluster_id,generation,credential_hash,revoked) VALUES('42',1,$1,false)",vec![credential.clone().into()]).await?;
            sql(self.db()?,"INSERT INTO webdock_auth.agent_transport_enrollment(installation,agent_id,generation,cluster_id,credential_hash,transport,unix_uid) VALUES($1,'task4-prepare-agent',1,'42',$2,'unix',$3)",vec![fixture_platform_id(&self.run)?.into(),credential.into(),i64::from(rustix::process::geteuid().as_raw()).into()]).await?;
            return Ok(());
        }
        let raw = self.manifest_raw.as_str();
        let manifest: Value =
            checked(serde_json::from_str(raw), "actual release manifest invalid")?;
        sql(self.db()?,"INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation,schema_high_water) VALUES('mail','external','enabled','enabled',$1,'ready',1,2)",vec![manifest["version"].as_str().ok_or("actual release version absent")?.into()]).await?;
        sql(self.db()?,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('mail',$1,$2,$3,$4,'https://fixture.invalid','task4-fixture')",vec![manifest["version"].as_str().ok_or("actual release version absent")?.into(),raw.into(),sha256(raw.as_bytes()).into(),manifest["image"].as_str().ok_or("actual release image absent")?.into()]).await?;
        sql(self.db()?,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) VALUES('mail',1,$1,$2,'ready',0,$3,'task4-owned','mail')",vec![manifest["version"].as_str().ok_or("actual release version absent")?.into(),manifest["image"].as_str().ok_or("actual release image absent")?.into(),self.runtime_identity.certificate_sha256.clone().into()]).await?;
        sql(self.db()?,"INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration) VALUES('mail','[\"plugin.workloads\"]','{}'); INSERT INTO webdock_mail.send_transport(customer_id,mode,revision,state,region,encrypted_credentials,provider_account_id) VALUES('123','managed',1,'ready','eu','fixture-encrypted-not-a-provider-secret',NULL); INSERT INTO webdock_mail.provider_binding(customer_id,plugin_id,revision,transport_revision) VALUES('123','mail',1,1)",vec![]).await?;
        // Native sender DNS/provider evidence is explicit reviewed fixture data, never plugin TXT evidence.
        if self.source == Source::Native {
            sql(self.db()?,"INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status,provider_id,provider_snapshot) VALUES('301','123','native.example.test','fixture-only','ownership_verified','task4-native','{\"id\":\"task4-native\",\"domain\":\"native.example.test\",\"spf_verified\":true,\"dkim_verified\":true,\"dmarc_verified\":true}')",vec![]).await?;
        } else {
            sql(self.db()?,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('234','Task4 intended project','123'); INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES('mail','123','234','[]')",vec![]).await?;
        }
        Ok(())
    }
    async fn prepare_metadata(&mut self) -> TestResult {
        use webdock_plugin_host::provisioning::storage::{MetadataProvisioner, StoragePlan};
        let provisioner = checked(
            MetadataProvisioner::connect(&self.metadata.url, Some(self.metadata.pki.ca.as_bytes()))
                .await,
            "fresh metadata provisioner failed",
        )?;
        let platform = checked(
            webdock_control_plane::plugins::installation::worker::platform_storage_identity(
                self.db()?,
            )
            .await,
            "fresh platform identity observation failed",
        )?;
        checked(
            provisioner.require_separate_cluster(&platform).await,
            "metadata is not a separate actual cluster",
        )?;
        let password = secret();
        let plan = checked(
            StoragePlan::new(
                &self.run,
                "mail",
                password,
                "localhost".into(),
                self.metadata.port,
            ),
            "fresh metadata plan failed",
        )?;
        let storage = checked(
            provisioner.provision(&plan).await,
            "fresh isolated metadata provisioning failed",
        )?;
        private(&self.path("metadata.url"), &storage.url)?;
        self.meta = Some(checked(
            webdock_plugin_kit::postgres::connect(
                &storage.url,
                Some(self.metadata.pki.ca.as_bytes()),
            )
            .await,
            "fresh private metadata TLS connection failed",
        )?);
        Ok(())
    }
    async fn create_native_key(&mut self) -> TestResult {
        if self.source != Source::Native {
            return Ok(());
        }
        let core = self.core.as_ref().ok_or("real Core absent")?;
        let principal = checked(
            core.identity.authenticate_cookie(&self.cookie).await,
            "fixture principal authentication failed",
        )?
        .ok_or("fixture principal absent")?;
        let key = checked(
            core.keys
                .manage(
                    &principal,
                    &webdock_browser_session::StoredAuthority::local_session(&principal),
                    "123",
                    KeyCommand::Create {
                        label: "Task4 isolated key".into(),
                        domain_id: None,
                    },
                )
                .await,
            "actual native key creation failed",
        )?;
        let value = checked(
            serde_json::to_value(key),
            "private native key result invalid",
        )?;
        self.native_key = value["token"]
            .as_str()
            .ok_or("private native key absent")?
            .into();
        self.native_key_id = value["id"]
            .as_str()
            .ok_or("private native key ID absent")?
            .into();
        Ok(())
    }
    fn configure_mail(&mut self, port: u16) -> TestResult {
        let storage = checked(
            std::fs::read_to_string(self.path("metadata.url")),
            "owned metadata credential read failed",
        )?;
        self.config = json!({"manifest":self.path("manifest.json"),"issuerPublicKeyHex":hex::encode(self.invocation_key.verifying_key().as_bytes()),"generation":1,
            "listenTls":{"address":format!("127.0.0.1:{port}"),"certificate":self.path("runtime.crt"),"privateKey":self.path("runtime.key"),"authority":self.path("ca.crt")},
            "coreTls":{"origin":self.callback_origin,"domain":"localhost","certificate":self.path("runtime.crt"),"privateKey":self.path("runtime.key"),"authority":self.path("ca.crt"),"serverCertificateSha256":self.core_identity.certificate_sha256},
            "corePeers":[{"kind":"mutualTls","certificateSha256":self.core_identity.certificate_sha256}],"storageUrl":storage,"storageCa":self.metadata.ca_path,"secretKeyHex":self.retained_key,
            "platformTransport":{"kind":"smtp","host":"127.0.0.1","port":self.catcher.address().port()},"trustUnverifiedDomains":true,"mailboxes":null,
            "delivery":{"nativeLeaseIssuerPublicKeyHex":hex::encode(self.issuer.verifying_key().as_bytes()),"gatewayPeers":[{"kind":"mutualTls","certificateSha256":self.gateway_identity.certificate_sha256}],
                "providerBinding":{"pluginID":"mail","generation":1,"descriptorSha256":sha256(self.manifest_raw.as_bytes()),"workerID":"mail","workerGeneration":1,"workerCredentialSha256":self.runtime_identity.certificate_sha256},
                "nativeAccounts":[{"customerID":"123","accountID":null,"mode":"managed","transportRevision":1,"bindingRevision":1,"byokRevision":null,"transport":{"kind":"smtp","host":"127.0.0.1","port":self.catcher.address().port()}}]}});
        replace_private(
            &self.path("runtime.json"),
            serde_json::to_vec(&self.config).map_err(|_| "private Mail config encode failed")?,
        )
    }
    async fn start_processes(&mut self) -> TestResult {
        self.mail = Some(Process::start(&self.binary, &self.path("runtime.json"))?);
        let connections = RuntimeConnections::new(self.path("connections.json"));
        let mut runtime_ready = false;
        for _ in 0..150 {
            if let Ok(connection) = connections.load("mail")
                && let Ok(mut client) = connection.connect().await
                && client
                    .health(webdock_plugin_runtime::wire::HealthRequest {})
                    .await
                    .is_ok()
            {
                runtime_ready = true;
                break;
            }
            if !self.mail.as_mut().ok_or("owned Mail absent")?.running()? {
                return Err("actual Mail exited during readiness; private diagnostics withheld");
            }
            sleep(Duration::from_millis(100)).await;
        }
        if !runtime_ready {
            return Err("actual Mail runtime readiness deadline");
        }
        let port = dynamic_port()?;
        self.gateway_origin = format!("http://127.0.0.1:{port}");
        let config = json!({"listen":format!("127.0.0.1:{port}"),"runtime":{"origin":self.runtime_origin,"domain":"localhost","certificate":self.path("gateway.crt"),"privateKey":self.path("gateway.key"),"authority":self.path("ca.crt"),"serverCertificateSha256":self.runtime_identity.certificate_sha256},"preauthentication":{"origin":self.auth_origin,"secretFile":self.path("gateway.secret"),"authority":self.path("ca.crt")}});
        replace_private(
            &self.path("gateway.json"),
            serde_json::to_vec(&config).map_err(|_| "private gateway config encode failed")?,
        )?;
        self.gateway = Some(Process::start(
            &self.gateway_binary,
            &self.path("gateway.json"),
        )?);
        let mut ready = false;
        for _ in 0..100 {
            if self
                .http()?
                .get(format!("{}/healthz", self.gateway_origin))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                ready = true;
                break;
            }
            if !self
                .gateway
                .as_mut()
                .ok_or("owned gateway absent")?
                .running()?
            {
                return Err("actual gateway exited during readiness; private diagnostics withheld");
            }
            sleep(Duration::from_millis(100)).await;
        }
        if !ready {
            return Err("actual gateway readiness deadline");
        }
        if self.source == Source::Action && self.domain().await?.is_none() {
            let created = self
                .action(
                    "domains.create",
                    "create-owned-domain",
                    &json!({"domain":"action.example.test"}),
                )
                .await?;
            if created.0 != 200 {
                return Err("real Core domain creation failed");
            }
            let domain = created.1["resource"]["id"]
                .as_str()
                .ok_or("real domain resource missing")?
                .to_owned();
            let verified = self
                .action(
                    "domains.verify",
                    "verify-owned-domain",
                    &json!({"resource_id":domain}),
                )
                .await?;
            if verified.0 != 200 {
                return Err("real Core plugin TXT verification failed");
            }
            self.domain_id = domain;
        }
        Ok(())
    }
    fn http(&self) -> TestResult<&reqwest::Client> {
        self.http.as_ref().ok_or("owned HTTP client absent")
    }
    pub fn input(&self, subject: &str) -> Value {
        if self.source == Source::Native {
            json!({"from":self.source.sender_address(),"to":"target@example.test","subject":subject,"text":"Task4 local-only body"})
        } else {
            json!({"resource_id":self.domain_sync(),"from":"sender","from_name":"Task4","to":"target@example.test","subject":subject,"text":"Task4 local-only body"})
        }
    }
    fn domain_sync(&self) -> String {
        self.domain_id.clone()
    }
    async fn domain(&mut self) -> TestResult<Option<String>> {
        if self.source != Source::Action {
            return Ok(None);
        }
        let row=json_row(self.db()?,"SELECT coalesce((SELECT to_jsonb(resource_id) FROM webdock_auth.plugin_resource WHERE plugin_id='mail' AND project_id='234' AND offering_id='domain' LIMIT 1),'null'::jsonb) AS value",vec![]).await?;
        let id = row.as_str().map(str::to_owned);
        if let Some(id) = &id {
            self.domain_id = id.clone();
        }
        Ok(id)
    }
    async fn action(&self, action: &str, key: &str, input: &Value) -> TestResult<(u16, Value)> {
        let response=checked(self.http()?.post(format!("{}/api/console/plugins/invoke",self.core_origin)).header("cookie",&self.cookie).header("origin",&self.core_origin).header("content-type","application/json").body(serde_json::to_vec(&json!({"pluginID":"mail","projectID":"234","action":action,"input":input,"idempotencyKey":key})).map_err(|_|"public action encoding failed")?).send().await,"actual Console endpoint request failed; private error withheld")?;
        let status = response.status().as_u16();
        Ok((status, read_reply(response).await?))
    }
    async fn native(
        &self,
        method: reqwest::Method,
        path: &str,
        key: Option<&str>,
        input: Option<&Value>,
    ) -> TestResult<(u16, Value)> {
        let mut request = self
            .http()?
            .request(method, format!("{}{path}", self.gateway_origin))
            .bearer_auth(&self.native_key);
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        if let Some(input) = input {
            request = request
                .header("content-type", "application/json")
                .body(serde_json::to_vec(input).map_err(|_| "public native encoding failed")?);
        }
        let response = checked(
            request.send().await,
            "actual native gateway request failed; private error withheld",
        )?;
        let status = response.status().as_u16();
        Ok((status, read_reply(response).await?))
    }
    async fn public(&self, key: &str, input: &Value) -> TestResult<(u16, Value)> {
        if self.faults.hold.load(Ordering::SeqCst) {
            wait(
                async || self.faults.held.load(Ordering::SeqCst),
                "actual worker claim gate deadline",
            )
            .await?;
        }
        if self.source == Source::Native {
            self.native(reqwest::Method::POST, "/v1/emails", Some(key), Some(input))
                .await
        } else {
            self.action("mail.send", key, input).await
        }
    }
    pub async fn submit(&self, key: &str, input: &Value) -> TestResult<Value> {
        let (status, value) = self.public(key, input).await?;
        if status
            != if self.source == Source::Native {
                202
            } else {
                200
            }
        {
            return Err("actual public send did not accept queued/replayed receipt");
        }
        job_id(&value)?;
        Ok(value)
    }
    pub async fn submit_status(&self, key: &str, input: &Value) -> TestResult<u16> {
        Ok(self.public(key, input).await?.0)
    }
    pub async fn discard_public_reply(&self, key: &str, input: &Value) -> TestResult {
        // Drop after real response headers, BEFORE polling/decoding the job receipt body.
        // This is a client-lost receipt, not a fabricated backend reply or claimed wire reset.
        let request = if self.source == Source::Native {
            self.http()?
                .post(format!("{}/v1/emails", self.gateway_origin))
                .bearer_auth(&self.native_key)
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .body(serde_json::to_vec(input).map_err(|_| "public send encoding failed")?)
        } else {
            self.http()?.post(format!("{}/api/console/plugins/invoke",self.core_origin)).header("cookie",&self.cookie).header("origin",&self.core_origin).header("content-type","application/json").body(serde_json::to_vec(&json!({"pluginID":"mail","projectID":"234","action":"mail.send","input":input,"idempotencyKey":key})).map_err(|_|"public action encoding failed")?)
        };
        let response = checked(
            request.send().await,
            "actual public send failed before receipt was discarded",
        )?;
        let status = response.status().as_u16();
        drop(response);
        if status == 200 || status == 202 {
            Ok(())
        } else {
            Err("public send failed before driver discarded receipt")
        }
    }
    pub async fn only_job(&self) -> TestResult<String> {
        let value = json_row(
            self.db()?,
            "SELECT jsonb_agg(id ORDER BY id) AS value FROM webdock_mail.send_job",
            vec![],
        )
        .await?;
        if value.as_array().is_none_or(|v| v.len() != 1) {
            return Err("expected one real canonical send job");
        }
        value[0]
            .as_str()
            .map(str::to_owned)
            .ok_or("real canonical send ID absent")
    }
    async fn row(&self, id: &str) -> TestResult<Value> {
        json_row(
            self.db()?,
            "SELECT to_jsonb(j) AS value FROM webdock_mail.send_job j WHERE id=$1",
            vec![id.into()],
        )
        .await
    }
    pub async fn wait_state(&self, id: &str, state: &str) -> TestResult {
        wait(
            async || {
                self.row(id)
                    .await
                    .is_ok_and(|value| value["state"] == state)
            },
            "actual canonical outcome deadline",
        )
        .await
    }
    async fn control(&self, command: Value) -> TestResult<Value> {
        checked(
            self.control_raw(command).await,
            "actual current-peer NativeControl denied request",
        )
    }
    async fn control_raw(&self, command: Value) -> Result<Value, ControlError> {
        self.control
            .as_ref()
            .ok_or_else(ControlError::unavailable)?
            .call(command)
            .await
    }
    pub async fn wait_fault(&self) -> TestResult {
        wait(
            async || self.faults.fired.load(Ordering::SeqCst),
            "postcommit failure injection deadline",
        )
        .await
    }
    pub async fn wait_catcher_blocked(&self) -> TestResult {
        wait(
            async || self.catcher.counts().blocked == 1,
            "full stored DATA gate deadline",
        )
        .await
    }
    pub async fn assert_one_use_attempt_replay_denied(&self) -> TestResult {
        let mut auth = self
            .faults
            .authorization
            .lock()
            .map_err(|_| "private authorization observation failed")?
            .clone()
            .ok_or("actual handoff authorization absent")?;
        let token = auth["leaseToken"]
            .as_str()
            .ok_or("actual handoff lease token absent")?
            .to_owned();
        let channel = checked(
            webdock_plugin_runtime::pinned_mtls_channel(
                self.callback_origin.clone(),
                "localhost".into(),
                self.runtime_tls(),
            )
            .await,
            "real replay authority channel failed",
        )?;
        let mut authority =
            webdock_mail_protocol::wire::mail_authority_client::MailAuthorityClient::new(channel);
        let replay = authority
            .begin_attempt(webdock_mail_protocol::wire::LeaseProof { lease_token: token })
            .await;
        scrub_json(&mut auth);
        match replay {
            Err(status) if status.code() == tonic::Code::PermissionDenied => Ok(()),
            _ => Err(
                "one-use replay requires actual permission denial; transport failure is not proof",
            ),
        }
    }
    pub async fn assert_capture(&self, id: &str, index: usize) -> TestResult {
        let lease=json_row(self.db()?,"SELECT claims_json::jsonb AS value FROM webdock_mail.provider_lease WHERE job_id=$1 AND attempted_at IS NOT NULL",vec![id.into()]).await?;
        let from = self.source.sender_address();
        let digest = lease["mimeSha256"]
            .as_str()
            .ok_or("actual Core MIME commitment absent")?;
        let bytes = usize::try_from(
            lease["mimeBytes"]
                .as_u64()
                .ok_or("actual Core MIME length absent")?,
        )
        .map_err(|_| "actual Core MIME length invalid")?;
        if !self.catcher.matches_smtp_framed_commitment(
            index,
            from,
            &["target@example.test"],
            digest,
            bytes,
        ) {
            return Err("stored DATA differs from signed prepared MIME plus exact SMTP framing");
        }
        Ok(())
    }
    pub async fn assert_one_message(&self, baseline: Counts) -> TestResult {
        self.assert_no_new_attempts(baseline, AttemptTotals::ONE_STORED)
            .await
    }
    pub async fn assert_definitive_rejection(&self, id: &str, mode: Mode) -> TestResult {
        let counts = self.catcher.counts();
        if counts.stored != 0
            || counts.data_attempts != 1
            || counts.complete_data != usize::from(mode == Mode::RejectAfterData)
        {
            return Err("definitively rejected DATA was counted as accepted/stored");
        }
        let reservation=json_row(self.db()?,"SELECT to_jsonb(r) AS value FROM webdock_mail.budget_reservation r WHERE reference_id=$1",vec![id.into()]).await?;
        if reservation["state"] != "released" {
            return Err("definitive rejection retained recipient allowance");
        }
        let reference = self.row(id).await?["payload_ref"]
            .as_str()
            .ok_or("terminal private reference absent")?
            .to_owned();
        let client = self.metadata_client()?;
        wait(async ||client.query_one("SELECT NOT EXISTS(SELECT FROM private_payloads WHERE reference=$1::text::uuid)",&[&reference]).await.is_ok_and(|row|row.get::<_,bool>(0)),"definitive rejection private payload cleanup deadline").await
    }
    pub async fn assert_no_new_attempts(
        &self,
        baseline: Counts,
        expected: AttemptTotals,
    ) -> TestResult {
        observe_attempts(
            baseline,
            expected,
            || self.catcher.counts(),
            sleep(Duration::from_secs(2)),
        )
        .await
    }
    pub async fn assert_source_scope(&self, id: &str) -> TestResult {
        let row = self.row(id).await?;
        if self.source == Source::Native {
            if !row["source_id"]
                .as_str()
                .is_some_and(|s| s.starts_with("key:"))
                || row["source_context"].get("projectID").is_some()
            {
                return Err("native key acquired fabricated project provenance");
            }
            let status = self
                .native(
                    reqwest::Method::GET,
                    &format!("/v1/emails/{id}"),
                    None,
                    None,
                )
                .await?;
            if status.0 != 200 || status.1["id"] != id {
                return Err("native originating key status failed");
            }
        } else {
            if !row["source_id"]
                .as_str()
                .is_some_and(|s| s.starts_with("action:"))
                || row["source_context"]["projectID"] != "234"
            {
                return Err("action source provenance absent");
            }
            let status = self
                .action(
                    "mail.status",
                    "status-scope",
                    &json!({"resource_id":self.domain_sync(),"id":id}),
                )
                .await?;
            if status.0 != 200 || status.1["id"] != id {
                return Err("action owned-domain status failed");
            }
        }
        Ok(())
    }
    pub async fn retained_custody(&self, id: &str) -> TestResult<Value> {
        let row = self.row(id).await?;
        let reference = row["payload_ref"]
            .as_str()
            .ok_or("canonical private reference absent")?;
        let payload=checked(self.metadata_client()?.query_opt("SELECT sealed,sha256,payload_bytes FROM private_payloads WHERE reference=$1::text::uuid",&[&reference]).await,"private custody observation failed")?.ok_or("unknown payload was removed")?;
        let reservation=json_row(self.db()?,"SELECT to_jsonb(r) AS value FROM webdock_mail.budget_reservation r WHERE reference_id=$1",vec![id.into()]).await?;
        if reservation["state"] != "reserved" {
            return Err("unknown recipient allowance was released");
        }
        Ok(
            json!({"sealed":payload.get::<_,String>(0),"digest":payload.get::<_,String>(1),"bytes":payload.get::<_,i32>(2),"reservation":reservation,"key":self.retained_key}),
        )
    }
    pub async fn assert_retained_custody(&self, id: &str, expected: &Value) -> TestResult {
        if self.retained_custody(id).await? != *expected {
            return Err("unknown private payload, key or quota changed across restart");
        }
        Ok(())
    }
    pub async fn expire_queue_lease(&self, id: &str) -> TestResult {
        // Explicit fixture clock-boundary mutation, never admission/authority replacement.
        sql(self.db()?,"UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1 AND state IN ('leased','handoff','unknown')",vec![id.into()]).await
    }
    fn secrets(&self) -> Vec<String> {
        vec![
            self.cookie.clone(),
            self.native_key.clone(),
            self.retained_key.clone(),
            self.session_token.clone(),
            self.rls_secret.clone(),
            self.install_secret.clone(),
            self.gateway_secret.clone(),
            self.runtime_identity.private_key_pem.clone(),
            self.gateway_identity.private_key_pem.clone(),
            self.core_identity.private_key_pem.clone(),
            "Task4 local-only body".into(),
        ]
    }
    pub async fn restart(&mut self, force: bool) -> TestResult {
        if self.process_cleanup_uncertain {
            return Err("restart blocked by earlier process cleanup uncertainty");
        }
        let secrets = self.secrets();
        stop_slot(
            &mut self.gateway,
            &mut self.process_cleanup_uncertain,
            async |process| process.stop(false, &secrets).await,
        )
        .await?;
        stop_slot(
            &mut self.mail,
            &mut self.process_cleanup_uncertain,
            async |process| process.stop(force, &secrets).await,
        )
        .await?;
        // Preserve exact listener certificate, private key, retained sealing key and metadata URL.
        self.start_processes().await
    }
    pub async fn cancel(&self, id: &str) -> TestResult<Value> {
        let response = if self.source == Source::Native {
            self.native(
                reqwest::Method::DELETE,
                &format!("/v1/emails/{id}"),
                None,
                None,
            )
            .await?
        } else {
            self.action(
                "mail.cancel",
                "cancel-pending",
                &json!({"resource_id":self.domain_sync(),"id":id}),
            )
            .await?
        };
        if response.0 != 200 {
            return Err("actual pending cancellation failed");
        }
        Ok(response.1)
    }
    pub async fn set_installation(&self, state: &str) -> TestResult {
        if !["enabled", "draining", "disabled"].contains(&state) {
            return Err("invalid fixture installation transition");
        }
        sql(
            self.db()?,
            "UPDATE webdock_auth.plugin_installation SET state=$1,desired_state=$1 WHERE id='mail'",
            vec![state.into()],
        )
        .await
    }
    pub async fn assert_draining_new_admission_denied(&self) -> TestResult {
        // Native HTTP uses admission-mode send.admit, not the separate send.preflight command.
        // Console discovery excludes draining releases and reports404 before invocation.
        let expected = if self.source == Source::Native {
            403
        } else {
            404
        };
        if self
            .submit_status("new-while-draining", &self.input("new intent"))
            .await?
            != expected
        {
            return Err("draining public ingress returned unexpected error class");
        }
        if self.source == Source::Native {
            let request = checked(
                webdock_mail::message::SendRequest::parse(
                    self.input("new intent"),
                    chrono::Utc::now(),
                ),
                "valid draining admission request could not compile",
            )?;
            let payload = checked(
                serde_json::to_vec(&request),
                "valid draining admission payload encoding failed",
            )?;
            let submission = json!({"token":self.native_key,"idempotencyKey":"new-while-draining-control","requestHash":sha256(&payload),"payloadRef":uuid::Uuid::new_v4().to_string(),"payloadBytes":payload.len(),"envelope":{"from":request.from.email,"to":request.to},"notBefore":null});
            require_control_denial(
                self.control_raw(json!({"action":"send.admit","submission":submission}))
                    .await,
                403,
            )?;
        }
        Ok(())
    }
    pub fn signal_mail_term(&mut self) -> TestResult {
        self.mail
            .as_mut()
            .ok_or("owned Mail process absent")?
            .signal(rustix::process::Signal::TERM)
    }
    pub async fn wait_mail_stopped(&mut self) -> TestResult {
        let process = self.mail.as_mut().ok_or("owned Mail process absent")?;
        wait(
            async || process.running().is_ok_and(|running| !running),
            "actual Mail SIGTERM drain deadline",
        )
        .await?;
        if !process.exited_successfully() {
            return Err("actual Mail SIGTERM did not exit successfully");
        }
        Ok(())
    }
    pub async fn assert_zero_ce(&self) -> TestResult {
        let count=checked(self.metadata_client()?.query_one("SELECT (SELECT count(*) FROM customer_engines)+(SELECT count(*) FROM engines)+(SELECT count(*) FROM mailboxes) AS n",&[]).await,"private zero-CE observation failed")?;
        if count.get::<_, i64>(0) != 0 {
            return Err("outgoing-only allocated incoming private state");
        }
        let value=json_row(self.db()?,"SELECT jsonb_build_object('services',(SELECT count(*) FROM webdock_mail.service),'workloads',(SELECT count(*) FROM webdock_auth.plugin_workload)) AS value",vec![]).await?;
        if value["services"] != 0 || value["workloads"] != 0 {
            return Err("outgoing-only created incoming or workload obligations");
        }
        Ok(())
    }
    async fn ensure_transition_domain(&mut self) -> TestResult {
        if !self.domain_id.is_empty() {
            return Ok(());
        }
        // A genuine incoming-action project, created only for the barrier test. Native sends
        // remain projectless and never use this project's relay/policy or resource evidence.
        sql(self.db()?,"INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES('234','Task4 incoming barrier project','123'); INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES('mail','123','234','[]')",vec![]).await?;
        let created = self
            .action(
                "domains.create",
                "incoming-barrier-domain",
                &json!({"domain":self.source.transition_domain()}),
            )
            .await?;
        if created.0 != 200 {
            return Err("actual incoming barrier domain creation failed");
        }
        self.domain_id = created.1["resource"]["id"]
            .as_str()
            .ok_or("actual incoming barrier resource absent")?
            .into();
        Ok(())
    }
    pub async fn assert_transition_denied(&mut self, id: &str) -> TestResult {
        self.ensure_transition_domain().await?;
        self.assert_transition_backend_healthy(id).await?;
        let row = self.row(id).await?;
        if ![
            "queued",
            "leased",
            "preparation_review",
            "handoff",
            "unknown",
        ]
        .contains(&row["state"].as_str().unwrap_or_default())
        {
            return Err("transition check requires actual outstanding send");
        }
        for (action, input) in [
            ("incoming.activate", json!({"resource_id":self.domain_id})),
            ("domains.delete", json!({"resource_id":self.domain_id})),
            (
                "mailboxes.create",
                json!({"domain_id":self.domain_id,"local_part":"task4-mailbox"}),
            ),
        ] {
            let before = self.faults.transition_denials.load(Ordering::SeqCst);
            let response = self
                .action(action, &format!("{action}-blocked-{id}"), &input)
                .await?;
            if response.0 != 503
                || self.faults.transition_denials.load(Ordering::SeqCst) != before + 1
            {
                return Err("actual Core transition barrier did not deny outstanding send");
            }
        }
        Ok(())
    }
    async fn assert_transition_backend_healthy(&self, id: &str) -> TestResult {
        // An unrelated owned domain is the counterfactual, never the asserted native barrier.
        let created = self
            .action(
                "domains.create",
                &format!("barrier-control-create-{id}"),
                &json!({"domain":"unrelated-barrier-control.example.test"}),
            )
            .await?;
        if created.0 != 200 {
            return Err("transition counterfactual resource creation failed");
        }
        let resource = created.1["resource"]["id"]
            .as_str()
            .ok_or("transition counterfactual resource absent")?;
        let before = self.faults.transition_allows.load(Ordering::SeqCst);
        let deleted = self
            .action(
                "domains.delete",
                &format!("barrier-control-delete-{id}"),
                &json!({"resource_id":resource}),
            )
            .await?;
        if deleted.0 != 200 || self.faults.transition_allows.load(Ordering::SeqCst) != before + 1 {
            return Err("transition backend lacks successful actual Core counterfactual");
        }
        Ok(())
    }
    pub async fn mutate(&mut self, mutation: Revocation, id: &str) -> TestResult {
        match mutation{
            Revocation::Cancel=>return Err("use actual public cancel API"),
            Revocation::Key=>{
                let core=self.core.as_ref().ok_or("real Core absent")?;
                let principal=checked(core.identity.authenticate_cookie(&self.cookie).await,"fixture key revocation authentication failed")?.ok_or("fixture principal absent")?;
                checked(core.keys.manage(&principal,&webdock_browser_session::StoredAuthority::local_session(&principal),"123",KeyCommand::Revoke{key_id:self.native_key_id.clone()}).await,"actual native key revocation failed")?;
            }
            Revocation::Actor=>sql(self.db()?,"UPDATE webdock_auth.\"user\" SET banned=true WHERE id='task4-actor'",vec![]).await?,
            Revocation::Grant=>sql(self.db()?,"UPDATE webdock_auth.plugin_grant SET revoked_at=now() WHERE plugin_id='mail' AND project_id='234'",vec![]).await?,
            Revocation::CurrentPeer=>{
                let mut value:Value=checked(serde_json::from_slice(&checked(std::fs::read(self.path("bindings.json")),"owned bindings read failed")?),"owned binding decode failed")?;
                value["runtimes"]["mail"]["peers"][0]["certificateSha256"]=json!("f".repeat(64));
                replace_private(&self.path("bindings.json"),serde_json::to_vec(&value).map_err(|_|"owned binding encode failed")?)?;
            }
            Revocation::Payload=>{
                let reference=self.row(id).await?["payload_ref"].as_str().ok_or("private payload ref absent")?.to_owned();
                checked(self.metadata_client()?.execute("UPDATE private_payloads SET sealed='fixture-corrupt-ciphertext' WHERE reference=$1::text::uuid",&[&reference]).await,"private payload corruption fixture failed")?;
            }
            Revocation::TransportAccount=>sql(self.db()?,"UPDATE webdock_mail.send_transport SET provider_account_id='changed-fixture-account' WHERE customer_id='123'",vec![]).await?,
            Revocation::PrivatePolicy=>{
                self.faults.gate_epoch.fetch_add(1,Ordering::SeqCst);
                let secrets=self.secrets();
                if self.process_cleanup_uncertain{return Err("policy restart blocked by earlier process cleanup uncertainty");}
                stop_slot(&mut self.mail,&mut self.process_cleanup_uncertain,async |process|process.stop(true,&secrets).await).await?;
                if self.source==Source::Native{self.config["delivery"]["nativeAccounts"]=json!([]);}
                else{self.config["platformTransport"]["host"]=json!("localhost");}
                replace_private(&self.path("runtime.json"),serde_json::to_vec(&self.config).map_err(|_|"private policy rotation encode failed")?)?;
                self.mail=Some(Process::start(&self.binary,&self.path("runtime.json"))?);
            }
        }
        Ok(())
    }
    pub async fn assert_denied_before_handoff(&self, mutation: Revocation, id: &str) -> TestResult {
        let state = match mutation {
            Revocation::Cancel | Revocation::Key | Revocation::Actor | Revocation::Grant => {
                "cancelled"
            }
            Revocation::CurrentPeer => {
                sleep(Duration::from_secs(2)).await;
                require_control_denial(
                    self.control_raw(json!({"action":"send.claim"})).await,
                    403,
                )?;
                "queued"
            }
            _ => "preparation_review",
        };
        self.wait_state(id, state).await?;
        if ["unknown", "preparation_review"].contains(&state) {
            self.retained_custody(id).await?;
        }
        Ok(())
    }
    pub async fn claim_fencing(&self, id: &str) -> TestResult {
        require_control_denial(
            self.control_raw(json!({"action":"fixture.invalid-command"}))
                .await,
            400,
        )?;
        let core = self.core.as_ref().ok_or("real Core absent")?;
        let legacy = checked(
            core.queue
                .execute(webdock_mail::submissions::QueueCommand::Claim {})
                .await,
            "actual legacy claim failed",
        )?;
        if !legacy.is_null() {
            return Err("legacy worker claimed managed private payload lane");
        }
        let original = self.control(json!({"action":"send.claim"})).await?;
        if original["id"] != id {
            return Err("real enrolled driver did not claim expected job");
        }
        if !self
            .control(json!({"action":"send.claim"}))
            .await?
            .is_null()
        {
            return Err("competing claim duplicated unexpired lease");
        }
        self.expire_queue_lease(id).await?;
        let replacement = self.control(json!({"action":"send.claim"})).await?;
        if replacement["id"] != id || original["leaseToken"] == replacement["leaseToken"] {
            return Err("expired preparation lease was not replaced");
        }
        let stale_review = self
            .control_raw(json!({"action":"send.reviewPreparation","lease":original}))
            .await;
        // Tuple rejection has the retained503 wire contract. It is NEVER sufficient alone:
        // require matching-token success through the same lane validation, plus unchanged row.
        require_control_denial(stale_review, 503)?;
        let stale_finish = self
            .control_raw(
                json!({"action":"send.finish","lease":original,"result":{"state":"rejected"}}),
            )
            .await;
        require_control_denial(stale_finish, 503)?;
        match self
            .control_raw(
                json!({"action":"send.finish","lease":replacement,"result":{"state":"rejected"}}),
            )
            .await
        {
            Ok(Value::Bool(false)) => {}
            _ => {
                return Err(
                    "stale tuple proof requires successful matching-token lane-validation counterfactual",
                );
            }
        }
        let (digest, preparation) = self.actual_preparation(&original).await?;
        let stale_begin=self.control_raw(json!({"action":"send.begin","lease":original,"digest":digest,"preparation":preparation})).await;
        if !matches!(stale_begin, Ok(Value::Null)) {
            return Err(
                "stale begin must successfully return no authorization; an error is not proof",
            );
        }
        let unchanged = self.row(id).await?;
        if unchanged["state"] != "leased" || unchanged["lease_token"] != replacement["leaseToken"] {
            return Err("stale requests changed replacement custody");
        }
        self.expire_queue_lease(id).await?;
        Ok(())
    }
    async fn private_request(&self, lease: &Value) -> TestResult<Value> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use sha2::{Digest, Sha256};
        let reference = lease["payloadRef"]
            .as_str()
            .ok_or("actual claim private ref absent")?;
        let customer = lease["customerID"]
            .as_str()
            .ok_or("actual claim customer absent")?;
        let row=checked(self.metadata_client()?.query_one("SELECT sealed FROM private_payloads WHERE reference=$1::text::uuid AND customer_id=$2",&[&reference,&customer]).await,"actual claim payload custody observation failed")?;
        let mut hash = Sha256::new();
        hash.update(b"webdock-mail:private-payload-key:v1\0");
        hash.update(checked(
            hex::decode(&self.retained_key),
            "fixture retained payload key invalid",
        )?);
        let key = STANDARD.encode(hash.finalize());
        let cipher = checked(
            webdock_mail::secrets::MailCipher::new(&key),
            "actual Mail cipher construction failed",
        )?;
        let mut encoded: String = checked(
            cipher.open(
                row.get::<_, &str>(0),
                &format!("payload:{customer}:{reference}"),
            ),
            "actual private payload decryption failed",
        )?;
        let mut payload = checked(
            STANDARD.decode(&encoded),
            "actual private payload encoding invalid",
        )?;
        owned::scrub(&mut encoded);
        let value: Value = checked(
            serde_json::from_slice(&payload),
            "actual canonical stored payload invalid",
        )?;
        payload.fill(0);
        Ok(value)
    }
    pub async fn assert_original_payload(&self, id: &str, input: &Value) -> TestResult {
        let row = self.row(id).await?;
        let mut value = self
            .private_request(
                &json!({"customerID":row["customer_id"],"payloadRef":row["payload_ref"]}),
            )
            .await?;
        let equal = value["text"] == input["text"]
            && value["from"]["name"] == input["from_name"]
            && value["subject"] == input["subject"];
        scrub_json(&mut value);
        if !equal {
            return Err(
                "actual canonical private custody truncated original Unicode input/display name",
            );
        }
        Ok(())
    }
    async fn actual_preparation(&self, lease: &Value) -> TestResult<(String, Value)> {
        use webdock_mail::message::{SendRequest, prepare_action_message, prepare_message};
        let value = self.private_request(lease).await?;
        let request = if self.source == Source::Action {
            checked(
                SendRequest::parse_stored_action(value),
                "actual action stored request invalid",
            )?
        } else {
            checked(
                SendRequest::parse_stored(value, chrono::Utc::now()),
                "actual native stored request invalid",
            )?
        };
        let message = if self.source == Source::Action {
            checked(
                request.compile_action(),
                "actual action MIME compile failed",
            )?
        } else {
            checked(request.compile(), "actual native MIME compile failed")?
        };
        let recipients: Vec<String> = checked(
            serde_json::from_value(lease["envelope"]["to"].clone()),
            "actual claim envelope invalid",
        )?;
        let prepared = if self.source == Source::Action {
            checked(
                prepare_action_message(&message.raw, &message.sender, &recipients),
                "actual action MIME preparation failed",
            )?
        } else {
            checked(
                prepare_message(&message.raw, &message.sender, &recipients),
                "actual native MIME preparation failed",
            )?
        };
        let digest = checked(
            webdock_mail::proof::message_digest(&prepared.raw),
            "actual MIME digest failed",
        )?;
        let commitment = checked(
            webdock_mail::provider_transport::preparation(&message, &prepared),
            "actual MIME preparation commitment failed",
        )?;
        Ok((
            digest,
            checked(
                serde_json::to_value(commitment),
                "actual commitment encoding failed",
            )?,
        ))
    }
    pub async fn assert_disabled_and_old_generation(&self, id: &str) -> TestResult {
        require_control_denial(self.control_raw(json!({"action":"send.claim"})).await, 403)?;
        // Current peer terminal status/cleanup bookkeeping is allowed; it grants no new I/O.
        self.control(json!({"action":"send.cleanup"})).await?;
        // Explicit fresh-fixture generation replacement; no cluster deployment is claimed.
        sql(self.db()?,"INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision,credential_hash,namespace,service_name) SELECT plugin_id,2,version,image,'ready',configuration_revision,credential_hash,namespace,service_name FROM webdock_auth.plugin_runtime WHERE plugin_id='mail' AND generation=1; UPDATE webdock_auth.plugin_installation SET current_generation=2 WHERE id='mail'",vec![]).await?;
        let mut binding: Value = checked(
            serde_json::from_slice(&checked(
                std::fs::read(self.path("bindings.json")),
                "owned binding observation failed",
            )?),
            "owned binding observation invalid",
        )?;
        binding["runtimes"]["mail"]["generation"] = json!(2);
        replace_private(
            &self.path("bindings.json"),
            serde_json::to_vec(&binding).map_err(|_| "owned replacement binding encode failed")?,
        )?;
        require_control_denial(
            self.control_raw(json!({"action":"send.cleanup"})).await,
            403,
        )?;
        let current = checked(
            NativeControlClient::new(
                checked(
                    webdock_plugin_runtime::pinned_mtls_channel(
                        self.callback_origin.clone(),
                        "localhost".into(),
                        self.runtime_tls(),
                    )
                    .await,
                    "old generation test channel failed",
                )?,
                2,
            ),
            "old generation test client failed",
        )?;
        checked(
            current.call(json!({"action":"send.cleanup"})).await,
            "current enrolled generation lost disabled bookkeeping authority",
        )?;
        require_control_denial(current.call(json!({"action":"send.claim"})).await, 403)?;
        if self.row(id).await?["state"] != "accepted" {
            return Err("disabled bookkeeping changed settled receipt");
        }
        Ok(())
    }
    pub async fn incoming_race(&self, id: &str) -> TestResult {
        self.assert_transition_backend_healthy(id).await?;
        let storage = checked(
            std::fs::read_to_string(self.path("metadata.url")),
            "owned metadata URL unavailable",
        )?;
        let mut connection = checked(
            webdock_plugin_kit::postgres::connect(&storage, Some(self.metadata.pki.ca.as_bytes()))
                .await,
            "owned cross-process fence connection failed",
        )?;
        let transaction = checked(
            connection.transaction().await,
            "owned cross-process fence transaction failed",
        )?;
        checked(
            transaction
                .query_one(
                    "SELECT pg_advisory_xact_lock(hashtextextended('123',704611))",
                    &[],
                )
                .await,
            "owned customer fence acquisition failed",
        )?;
        let before = self.faults.transitions.load(Ordering::SeqCst);
        let denied_before = self.faults.transition_denials.load(Ordering::SeqCst);
        let input = json!({"resource_id":self.domain_id});
        let mut request = Box::pin(self.action("incoming.activate", "incoming-race", &input));
        wait_for_fence_witness(request.as_mut(),async ||{
            checked(transaction.query_one("SELECT pg_stat_clear_snapshot()",&[]).await,"owned lock stats refresh failed")?;
            let waiting=checked(transaction.query_one("SELECT EXISTS(SELECT FROM pg_stat_activity a WHERE a.datname=current_database() AND a.wait_event_type='Lock' AND pg_backend_pid()=ANY(pg_blocking_pids(a.pid)) AND a.query LIKE '%pg_advisory_xact_lock%')",&[]).await,"owned exact lock waiter observation failed")?;
            Ok(waiting.get::<_,bool>(0))
        },Duration::from_secs(3)).await?;
        if self.faults.transitions.load(Ordering::SeqCst) != before {
            return Err("Core barrier ran before product customer fence");
        }
        checked(
            transaction.rollback().await,
            "owned customer fence release failed",
        )?;
        let response = request.await?;
        if response.0 != 503
            || self.faults.transition_denials.load(Ordering::SeqCst) != denied_before + 1
        {
            return Err("released incoming race bypassed actual outstanding-send barrier");
        }
        if self.row(id).await?["state"] != "queued" {
            return Err("incoming race changed canonical outgoing intent");
        }
        // Reverse ordering uses an explicitly labeled retained-reservation sentinel, not a CE
        // deployment or fake successful incoming action. Product denial must precede decryption.
        checked(self.metadata_client()?.execute("INSERT INTO customer_engines(customer_id,id,admin_sealed,desired_enabled,state) VALUES('123',$1::text::uuid,decode('00','hex'),false,'suspended')",&[&uuid::Uuid::new_v4().to_string()]).await,"owned retained-reservation sentinel failed")?;
        let response = self
            .submit_status(
                "incoming-first-sentinel",
                &self.input("retained incoming barrier"),
            )
            .await?;
        checked(
            self.metadata_client()?
                .execute("DELETE FROM customer_engines WHERE customer_id='123'", &[])
                .await,
            "owned retained-reservation sentinel cleanup failed",
        )?;
        if response != 409 {
            return Err(
                "retained incoming reservation requires product conflict, not infrastructure failure",
            );
        }
        Ok(())
    }
    pub async fn generic_material_and_reader_fence(&mut self) -> TestResult {
        use webdock_plugin_runtime::InvocationAuthority;
        let (_, pem) = checked(
            x509_parser::pem::parse_x509_pem(self.runtime_identity.certificate_pem.as_bytes()),
            "actual runtime leaf invalid",
        )?;
        let host = &self.core.as_ref().ok_or("real Core absent")?.host;
        let authority = checked(
            host.runtime_authority("mail".into(), 1),
            "real current runtime authority failed",
        )?;
        checked(
            authority
                .authorize_peer(&PeerIdentity::MutualTls {
                    certificates: vec![pem.contents],
                })
                .await,
            "actual enrolled TLS identity denied before binding derivation",
        )?;
        // Normal same-leaf DER profile must work without a hand-authored ProviderBinding.
        self.config["delivery"]
            .as_object_mut()
            .ok_or("private delivery config absent")?
            .remove("providerBinding");
        replace_private(
            &self.path("runtime.json"),
            serde_json::to_vec(&self.config).map_err(|_| "derived-binding config encode failed")?,
        )?;
        let baseline = self.catcher.counts();
        self.restart(false).await?;
        self.assert_no_new_attempts(baseline, AttemptTotals::NONE)
            .await?;
        let id = job_id(
            &self
                .submit("derived-binding", &self.input("generic TLS binding"))
                .await?,
        )?;
        self.wait_state(&id, "accepted").await?;
        self.assert_capture(&id, 0).await?;
        self.assert_one_message(self.catcher.counts()).await?;
        // Explicit negative old-release metadata fixture, never a claimed old executable rollout.
        // Custody high-water remains2; a schema1-only reader must fail real Core authorization.
        let mut old: Value = checked(
            serde_json::from_str(&self.manifest_raw),
            "actual manifest decode failed",
        )?;
        old["version"] = json!("0.2.0-task4");
        old["schemaVersion"] = json!(1);
        old["readableSchema"] = json!({"min":1,"max":1});
        let raw = serde_json::to_string(&old).map_err(|_| "old reader fixture encoding failed")?;
        sql(self.db()?,"INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES('mail','0.2.0-task4',$1,$2,$3,'https://fixture.invalid','task4-negative-old-reader')",vec![raw.clone().into(),sha256(raw.as_bytes()).into(),old["image"].as_str().ok_or("old reader image absent")?.into()]).await?;
        sql(self.db()?,"UPDATE webdock_auth.plugin_runtime SET version='0.2.0-task4' WHERE plugin_id='mail' AND generation=1; UPDATE webdock_auth.plugin_installation SET version='0.2.0-task4' WHERE id='mail'",vec![]).await?;
        require_control_denial(self.control_raw(json!({"action":"send.claim"})).await, 403)?;
        // High-water is never lowered or reset. This entire owned fixture is deleted afterward.
        Ok(())
    }
    async fn cleanup(&mut self) -> TestResult {
        let resources = json!({"platform":self.platform.journal(),"metadata":self.metadata.journal(),"mail":self.mail.as_ref().map(Process::ownership),"gateway":self.gateway.as_ref().map(Process::ownership),"privateDirectory":self.directory.as_ref().map(|directory|directory.path())});
        self.faults.release();
        let mut clean = true;
        let mut secrets = self.secrets();
        clean &= stop_slot(
            &mut self.gateway,
            &mut self.process_cleanup_uncertain,
            async |process| process.stop(true, &secrets).await,
        )
        .await
        .is_ok();
        clean &= stop_slot(
            &mut self.mail,
            &mut self.process_cleanup_uncertain,
            async |process| process.stop(true, &secrets).await,
        )
        .await
        .is_ok();
        self.control = None;
        self.http = None;
        for server in &mut self.servers {
            clean &= server.stop().await.is_ok();
        }
        self.servers.clear();
        self.catcher.close().await;
        clean &= self.catcher.counts().active == 0 && self.catcher.counts().blocked == 0;
        self.core = None;
        if let Ok(mut backend) = self.faults.backend.lock() {
            backend.take();
        } else {
            clean = false;
        }
        if let Ok(mut auth) = self.faults.authorization.lock() {
            if let Some(value) = auth.as_mut() {
                scrub_json(value);
            }
            auth.take();
        } else {
            clean = false;
        }
        self.meta = None;
        for pool in self.pools.drain(..) {
            clean &= pool.close().await.is_ok();
        }
        if let Some(owner) = self.owner.take() {
            clean &= owner.close().await.is_ok();
        }
        clean &= self.metadata.cleanup().await.is_ok();
        clean &= self.platform.cleanup().await.is_ok();
        for secret in &mut secrets {
            owned::scrub(secret);
        }
        for value in [
            &mut self.cookie,
            &mut self.native_key,
            &mut self.retained_key,
            &mut self.session_token,
            &mut self.rls_secret,
            &mut self.install_secret,
            &mut self.gateway_secret,
        ] {
            owned::scrub(value);
        }
        scrub_json(&mut self.config);
        if self
            .execution_receipt("cleanup_pending_directory", resources.clone(), None)
            .is_err()
        {
            clean = false;
        }
        clean &= finish_private_directory(
            &mut self.directory,
            clean,
            self.process_cleanup_uncertain,
            self.mail.is_some() || self.gateway.is_some(),
        )
        .is_ok();
        self.cleanup_complete = clean;
        if self
            .execution_receipt(
                if clean {
                    "passed_cleanup_verified"
                } else {
                    "cleanup_uncertain"
                },
                resources,
                None,
            )
            .is_err()
        {
            self.cleanup_complete = false;
            return Err("owned cleanup receipt finalization uncertain");
        }
        if clean {
            Ok(())
        } else {
            Err("owned fixture cleanup uncertain")
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleanup_complete
            && let Some(directory) = self.directory.take()
        {
            // Preserve exact IDs for root if cancellation prevented verified cleanup.
            let _=replace_private(&directory.path().join("ownership.json"),serde_json::to_vec(&json!({"run":self.run,"cleanup":"uncertain","processCleanupUncertain":self.process_cleanup_uncertain,"mail":self.mail.as_ref().map(Process::ownership),"gateway":self.gateway.as_ref().map(Process::ownership),"platform":self.platform.journal(),"metadata":self.metadata.journal()})).unwrap_or_default());
            let _ = directory.keep();
        }
    }
}
fn dynamic_port() -> TestResult<u16> {
    let listener = checked(
        std::net::TcpListener::bind("127.0.0.1:0"),
        "owned dynamic port reservation failed",
    )?;
    Ok(checked(listener.local_addr(), "owned dynamic port unavailable")?.port())
}
async fn read_reply(mut response: reqwest::Response) -> TestResult<Value> {
    let mut body = Vec::new();
    while let Some(chunk) = checked(
        response.chunk().await,
        "public response body read failed; details withheld",
    )? {
        if body.len() + chunk.len() > 131072 {
            body.fill(0);
            return Err("public response bound exceeded");
        }
        body.extend_from_slice(&chunk);
    }
    let result = checked(
        serde_json::from_slice(&body),
        "public JSON receipt invalid; contents withheld",
    );
    body.fill(0);
    result
}
pub async fn wait(mut ready: impl AsyncFnMut() -> bool, failure: &'static str) -> TestResult {
    checked(
        timeout(Duration::from_secs(60), async {
            while !ready().await {
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await,
        failure,
    )
}
fn scrub_json(value: &mut Value) {
    match value {
        Value::String(text) => owned::scrub(text),
        Value::Array(values) => values.iter_mut().for_each(scrub_json),
        Value::Object(values) => values.values_mut().for_each(scrub_json),
        _ => {}
    }
}
fn self_private_panic(path: &std::path::Path, payload: &(dyn std::any::Any + Send)) -> TestResult {
    let text = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("nontext panic payload");
    private(path, text)
}
