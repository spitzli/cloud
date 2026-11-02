//! Durable custody for submitted Mail source effects. Drop never proves remote completion.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use webdock_supervisor::{ExecutionError, Result};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    closed: bool,
    pending: BTreeMap<String, Value>,
}
struct Journal {
    _lock: Option<std::fs::File>,
    path: Option<PathBuf>,
    state: State,
}
impl Journal {
    fn persist(&mut self, next: State) -> Result<()> {
        if let Some(path) = &self.path {
            let parent = path.parent().ok_or(ExecutionError::Journal)?;
            let mut file =
                tempfile::NamedTempFile::new_in(parent).map_err(|_| ExecutionError::Journal)?;
            file.write_all(&serde_json::to_vec(&next).map_err(|_| ExecutionError::Journal)?)
                .map_err(|_| ExecutionError::Journal)?;
            file.as_file()
                .sync_all()
                .map_err(|_| ExecutionError::Journal)?;
            file.persist(path).map_err(|_| ExecutionError::Journal)?;
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|_| ExecutionError::Journal)?;
        }
        self.state = next;
        Ok(())
    }
}
#[derive(Clone)]
pub struct MailEffectFence {
    closed: Arc<AtomicBool>,
    effects: Arc<RwLock<()>>,
    journal: Arc<Mutex<Journal>>,
}
#[cfg(test)]
impl Default for MailEffectFence {
    fn default() -> Self {
        Self {
            closed: Arc::new(AtomicBool::new(false)),
            effects: Arc::new(RwLock::new(())),
            journal: Arc::new(Mutex::new(Journal {
                _lock: None,
                path: None,
                state: State::default(),
            })),
        }
    }
}
pub struct MailEffectPermit {
    token: String,
    journal: Arc<Mutex<Journal>>,
    _held: OwnedRwLockReadGuard<()>,
}
pub struct MailEffectDrain {
    _held: OwnedRwLockWriteGuard<()>,
}
impl MailEffectPermit {
    /// Only a definitive provider acknowledgement may discharge this custody.
    pub fn confirmed(self) -> Result<()> {
        let mut journal = self.journal.lock().map_err(|_| ExecutionError::Journal)?;
        let mut next = journal.state.clone();
        if next.pending.remove(&self.token).is_none() {
            return Err(ExecutionError::Journal);
        }
        journal.persist(next)
    }
}
impl MailEffectFence {
    pub fn open(path: &Path) -> Result<Self> {
        crate::config::private_directory(path.parent().ok_or(ExecutionError::Ownership)?)
            .map_err(|_| ExecutionError::Ownership)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(path.with_extension("lock"))
            .map_err(|_| ExecutionError::Journal)?;
        let metadata = lock.metadata().map_err(|_| ExecutionError::Journal)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(ExecutionError::Ownership);
        }
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| ExecutionError::Fenced)?;
        let state = match std::fs::symlink_metadata(path) {
            Ok(_) => serde_json::from_slice::<State>(
                &crate::config::read_private(path, 1024 * 1024)
                    .map_err(|_| ExecutionError::Journal)?,
            )
            .map_err(|_| ExecutionError::Journal)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(_) => return Err(ExecutionError::Journal),
        };
        if state.pending.len() > 10000 {
            return Err(ExecutionError::Journal);
        }
        let closed = state.closed || !state.pending.is_empty();
        let mut journal = Journal {
            _lock: Some(lock),
            path: Some(path.into()),
            state,
        };
        let mut next = journal.state.clone();
        next.closed = closed;
        journal.persist(next)?;
        Ok(Self {
            closed: Arc::new(AtomicBool::new(closed)),
            effects: Arc::new(RwLock::new(())),
            journal: Arc::new(Mutex::new(journal)),
        })
    }
    pub async fn enter(&self) -> Result<MailEffectPermit> {
        self.enter_effect(Value::Null).await
    }
    pub async fn enter_effect(&self, fingerprint: Value) -> Result<MailEffectPermit> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ExecutionError::Fenced);
        }
        let held = self.effects.clone().read_owned().await;
        let mut journal = self.journal.lock().map_err(|_| ExecutionError::Journal)?;
        if self.closed.load(Ordering::SeqCst)
            || journal.state.closed
            || journal.state.pending.len() >= 10000
        {
            return Err(ExecutionError::Fenced);
        }
        let token = uuid::Uuid::new_v4().to_string();
        let mut next = journal.state.clone();
        next.pending.insert(token.clone(), fingerprint);
        journal.persist(next)?;
        Ok(MailEffectPermit {
            token,
            journal: self.journal.clone(),
            _held: held,
        })
    }
    pub async fn close_and_drain(&self) -> Result<MailEffectDrain> {
        self.closed.store(true, Ordering::SeqCst);
        {
            let mut journal = self.journal.lock().map_err(|_| ExecutionError::Journal)?;
            let mut next = journal.state.clone();
            next.closed = true;
            journal.persist(next)?;
        }
        let held = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.effects.clone().write_owned(),
        )
        .await
        .map_err(|_| ExecutionError::Expired)?;
        if !self
            .journal
            .lock()
            .map_err(|_| ExecutionError::Journal)?
            .state
            .pending
            .is_empty()
        {
            return Err(ExecutionError::Fenced);
        }
        Ok(MailEffectDrain { _held: held })
    }
    /// Caller retains a current Core same-fence compensation grant. Uncertainty cannot reopen.
    pub fn reopen(&self, drain: MailEffectDrain) -> Result<()> {
        let mut journal = self.journal.lock().map_err(|_| ExecutionError::Journal)?;
        if !journal.state.pending.is_empty() {
            return Err(ExecutionError::Fenced);
        }
        let mut next = journal.state.clone();
        next.closed = false;
        journal.persist(next)?;
        drop(drain);
        self.closed.store(false, Ordering::SeqCst);
        Ok(())
    }
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// Exact Core custody for one concrete application source. No secret plaintext is stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppFenceBinding {
    #[serde(rename = "appID")]
    pub app_id: String,
    #[serde(rename = "projectID")]
    pub project_id: String,
    #[serde(rename = "customerID")]
    pub customer_id: String,
    #[serde(rename = "clusterID")]
    pub cluster_id: String,
    #[serde(rename = "policyID")]
    pub policy_id: String,
    pub policy_revision: u64,
    pub agent_generation: u64,
    pub source_revision: u64,
    #[serde(rename = "sourceUID")]
    pub source_uid: String,
    #[serde(rename = "specSHA256")]
    pub spec_sha256: String,
    #[serde(rename = "environmentSHA256")]
    pub environment_sha256: String,
    #[serde(rename = "sourceOperationID")]
    pub source_operation_id: String,
    pub source_operation_generation: u64,
    pub source_operation_status: String,
    pub source_operation_fingerprint: String,
    #[serde(rename = "operationID")]
    pub operation_id: String,
    pub generation: u64,
    pub epoch: String,
    pub fence_fingerprint: String,
}
impl AppFenceBinding {
    fn validate(&self) -> Result<()> {
        for id in [
            &self.app_id,
            &self.project_id,
            &self.customer_id,
            &self.cluster_id,
            &self.policy_id,
            &self.source_operation_id,
            &self.operation_id,
        ] {
            app_id(id)?;
        }
        let hash = |v: &str| {
            v.len() == 64
                && v.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        };
        if self.source_revision == 0
            || self.policy_revision == 0
            || self.agent_generation == 0
            || self.generation == 0
            || self.source_operation_generation == 0
            || self.source_uid.is_empty()
            || self.source_uid.len() > 256
            || ![
                &self.spec_sha256,
                &self.environment_sha256,
                &self.source_operation_fingerprint,
                &self.fence_fingerprint,
            ]
            .into_iter()
            .all(|v| hash(v))
            || self.source_operation_status != "succeeded"
            || !uuid::Uuid::parse_str(&self.epoch)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.epoch)
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
}
fn app_id(id: &str) -> Result<()> {
    if !id.starts_with('0')
        && id.len() <= 19
        && id.bytes().all(|c| c.is_ascii_digit())
        && id.parse::<i64>().is_ok_and(|v| v > 0)
    {
        Ok(())
    } else {
        Err(ExecutionError::Ownership)
    }
}
const APP_RECENT_LIMIT: usize = 256;
const APP_RECENT_KEEP: usize = 128;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppCommandOrder {
    revision: u64,
    generation: u64,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppCommandStamp {
    revision: u64,
    generation: u64,
    operation_id: String,
    payload_fingerprint: String,
}
impl AppCommandStamp {
    pub fn from_command(command: &webdock_supervisor::Command) -> Result<Self> {
        command.validate()?;
        let stamp = Self {
            revision: command.spec.revision,
            generation: command.generation,
            operation_id: command.operation_id.clone(),
            payload_fingerprint: command.payload_fingerprint()?,
        };
        stamp.validate()?;
        Ok(stamp)
    }
    fn order(&self) -> AppCommandOrder {
        AppCommandOrder {
            revision: self.revision,
            generation: self.generation,
        }
    }
    fn validate(&self) -> Result<()> {
        app_id(&self.operation_id)?;
        if self.revision == 0
            || self.revision > 9_007_199_254_740_991
            || self.generation == 0
            || self.generation > 2_147_483_646
            || self.payload_fingerprint.len() != 64
            || !self
                .payload_fingerprint
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppReleasedReceipt {
    binding: AppFenceBinding,
    previous_sha256: String,
    sha256: String,
}
impl AppReleasedReceipt {
    fn validate(&self, app: &str) -> Result<()> {
        self.binding.validate()?;
        if self.binding.app_id != app
            || self.previous_sha256.len() != 64
            || !self
                .previous_sha256
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || self.sha256 != webdock_supervisor::digest(&(&self.previous_sha256, &self.binding))?
        {
            return Err(ExecutionError::Journal);
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppState {
    version: u8,
    app_id: String,
    fence: Option<AppFenceBinding>,
    resume_completed: bool,
    pending: BTreeMap<String, AppCommandStamp>,
    completed: Vec<AppCommandStamp>,
    #[serde(default)]
    retired_through: Option<AppCommandOrder>,
    released: Option<AppReleasedReceipt>,
}
impl AppState {
    fn compact_confirmed(&mut self) {
        if self.fence.is_some()
            || !self.pending.is_empty()
            || self.completed.len() <= APP_RECENT_LIMIT
        {
            return;
        }
        let retire = self.completed.len() - APP_RECENT_KEEP;
        self.retired_through = Some(self.completed[retire - 1].order());
        self.completed.drain(..retire);
    }
}
struct AppJournal {
    lock: std::fs::File,
    path: PathBuf,
    state: AppState,
    poisoned: bool,
}
impl AppJournal {
    fn persist(&mut self, next: AppState) -> Result<()> {
        if self.poisoned {
            return Err(ExecutionError::Journal);
        }
        let result = (|| -> std::io::Result<()> {
            let parent = self.path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(&serde_json::to_vec(&next)?)?;
            temporary.as_file().sync_all()?;
            temporary.persist(&self.path).map_err(|e| e.error)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            self.poisoned = true;
            return Err(ExecutionError::Journal);
        }
        self.state = next;
        Ok(())
    }
    fn check(&self) -> Result<()> {
        if self.poisoned {
            Err(ExecutionError::Journal)
        } else {
            crate::config::private_directory(self.path.parent().ok_or(ExecutionError::Ownership)?)
                .map_err(|_| ExecutionError::Ownership)?;
            let lock = std::fs::symlink_metadata(self.path.with_extension("lock"))
                .map_err(|_| ExecutionError::Journal)?;
            let held = self.lock.metadata().map_err(|_| ExecutionError::Journal)?;
            if !lock.is_file()
                || lock.nlink() != 1
                || lock.uid() != rustix::process::geteuid().as_raw()
                || lock.mode() & 0o777 != 0o600
                || (lock.dev(), lock.ino()) != (held.dev(), held.ino())
            {
                return Err(ExecutionError::Journal);
            }
            let durable = crate::config::read_private(&self.path, 1024 * 1024)
                .map_err(|_| ExecutionError::Journal)?;
            if durable != serde_json::to_vec(&self.state).map_err(|_| ExecutionError::Journal)? {
                return Err(ExecutionError::Journal);
            }
            Ok(())
        }
    }
}
/// One daemon owns this registry and shares its Arc with normal Docker and recovery.
/// A cancelled/lost provider response remains pending across process restart.
pub(crate) struct AppEffectFenceRegistry {
    directory: PathBuf,
    apps: Mutex<BTreeMap<String, Arc<Mutex<AppJournal>>>>,
}
impl AppEffectFenceRegistry {
    pub fn available(&self) -> Result<()> {
        crate::config::private_directory(&self.directory).map_err(|_| ExecutionError::Ownership)?;
        for app in self
            .apps
            .lock()
            .map_err(|_| ExecutionError::Journal)?
            .values()
        {
            app.lock().map_err(|_| ExecutionError::Journal)?.check()?;
        }
        Ok(())
    }
    pub fn open(directory: PathBuf) -> Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        crate::config::private_directory(directory.parent().ok_or(ExecutionError::Ownership)?)
            .map_err(|_| ExecutionError::Ownership)?;
        match std::fs::symlink_metadata(&directory) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&directory)
                    .map_err(|_| ExecutionError::Journal)?;
                std::fs::File::open(directory.parent().ok_or(ExecutionError::Ownership)?)
                    .and_then(|parent| parent.sync_all())
                    .map_err(|_| ExecutionError::Journal)?;
            }
            Err(_) => return Err(ExecutionError::Ownership),
        }
        crate::config::private_directory(&directory).map_err(|_| ExecutionError::Ownership)?;
        Ok(Self {
            directory,
            apps: Mutex::new(BTreeMap::new()),
        })
    }
    fn app(&self, id: &str, allow_create: bool) -> Result<Arc<Mutex<AppJournal>>> {
        app_id(id)?;
        crate::config::private_directory(&self.directory).map_err(|_| ExecutionError::Ownership)?;
        let mut apps = self.apps.lock().map_err(|_| ExecutionError::Journal)?;
        if let Some(app) = apps.get(id) {
            return Ok(app.clone());
        }
        let path = self.directory.join(format!("{id}.json"));
        let lock_path = path.with_extension("lock");
        let exists = |path: &Path| -> Result<bool> {
            match std::fs::symlink_metadata(path) {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(_) => Err(ExecutionError::Ownership),
            }
        };
        let state_existed = exists(&path)?;
        let lock_existed = exists(&lock_path)?;
        if state_existed != lock_existed {
            return Err(ExecutionError::Journal);
        }
        let fresh = !state_existed;
        if fresh && !allow_create {
            return Err(ExecutionError::Journal);
        }
        let lock = std::fs::OpenOptions::new()
            .create_new(fresh)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&lock_path)
            .map_err(|_| ExecutionError::Journal)?;
        let meta = lock.metadata().map_err(|_| ExecutionError::Journal)?;
        if !meta.is_file()
            || meta.nlink() != 1
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o777 != 0o600
        {
            return Err(ExecutionError::Ownership);
        }
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| ExecutionError::Fenced)?;
        let state = match std::fs::symlink_metadata(&path) {
            Ok(_) => serde_json::from_slice::<AppState>(
                &crate::config::read_private(&path, 1024 * 1024)
                    .map_err(|_| ExecutionError::Ownership)?,
            )
            .map_err(|_| ExecutionError::Journal)?,
            Err(e) if fresh && e.kind() == std::io::ErrorKind::NotFound => AppState {
                version: 2,
                app_id: id.into(),
                fence: None,
                resume_completed: false,
                pending: BTreeMap::new(),
                completed: Vec::new(),
                retired_through: None,
                released: None,
            },
            Err(_) => return Err(ExecutionError::Ownership),
        };
        if state.version != 2
            || state.app_id != id
            || state.pending.len() > 1
            || state.completed.len() > APP_RECENT_LIMIT + 1
            || state.retired_through.is_some() && state.completed.is_empty()
            || state.pending.values().any(|s| s.validate().is_err())
            || state.completed.iter().any(|s| {
                s.validate().is_err() || state.retired_through.is_some_and(|r| s.order() <= r)
            })
            || state.completed.windows(2).any(|s| {
                s[0].order() >= s[1].order()
                    || s[0].revision == s[1].revision
                        && (s[0].operation_id != s[1].operation_id
                            || s[0].payload_fingerprint != s[1].payload_fingerprint)
            })
            || state.retired_through.is_some_and(|r| {
                r.revision == 0
                    || r.revision > 9_007_199_254_740_991
                    || r.generation == 0
                    || r.generation > 2_147_483_646
            })
            || state.resume_completed && state.fence.is_none()
            || state
                .fence
                .as_ref()
                .is_some_and(|f| f.app_id != id || f.validate().is_err())
            || state
                .released
                .as_ref()
                .is_some_and(|r| r.validate(id).is_err())
        {
            return Err(ExecutionError::Journal);
        }
        let journal = AppJournal {
            lock,
            path,
            state,
            poisoned: false,
        };
        if fresh {
            // A crash between exclusive lock creation and this first write leaves
            // an incomplete pair. Restart must reject it, never infer first-ever.
            let mut initial = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(&journal.path)
                .map_err(|_| ExecutionError::Journal)?;
            initial
                .write_all(
                    &serde_json::to_vec(&journal.state).map_err(|_| ExecutionError::Journal)?,
                )
                .and_then(|()| initial.sync_all())
                .map_err(|_| ExecutionError::Journal)?;
            std::fs::File::open(&self.directory)
                .and_then(|d| d.sync_all())
                .map_err(|_| ExecutionError::Journal)?;
        }
        let app = Arc::new(Mutex::new(journal));
        apps.insert(id.into(), app.clone());
        Ok(app)
    }
    pub fn enter(&self, app_id: &str, stamp: AppCommandStamp) -> Result<AppEffectAdmission> {
        stamp.validate()?;
        let app = self.app(app_id, true)?;
        let mut journal = app.lock().map_err(|_| ExecutionError::Journal)?;
        journal.check()?;
        if journal.state.fence.is_some() || !journal.state.pending.is_empty() {
            return Err(ExecutionError::Fenced);
        }
        if let Some(prior) = journal
            .state
            .completed
            .iter()
            .find(|p| p.order() == stamp.order())
        {
            return if prior == &stamp {
                Ok(AppEffectAdmission::Verify)
            } else {
                Err(ExecutionError::Fenced)
            };
        }
        if journal
            .state
            .retired_through
            .is_some_and(|r| stamp.order() <= r)
        {
            return Err(ExecutionError::Fenced);
        }
        if let Some(last) = journal.state.completed.last()
            && (stamp.order() <= last.order()
                || stamp.revision == last.revision
                    && (stamp.operation_id != last.operation_id
                        || stamp.payload_fingerprint != last.payload_fingerprint))
        {
            return Err(ExecutionError::Fenced);
        }
        let token = uuid::Uuid::new_v4().to_string();
        let mut next = journal.state.clone();
        next.pending.insert(token.clone(), stamp);
        journal.persist(next)?;
        drop(journal);
        Ok(AppEffectAdmission::Execute(AppEffectPermit { app, token }))
    }
    pub async fn close_authorized(
        &self,
        binding: AppFenceBinding,
        current_core_guard: impl std::future::Future<Output = Result<()>>,
    ) -> Result<SameFencePermit> {
        // The bounded chain is audit continuity, not membership/authority for older operations.
        current_core_guard.await?;
        self.close(binding)
    }
    fn close(&self, binding: AppFenceBinding) -> Result<SameFencePermit> {
        binding.validate()?;
        let app = self.app(&binding.app_id, true)?;
        let mut journal = app.lock().map_err(|_| ExecutionError::Journal)?;
        journal.check()?;
        if journal
            .state
            .released
            .as_ref()
            .is_some_and(|f| f.binding.operation_id == binding.operation_id)
            || journal.state.fence.as_ref().is_some_and(|f| *f != binding)
        {
            return Err(ExecutionError::Fenced);
        }
        if journal.state.fence.is_none() {
            let mut next = journal.state.clone();
            next.fence = Some(binding.clone());
            journal.persist(next)?;
        }
        drop(journal);
        Ok(SameFencePermit { app, binding })
    }
    pub async fn finish_release_ack(
        &self,
        binding: AppFenceBinding,
        current_core_guard: impl std::future::Future<Output = Result<()>>,
    ) -> Result<()> {
        binding.validate()?;
        let app = self.app(&binding.app_id, false)?;
        let already_released = || -> Result<bool> {
            let journal = app.lock().map_err(|_| ExecutionError::Journal)?;
            journal.check()?;
            if journal.state.fence.is_none()
                && journal
                    .state
                    .released
                    .as_ref()
                    .is_some_and(|r| r.binding == binding)
            {
                return Ok(true);
            }
            if journal.state.fence.as_ref() != Some(&binding)
                || !journal.state.resume_completed
                || !journal.state.pending.is_empty()
            {
                return Err(ExecutionError::Fenced);
            }
            Ok(false)
        };
        if already_released()? {
            return Ok(());
        }
        current_core_guard.await?;
        if already_released()? {
            return Ok(());
        }
        SameFencePermit {
            app,
            binding: binding.clone(),
        }
        .release_after_core_ack(&binding)
    }
}
pub(crate) enum AppEffectAdmission {
    Execute(AppEffectPermit),
    Verify,
}
#[cfg(test)]
impl AppEffectAdmission {
    fn confirmed(self) -> Result<()> {
        match self {
            Self::Execute(p) => p.confirmed(),
            Self::Verify => Ok(()),
        }
    }
}
pub(crate) struct AppEffectPermit {
    app: Arc<Mutex<AppJournal>>,
    token: String,
}
impl AppEffectPermit {
    pub fn check(&self) -> Result<()> {
        let journal = self.app.lock().map_err(|_| ExecutionError::Journal)?;
        journal.check()?;
        if journal.state.fence.is_some() || !journal.state.pending.contains_key(&self.token) {
            Err(ExecutionError::Fenced)
        } else {
            Ok(())
        }
    }
    /// Discharge this known-complete command only. The source fence is never opened.
    pub fn confirmed(self) -> Result<()> {
        let mut journal = self.app.lock().map_err(|_| ExecutionError::Journal)?;
        journal.check()?;
        let mut next = journal.state.clone();
        let stamp = next
            .pending
            .remove(&self.token)
            .ok_or(ExecutionError::Journal)?;
        next.completed.push(stamp);
        next.compact_confirmed();
        journal.persist(next)
    }
}
pub(crate) struct SameFencePermit {
    app: Arc<Mutex<AppJournal>>,
    binding: AppFenceBinding,
}
impl SameFencePermit {
    pub fn source(&self, app: &str, revision: u64, uid: &str) -> Result<()> {
        self.drained()?;
        if self.binding.app_id != app
            || self.binding.source_revision != revision
            || self.binding.source_uid != uid
        {
            return Err(ExecutionError::Ownership);
        }
        Ok(())
    }
    pub fn drained(&self) -> Result<()> {
        let journal = self.app.lock().map_err(|_| ExecutionError::Journal)?;
        journal.check()?;
        if journal.state.fence.as_ref() != Some(&self.binding) || !journal.state.pending.is_empty()
        {
            Err(ExecutionError::Fenced)
        } else {
            Ok(())
        }
    }
    /// Call only after the recovery-controlled provider start definitively completed.
    pub fn record_resume_completion(&self, binding: &AppFenceBinding) -> Result<()> {
        self.drained()?;
        if binding != &self.binding {
            return Err(ExecutionError::Fenced);
        }
        let mut journal = self.app.lock().map_err(|_| ExecutionError::Journal)?;
        let mut next = journal.state.clone();
        next.resume_completed = true;
        journal.persist(next)
    }
    /// The caller must first receive the exact current Core resumed event over enrolled transport.
    pub fn release_after_core_ack(&self, binding: &AppFenceBinding) -> Result<()> {
        self.drained()?;
        if binding != &self.binding {
            return Err(ExecutionError::Fenced);
        }
        let mut journal = self.app.lock().map_err(|_| ExecutionError::Journal)?;
        if !journal.state.resume_completed {
            return Err(ExecutionError::Fenced);
        }
        let mut next = journal.state.clone();
        let previous_sha256 = next
            .released
            .as_ref()
            .map(|r| r.sha256.clone())
            .unwrap_or_else(|| "0".repeat(64));
        let sha256 = webdock_supervisor::digest(&(&previous_sha256, &self.binding))?;
        next.released = Some(AppReleasedReceipt {
            binding: self.binding.clone(),
            previous_sha256,
            sha256,
        });
        next.fence = None;
        next.resume_completed = false;
        next.compact_confirmed();
        journal.persist(next)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_submitted_effect_must_not_be_treated_as_drained() {
        let gate = MailEffectFence::default();
        let submitted = gate.enter().await.unwrap();
        drop(submitted); // lost remote reply/cancelled future; the remote write may still finish
        assert!(
            gate.close_and_drain().await.is_err(),
            "dropping a local permit is not provider completion"
        );
    }
    #[tokio::test]
    async fn dropped_effect_survives_process_restart_and_keeps_dispatch_closed() {
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("effects.json");
        let gate = MailEffectFence::open(&path).unwrap();
        assert!(
            MailEffectFence::open(&path).is_err(),
            "another executor cannot overwrite durable custody"
        );
        let submitted = gate
            .enter_effect(serde_json::json!({"method":"PATCH","intentSha256":"a".repeat(64)}))
            .await
            .unwrap();
        drop(submitted);
        drop(gate);
        let restarted = MailEffectFence::open(&path).unwrap();
        assert!(restarted.closed());
        assert!(restarted.enter().await.is_err());
        assert!(restarted.close_and_drain().await.is_err());
    }
    #[tokio::test]
    async fn captured_old_intent_released_after_fence_cannot_enter_provider_effect() {
        let gate = MailEffectFence::default();
        let provider_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (release, held) = tokio::sync::oneshot::channel();
        let worker_gate = gate.clone();
        let calls = provider_calls.clone();
        let worker = tokio::spawn(async move {
            held.await.unwrap();
            let permit = worker_gate.enter().await?;
            calls.fetch_add(1, Ordering::SeqCst);
            permit.confirmed()?;
            Ok::<_, ExecutionError>(())
        });
        let drain = gate.close_and_drain().await.unwrap();
        release.send(()).unwrap();
        assert!(matches!(worker.await.unwrap(), Err(ExecutionError::Fenced)));
        assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
        gate.reopen(drain).unwrap();
        assert!(gate.enter().await.is_ok());
    }
    #[tokio::test]
    async fn an_already_entered_provider_effect_prevents_drain_acknowledgement() {
        let gate = MailEffectFence::default();
        let entered = gate.enter().await.unwrap();
        let closer = gate.clone();
        let drain = tokio::spawn(async move { closer.close_and_drain().await });
        while !gate.closed() {
            tokio::task::yield_now().await;
        }
        assert!(!drain.is_finished());
        assert!(gate.enter().await.is_err());
        entered.confirmed().unwrap();
        assert!(drain.await.unwrap().is_ok());
    }

    fn app_stamp(revision: u64, label: &str) -> AppCommandStamp {
        AppCommandStamp {
            revision,
            generation: 1,
            operation_id: (1000 + revision).to_string(),
            payload_fingerprint: webdock_supervisor::digest(&label).unwrap(),
        }
    }
    fn app_binding() -> AppFenceBinding {
        serde_json::from_value(serde_json::json!({
            "appID":"123","projectID":"124","customerID":"125","clusterID":"126",
            "policyID":"127","policyRevision":1,"agentGeneration":1,
            "sourceRevision":1,"sourceUID":"a".repeat(64),"specSHA256":"b".repeat(64),
            "environmentSHA256":"c".repeat(64),"sourceOperationID":"128",
            "sourceOperationGeneration":1,"sourceOperationStatus":"succeeded",
            "sourceOperationFingerprint":"d".repeat(64),"operationID":"129","generation":1,
            "epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","fenceFingerprint":"e".repeat(64)
        }))
        .unwrap()
    }
    #[test]
    fn application_fence_survives_restart_and_normal_success_never_reopens_it() {
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        let command = registry
            .enter("123", app_stamp(1, "normal-command"))
            .unwrap();
        let recovery = registry.close(app_binding()).unwrap();
        assert!(recovery.drained().is_err());
        command.confirmed().unwrap();
        recovery.drained().unwrap();
        assert!(
            registry
                .enter("123", app_stamp(1, "another-command"))
                .is_err()
        );
        // A different application is not globally frozen.
        registry
            .enter("130", app_stamp(1, "unrelated-command"))
            .unwrap()
            .confirmed()
            .unwrap();
        drop(recovery);
        drop(registry);
        let restarted = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        assert!(
            restarted
                .enter("123", app_stamp(1, "after-restart"))
                .is_err()
        );
        restarted.close(app_binding()).unwrap().drained().unwrap();
    }
    #[test]
    fn application_unknown_effect_cannot_drain_after_client_stop_or_restart() {
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        drop(
            registry
                .enter("123", app_stamp(1, "remote-write-response-lost"))
                .unwrap(),
        );
        assert!(registry.close(app_binding()).unwrap().drained().is_err());
        drop(registry);
        let restarted = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        assert!(restarted.close(app_binding()).unwrap().drained().is_err());
        assert!(
            restarted
                .enter("123", app_stamp(1, "must-not-retry"))
                .is_err()
        );
    }
    #[test]
    fn application_same_fence_only_and_exact_core_resume_acknowledgement() {
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        let recovery = registry.close(app_binding()).unwrap();
        let mut changed = app_binding();
        changed.source_uid = "f".repeat(64);
        assert!(registry.close(changed).is_err());
        let mut ack = app_binding();
        ack.generation += 1;
        assert!(recovery.release_after_core_ack(&ack).is_err());
        assert!(registry.enter("123", app_stamp(1, "still-closed")).is_err());
        assert!(recovery.release_after_core_ack(&app_binding()).is_err());
        recovery.record_resume_completion(&app_binding()).unwrap();
        recovery.release_after_core_ack(&app_binding()).unwrap();
        registry
            .enter("123", app_stamp(2, "fresh-authorized-command"))
            .unwrap()
            .confirmed()
            .unwrap();
        // Old replay cannot close a released fence and stop fresh work again.
        assert!(registry.close(app_binding()).is_err());
    }
    #[test]
    fn application_registry_rejects_corrupt_unsafe_and_foreign_files() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("apps");
        let registry = AppEffectFenceRegistry::open(path.clone()).unwrap();
        assert!(registry.enter("../123", app_stamp(1, "bad-path")).is_err());
        assert!(
            registry
                .enter("0123", app_stamp(1, "noncanonical-id"))
                .is_err()
        );
        std::fs::write(path.join("123.json"), b"{}").unwrap();
        std::fs::set_permissions(
            path.join("123.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(registry.enter("123", app_stamp(1, "corrupt")).is_err());
        symlink(path.join("123.json"), path.join("124.json")).unwrap();
        assert!(registry.enter("124", app_stamp(1, "symlink")).is_err());
        std::fs::hard_link(path.join("123.json"), path.join("125.json")).unwrap();
        assert!(registry.enter("125", app_stamp(1, "hardlink")).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            registry
                .enter("126", app_stamp(1, "changed-directory"))
                .is_err()
        );
    }
    #[test]
    fn application_registry_missing_half_of_durable_pair_never_reinitializes() {
        use std::os::unix::fs::PermissionsExt;
        for remove in ["123.json", "123.lock"] {
            let root = tempfile::tempdir().unwrap();
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            let directory = root.path().join("apps");
            let registry = AppEffectFenceRegistry::open(directory.clone()).unwrap();
            drop(
                registry
                    .enter("123", app_stamp(1, "submitted-before-restart"))
                    .unwrap(),
            );
            drop(registry);
            assert!(directory.join("123.json").is_file());
            assert!(directory.join("123.lock").is_file());
            std::fs::remove_file(directory.join(remove)).unwrap();
            let restarted = AppEffectFenceRegistry::open(directory).unwrap();
            assert!(
                restarted
                    .enter("123", app_stamp(1, "must-not-reinitialize"))
                    .is_err(),
                "missing {remove} was treated as a first-ever application"
            );
        }
    }
    #[test]
    fn application_confirmed_history_does_not_permanently_exhaust_the_source() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        for revision in 1..=2100 {
            registry
                .enter(
                    "123",
                    app_stamp(revision, &format!("confirmed-intent-{revision}")),
                )
                .unwrap()
                .confirmed()
                .unwrap();
        }
        let path = root.path().join("apps/123.json");
        let state: AppState = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(state.completed.len() <= APP_RECENT_LIMIT);
        assert!(state.retired_through.is_some());
        assert!(std::fs::metadata(&path).unwrap().len() < 1024 * 1024);
        assert!(
            registry
                .enter("123", app_stamp(1, "confirmed-intent-1"))
                .is_err()
        );
        let recent = app_stamp(2100, "confirmed-intent-2100");
        assert!(matches!(
            registry.enter("123", recent.clone()).unwrap(),
            AppEffectAdmission::Verify
        ));
        let mut changed = recent.clone();
        changed.payload_fingerprint = "f".repeat(64);
        assert!(registry.enter("123", changed).is_err());
        let mut reconciled = recent.clone();
        reconciled.generation = 2;
        registry
            .enter("123", reconciled.clone())
            .unwrap()
            .confirmed()
            .unwrap();
        let mut foreign = reconciled.clone();
        foreign.operation_id = "987654".into();
        foreign.generation = 3;
        assert!(registry.enter("123", foreign).is_err());
        // New operations reset generation to one but must advance the app revision.
        let next = app_stamp(2101, "next-normal-intent");
        registry
            .enter("123", next.clone())
            .unwrap()
            .confirmed()
            .unwrap();
        drop(registry);
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        assert!(matches!(
            registry.enter("123", next).unwrap(),
            AppEffectAdmission::Verify
        ));
        assert!(matches!(
            registry.enter("123", reconciled).unwrap(),
            AppEffectAdmission::Verify
        ));
        assert!(
            registry
                .enter("123", app_stamp(1, "confirmed-intent-1"))
                .is_err()
        );
        let before = std::fs::read(&path).unwrap();
        drop(
            registry
                .enter("123", app_stamp(2102, "unknown-provider-outcome"))
                .unwrap(),
        );
        let pending: AppState = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let before: AppState = serde_json::from_slice(&before).unwrap();
        assert_eq!(pending.retired_through, before.retired_through);
        assert_eq!(pending.completed.len(), before.completed.len());
        assert_eq!(pending.pending.len(), 1);
        assert!(
            registry
                .enter("123", app_stamp(2103, "must-not-pass-unknown"))
                .is_err()
        );
    }
    #[tokio::test]
    async fn application_released_chain_is_bounded_and_never_authorizes_old_core_work() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("apps");
        let registry = AppEffectFenceRegistry::open(directory.clone()).unwrap();
        for cycle in 1..=1100 {
            let mut binding = app_binding();
            binding.operation_id = (10_000 + cycle).to_string();
            let permit = registry
                .close_authorized(binding.clone(), async { Ok(()) })
                .await
                .unwrap();
            permit.record_resume_completion(&binding).unwrap();
            permit.release_after_core_ack(&binding).unwrap();
        }
        let path = directory.join("123.json");
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() < 16384);
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let denied = registry
            .close_authorized(app_binding(), async {
                checks.fetch_add(1, Ordering::SeqCst);
                Err(ExecutionError::Fenced)
            })
            .await;
        assert!(denied.is_err());
        assert_eq!(checks.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "stale Core guard reached local custody"
        );
        let mut latest = app_binding();
        latest.operation_id = "11100".into();
        assert!(
            registry
                .close_authorized(latest, async { Ok(()) })
                .await
                .is_err()
        );
        drop(registry);
        let mut corrupted: serde_json::Value = serde_json::from_slice(&before).unwrap();
        corrupted["released"]["sha256"] = serde_json::json!("f".repeat(64));
        std::fs::write(&path, serde_json::to_vec(&corrupted).unwrap()).unwrap();
        let restarted = AppEffectFenceRegistry::open(directory).unwrap();
        assert!(
            restarted
                .enter("123", app_stamp(2, "corrupt-chain"))
                .is_err()
        );
    }
    #[test]
    fn application_old_or_malformed_state_is_not_a_migration_to_open() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("apps");
        let registry = AppEffectFenceRegistry::open(directory.clone()).unwrap();
        registry
            .enter("123", app_stamp(1, "first"))
            .unwrap()
            .confirmed()
            .unwrap();
        drop(registry);
        let path = directory.join("123.json");
        let mut old: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        old["version"] = serde_json::json!(1);
        let bytes = serde_json::to_vec(&old).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let restarted = AppEffectFenceRegistry::open(directory).unwrap();
        assert!(
            restarted
                .enter("123", app_stamp(2, "must-not-open-old-state"))
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    #[test]
    fn application_watermark_without_recent_identity_anchor_is_corrupt() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let directory = root.path().join("apps");
        let registry = AppEffectFenceRegistry::open(directory.clone()).unwrap();
        registry
            .enter("123", app_stamp(1, "original"))
            .unwrap()
            .confirmed()
            .unwrap();
        drop(registry);
        let path = directory.join("123.json");
        let mut invalid: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        invalid["retired_through"] = serde_json::json!({"revision":1,"generation":1});
        invalid["completed"] = serde_json::json!([]);
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let restarted = AppEffectFenceRegistry::open(directory).unwrap();
        let mut changed = app_stamp(1, "changed-operation");
        changed.generation = 2;
        changed.operation_id = "9999".into();
        assert!(restarted.enter("123", changed).is_err());
    }
    #[tokio::test]
    async fn application_resume_ack_requires_existing_completion_and_is_idempotent() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::AtomicUsize;
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let registry = AppEffectFenceRegistry::open(root.path().join("apps")).unwrap();
        let binding = app_binding();
        assert!(
            registry
                .finish_release_ack(binding.clone(), async { Ok(()) })
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_dir(root.path().join("apps")).unwrap().count(),
            0,
            "ACK adopted a missing journal"
        );
        let permit = registry.close(binding.clone()).unwrap();
        let path = root.path().join("apps/123.json");
        let before = std::fs::read(&path).unwrap();
        assert!(
            registry
                .finish_release_ack(binding.clone(), async { Ok(()) })
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "ACK inferred provider completion"
        );
        permit.record_resume_completion(&binding).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            registry
                .finish_release_ack(binding.clone(), async { Err(ExecutionError::Fenced) })
                .await
                .is_err()
        );
        let mut wrong = binding.clone();
        wrong.source_uid = "f".repeat(64);
        assert!(
            registry
                .finish_release_ack(wrong, async { Ok(()) })
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let calls = AtomicUsize::new(0);
        registry
            .finish_release_ack(binding.clone(), async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let released = std::fs::read(&path).unwrap();
        registry
            .finish_release_ack(binding, async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ExecutionError::Fenced)
            })
            .await
            .unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "read-only replay minted a fresh grant"
        );
        assert_eq!(std::fs::read(&path).unwrap(), released);
    }
}
