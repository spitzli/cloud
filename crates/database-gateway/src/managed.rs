//! Isolated, short-lived database-browser processes with private saved-connection storage.
use crate::{
    Error, Result,
    control::{ManagedAccess, RuntimeAccess},
};
use serde_json::json;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore, watch},
};
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct ManagedRuntime {
    pub access: RuntimeAccess,
    pub authentication_origin: String,
}
#[async_trait::async_trait]
pub trait ManagedRuntimes: Send + Sync + 'static {
    async fn acquire(&self, session: &str, access: &ManagedAccess) -> Result<ManagedRuntime>;
    async fn release(&self, session: &str);
    async fn shutdown(&self);
}
#[derive(Clone)]
pub struct ProcessConfig {
    pub executable: PathBuf,
    pub web_root: Option<PathBuf>,
    pub temporary_directory: PathBuf,
    pub capacity: usize,
    pub startup_timeout: Duration,
}
struct Entry {
    identity: String,
    ready: watch::Sender<Option<Result<ManagedRuntime>>>,
    cancelled: CancellationToken,
    child: Mutex<Option<Child>>,
    directory: Mutex<Option<tempfile::TempDir>>,
    _permit: OwnedSemaphorePermit,
}
struct Inner {
    config: ProcessConfig,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    capacity: Arc<Semaphore>,
    stopped: AtomicBool,
    shutdown: CancellationToken,
    client: reqwest::Client,
}
#[derive(Clone)]
pub struct ProcessManager {
    inner: Arc<Inner>,
}
fn unavailable() -> Error {
    Error::new(503, "Managed database runtime unavailable.")
}
pub fn managed_identity(access: &ManagedAccess) -> String {
    json!([
        access.scope_id,
        access.subject,
        access.connection_id,
        access.profile,
        access.revision
    ])
    .to_string()
}
impl ProcessManager {
    pub fn new(config: ProcessConfig) -> Result<Self> {
        if !config.executable.is_absolute()
            || !config.temporary_directory.is_absolute()
            || config.web_root.as_ref().is_some_and(|p| !p.is_absolute())
            || !(1..=64).contains(&config.capacity)
            || config.startup_timeout.is_zero()
            || config.startup_timeout > Duration::from_secs(60)
        {
            return Err(unavailable());
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(250))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            inner: Arc::new(Inner {
                capacity: Arc::new(Semaphore::new(config.capacity)),
                config,
                entries: Mutex::new(HashMap::new()),
                stopped: AtomicBool::new(false),
                shutdown: CancellationToken::new(),
                client,
            }),
        })
    }
    async fn cleanup(entry: &Entry) {
        entry.cancelled.cancel();
        if let Some(mut child) = entry.child.lock().await.take() {
            if let Some(id) = child
                .id()
                .and_then(|id| i32::try_from(id).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process(id, rustix::process::Signal::TERM);
            }
            if tokio::time::timeout(Duration::from_secs(1), child.wait())
                .await
                .is_err()
            {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            }
        }
        // Only the TempDir guard created for this entry can remove its storage.
        drop(entry.directory.lock().await.take());
    }
    async fn release_entry(&self, session: &str, entry: &Arc<Entry>) {
        let mut entries = self.inner.entries.lock().await;
        if entries
            .get(session)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            entries.remove(session);
        }
        drop(entries);
        self.stop_entry(entry).await;
    }
    async fn stop_entry(&self, entry: &Entry) {
        Self::cleanup(entry).await;
        // A start task can be between its cancellation check and a file write.
        // Wait for its owned result before the final sweep and releasing capacity.
        let mut ready = entry.ready.subscribe();
        let _ = tokio::time::timeout(
            self.inner.config.startup_timeout + Duration::from_secs(1),
            async {
                loop {
                    if ready.borrow().is_some() {
                        break;
                    }
                    if ready.changed().await.is_err() {
                        break;
                    }
                }
            },
        )
        .await;
        Self::cleanup(entry).await;
    }
    async fn start(&self, entry: &Entry, access: &ManagedAccess) -> Result<ManagedRuntime> {
        let started = self.start_inner(entry, access).await;
        if started.is_err() {
            Self::cleanup(entry).await;
        }
        started
    }
    async fn start_inner(&self, entry: &Entry, access: &ManagedAccess) -> Result<ManagedRuntime> {
        use tokio::io::AsyncWriteExt;
        if entry.cancelled.is_cancelled() {
            return Err(unavailable());
        }
        let directory = tempfile::Builder::new()
            .prefix("webdock-database-")
            .tempdir_in(&self.inner.config.temporary_directory)
            .map_err(|_| unavailable())?;
        let path = directory.path().to_owned();
        let ca = path.join("ca.crt");
        *entry.directory.lock().await = Some(directory);
        let db = &access.database;
        let connections = json!([{"id":access.connection_id,"name":db.database,"params":{"driver":"postgres","host":db.host,"port":db.port,"database":db.database,"username":db.username,"password":db.password,"ssl_mode":"verify-full","ssl_ca":ca,"save_in_keychain":false}}]);
        let configuration = json!({"checkForUpdates":false,"autoCheckUpdatesOnStartup":false});
        for (file, bytes) in [
            (ca, db.ca_certificate.as_bytes().to_vec()),
            (
                path.join("connections.json"),
                serde_json::to_vec(&connections).map_err(|_| unavailable())?,
            ),
            (
                path.join("config.json"),
                serde_json::to_vec(&configuration).map_err(|_| unavailable())?,
            ),
        ] {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(file)
                .await
                .map_err(|_| unavailable())?;
            file.write_all(&bytes).await.map_err(|_| unavailable())?;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| unavailable())?;
        let port = listener.local_addr().map_err(|_| unavailable())?.port();
        drop(listener);
        let runtime_origin = format!("http://127.0.0.1:{port}");
        let authentication_origin = format!("https://127.0.0.1:{port}");
        let proxy_secret = crate::random_token()?;
        if entry.cancelled.is_cancelled() {
            return Err(unavailable());
        }
        let mut command = Command::new(&self.inner.config.executable);
        command.args([
            "web",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--auth",
            "proxy",
            "--no-open",
            "--public-url",
            &authentication_origin,
            "--allowed-origin",
            &authentication_origin,
        ]);
        if let Some(root) = &self.inner.config.web_root {
            command.arg("--web-root").arg(root);
        }
        command
            .current_dir(&path)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("RUST_LOG", "off")
            .env("TABULARIS_DATA_DIR", &path)
            .env("TABULARIS_WEB_PROXY_SECRET", &proxy_secret)
            .env("TMPDIR", &path)
            .env("XDG_CONFIG_HOME", &path)
            .env("XDG_DATA_HOME", &path)
            .env("XDG_CACHE_HOME", &path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| unavailable())?;
        {
            let mut slot = entry.child.lock().await;
            if entry.cancelled.is_cancelled() {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(unavailable());
            }
            *slot = Some(child);
        }
        let deadline = tokio::time::Instant::now() + self.inner.config.startup_timeout;
        while tokio::time::Instant::now() < deadline {
            if entry.cancelled.is_cancelled() {
                return Err(unavailable());
            }
            let exited = entry
                .child
                .lock()
                .await
                .as_mut()
                .ok_or_else(unavailable)?
                .try_wait()
                .map_err(|_| unavailable())?
                .is_some();
            if exited {
                return Err(unavailable());
            }
            if self
                .inner
                .client
                .get(format!("{runtime_origin}/healthz"))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
                && !entry.cancelled.is_cancelled()
            {
                return Ok(ManagedRuntime {
                    access: RuntimeAccess {
                        scope_id: access.scope_id.clone(),
                        subject: access.subject.clone(),
                        connection_id: access.connection_id.clone(),
                        profile: access.profile,
                        runtime_origin,
                        proxy_secret,
                        expires_at: access.expires_at,
                    },
                    authentication_origin,
                });
            }
            tokio::select! {_=entry.cancelled.cancelled()=>return Err(unavailable()),_=tokio::time::sleep(Duration::from_millis(25))=>{}}
        }
        Err(Error::new(503, "Managed database runtime could not start."))
    }
}
#[async_trait::async_trait]
impl ManagedRuntimes for ProcessManager {
    async fn acquire(&self, session: &str, access: &ManagedAccess) -> Result<ManagedRuntime> {
        if self.inner.stopped.load(Ordering::SeqCst) || access.expires_at <= chrono::Utc::now() {
            return Err(unavailable());
        }
        let identity = managed_identity(access);
        let entry = {
            let mut entries = self.inner.entries.lock().await;
            if self.inner.stopped.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            if let Some(entry) = entries.get(session) {
                if entry.identity != identity {
                    return Err(Error::new(
                        401,
                        "Managed database access changed. Open a new session.",
                    ));
                }
                entry.clone()
            } else {
                let permit = self
                    .inner
                    .capacity
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| Error::new(503, "Managed database runtime capacity reached."))?;
                let duration = (access.expires_at - chrono::Utc::now())
                    .to_std()
                    .map_err(|_| unavailable())?;
                let (ready, _) = watch::channel(None);
                let entry = Arc::new(Entry {
                    identity,
                    ready,
                    cancelled: self.inner.shutdown.child_token(),
                    child: Mutex::new(None),
                    directory: Mutex::new(None),
                    _permit: permit,
                });
                entries.insert(session.into(), entry.clone());
                // Starting is owned by the entry, not an individual waiting HTTP future.
                let manager = self.clone();
                let starting = entry.clone();
                let access = access.clone();
                tokio::spawn(async move {
                    let result = manager.start(&starting, &access).await;
                    starting.ready.send_replace(Some(result));
                });
                let manager = self.clone();
                let session = session.to_owned();
                let weak = Arc::downgrade(&entry);
                let cancelled = entry.cancelled.clone();
                tokio::spawn(async move {
                    tokio::select! {_=cancelled.cancelled()=>{},_=tokio::time::sleep(duration)=>{}}
                    if let Some(entry) = weak.upgrade() {
                        manager.release_entry(&session, &entry).await;
                    }
                });
                entry
            }
        };
        if entry.cancelled.is_cancelled() {
            return Err(unavailable());
        }
        let mut ready = entry.ready.subscribe();
        let result = loop {
            if let Some(result) = ready.borrow().clone() {
                break result;
            }
            tokio::select! {_=entry.cancelled.cancelled()=>return Err(unavailable()),changed=ready.changed()=>{changed.map_err(|_|unavailable())?;}}
        };
        if result.is_err() {
            self.release_entry(session, &entry).await;
            return result;
        }
        if entry.cancelled.is_cancelled()
            || entry
                .child
                .lock()
                .await
                .as_mut()
                .ok_or_else(unavailable)?
                .try_wait()
                .map_err(|_| unavailable())?
                .is_some()
        {
            return Err(unavailable());
        }
        result
    }
    async fn release(&self, session: &str) {
        let entry = self.inner.entries.lock().await.remove(session);
        if let Some(entry) = entry {
            self.stop_entry(&entry).await;
        }
    }
    async fn shutdown(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.shutdown.cancel();
        let entries = std::mem::take(&mut *self.inner.entries.lock().await);
        futures_util::future::join_all(entries.values().map(|entry| self.stop_entry(entry))).await;
    }
}
