//! Bounded, anonymous private staging; no binary data passes through JSON Invoke.
use crate::{invalid, unavailable};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{OwnedSemaphorePermit, Semaphore},
};
use webdock_plugin_http::storage::{
    StorageBinding, StorageTarget, bind_storage, bind_storage_loopback_test, send_storage,
};
use webdock_plugin_protocol::{
    PluginError, Result,
    storage::{
        MAX_TRANSFER_BYTES, StorageDirection, StoragePlan, StorageReceipt, TRANSFER_CHUNK_BYTES,
    },
};

#[async_trait]
pub trait StorageAuthority: Send + Sync {
    async fn check(&self) -> Result<()>;
}
pub struct StorageGateway {
    root: PathBuf,
    permits: Arc<Semaphore>,
    loopback_test: bool,
}
impl StorageGateway {
    pub fn new(root: PathBuf) -> Result<Self> {
        Self::construct(root, false)
    }
    pub fn loopback_test(root: PathBuf) -> Result<Self> {
        Self::construct(root, true)
    }
    fn construct(root: PathBuf, loopback_test: bool) -> Result<Self> {
        let m = std::fs::symlink_metadata(&root).map_err(|_| unavailable())?;
        if !root.is_absolute()
            || !m.is_dir()
            || m.uid() != rustix::process::geteuid().as_raw()
            || m.mode() & 0o077 != 0
        {
            return Err(invalid("Private storage staging directory required"));
        }
        Ok(Self {
            root,
            permits: Arc::new(Semaphore::new(4)),
            loopback_test,
        })
    }
    pub fn target(&self, plan: &StoragePlan, binding: StorageBinding) -> Result<StorageTarget> {
        validate_provider_headers(plan)?;
        let bind = if self.loopback_test {
            bind_storage_loopback_test
        } else {
            bind_storage
        };
        bind(
            plan.direction.method(),
            &plan.url,
            plan.headers.clone(),
            binding,
        )
        .map_err(|_| PluginError::denied())
    }
    /// Size/hash are checked before an upload reaches the provider, or download reaches a caller.
    /// Four permits cover staging AND delivery: at most 2 GiB private disk and 64 KiB read buffers.
    pub async fn stage<S>(
        &self,
        plan: &StoragePlan,
        chunks: S,
        authority: Arc<dyn StorageAuthority>,
    ) -> Result<StagedTransfer>
    where
        S: Stream<Item = Result<Vec<u8>>> + Send,
    {
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        self.stage_with_permit(plan, chunks, authority, permit)
            .await
    }
    async fn stage_with_permit<S>(
        &self,
        plan: &StoragePlan,
        chunks: S,
        authority: Arc<dyn StorageAuthority>,
        permit: OwnedSemaphorePermit,
    ) -> Result<StagedTransfer>
    where
        S: Stream<Item = Result<Vec<u8>>> + Send,
    {
        plan.validate(
            plan.direction.as_str(),
            &plan.resource_id,
            chrono::Utc::now().timestamp(),
        )?;
        guarded(plan.expires_at, authority.as_ref(), async { Ok(()) }).await?;
        let path = self.root.join(format!("transfer-{}", uuid::Uuid::new_v4()));
        // No await between creation and unlink: cancellation cannot leave a named payload behind.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&path)
            .map_err(|_| unavailable())?;
        std::fs::remove_file(&path).map_err(|_| unavailable())?;
        let mut file = tokio::fs::File::from_std(file);
        let mut bytes = 0u64;
        let mut hash = Sha256::new();
        futures_util::pin_mut!(chunks);
        while let Some(chunk) = guarded(plan.expires_at, authority.as_ref(), async {
            chunks.next().await.transpose()
        })
        .await?
        {
            if bytes.saturating_add(chunk.len() as u64) > plan.exact_bytes
                || chunk.len() > 1024 * 1024
            {
                return Err(invalid("Storage size mismatch"));
            }
            bytes += chunk.len() as u64;
            hash.update(&chunk);
            // Never keep a second complete copy; writes use the caller's bounded chunk.
            guarded(plan.expires_at, authority.as_ref(), async {
                file.write_all(&chunk).await.map_err(|_| unavailable())
            })
            .await?;
        }
        let sha256 = hex::encode(hash.finalize());
        if bytes != plan.exact_bytes
            || bytes > MAX_TRANSFER_BYTES
            || plan.expected_sha256.as_ref().is_some_and(|s| s != &sha256)
        {
            return Err(invalid("Storage content mismatch"));
        }
        guarded(plan.expires_at, authority.as_ref(), async {
            file.rewind().await.map_err(|_| unavailable())
        })
        .await?;
        guarded(plan.expires_at, authority.as_ref(), async { Ok(()) }).await?;
        Ok(StagedTransfer {
            file,
            bytes,
            sha256,
            expires_at: plan.expires_at,
            authority,
            _permit: permit,
        })
    }
    pub async fn download(
        &self,
        plan: &StoragePlan,
        target: StorageTarget,
        authority: Arc<dyn StorageAuthority>,
    ) -> Result<StagedTransfer> {
        if plan.direction != StorageDirection::Download {
            return Err(PluginError::denied());
        }
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        let response = guarded(plan.expires_at, authority.as_ref(), async {
            send_storage(target, None, plan.exact_bytes)
                .await
                .map_err(|_| unavailable())
        })
        .await?;
        let chunks = response
            .bytes_stream()
            .map(|c| c.map(|b| b.to_vec()).map_err(|_| unavailable()));
        self.stage_with_permit(plan, chunks, authority, permit)
            .await
    }
    /// Provider commit may have occurred when a connection or authority is lost: such errors are
    /// deliberately not success receipts. Callers must reconcile before retrying a write.
    pub async fn upload(
        &self,
        plan: &StoragePlan,
        target: StorageTarget,
        staged: StagedTransfer,
    ) -> Result<StorageReceipt> {
        if plan.direction != StorageDirection::Upload
            || staged.bytes != plan.exact_bytes
            || plan.expected_sha256.as_deref() != Some(&staged.sha256)
        {
            return Err(PluginError::denied());
        }
        let receipt = StorageReceipt {
            bytes: staged.bytes,
            sha256: staged.sha256.clone(),
            etag: None,
        };
        let authority = staged.authority.clone();
        let emitted = Arc::new(AtomicU64::new(0));
        let sent = emitted.clone();
        let chunks = futures_util::stream::try_unfold(staged.into_stream(), move |mut s| {
            let sent = sent.clone();
            async move {
                let chunk = s.next_chunk().await?;
                if let Some(chunk) = &chunk {
                    sent.fetch_add(chunk.len() as u64, Ordering::SeqCst);
                }
                Ok::<_, PluginError>(chunk.map(|chunk| (chunk, s)))
            }
        });
        let body = reqwest::Body::wrap_stream(chunks);
        let mut response = guarded(plan.expires_at, authority.as_ref(), async {
            send_storage(target, Some(body), plan.exact_bytes)
                .await
                .map_err(|_| upload_uncertain())
        })
        .await
        .map_err(|_| upload_uncertain())?;
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .filter(|s| {
                s.len() <= 80
                    && s.bytes()
                        .all(|b| b.is_ascii_hexdigit() || b"\"-".contains(&b))
            })
            .map(str::to_owned);
        let mut response_bytes = 0usize;
        while let Some(chunk) = guarded(plan.expires_at, authority.as_ref(), async {
            response.chunk().await.map_err(|_| unavailable())
        })
        .await?
        {
            response_bytes = response_bytes.saturating_add(chunk.len());
            if response_bytes > 8192 {
                return Err(unavailable());
            }
        }
        guarded(plan.expires_at, authority.as_ref(), async { Ok(()) }).await?;
        if emitted.load(Ordering::SeqCst) != plan.exact_bytes {
            return Err(upload_uncertain());
        }
        Ok(StorageReceipt { etag, ..receipt })
    }
}
pub struct StagedTransfer {
    file: tokio::fs::File,
    pub bytes: u64,
    pub sha256: String,
    expires_at: i64,
    authority: Arc<dyn StorageAuthority>,
    _permit: OwnedSemaphorePermit,
}
impl StagedTransfer {
    pub fn into_stream(self) -> StagedStream {
        StagedStream {
            staged: self,
            read: 0,
        }
    }
}
pub struct StagedStream {
    staged: StagedTransfer,
    read: u64,
}
impl StagedStream {
    /// The staged file and permit live only while this bounded, revocable delivery runs.
    pub async fn deliver(mut self, sender: tokio::sync::mpsc::Sender<Result<Vec<u8>>>) {
        loop {
            let next = tokio::select! {
                biased;
                _ = sender.closed() => break,
                next = self.next_chunk() => next,
            };
            match next {
                Ok(Some(bytes)) => {
                    let result = tokio::select! {
                        biased;
                        _ = sender.closed() => break,
                        result = guarded(self.staged.expires_at, self.staged.authority.as_ref(), async {
                            sender.send(Ok(bytes)).await.map_err(|_| PluginError::denied())
                        }) => result,
                    };
                    if let Err(error) = result {
                        let _ = sender.try_send(Err(error));
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.try_send(Err(error));
                    break;
                }
            }
        }
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        guarded(
            self.staged.expires_at,
            self.staged.authority.as_ref(),
            async { Ok(()) },
        )
        .await?;
        if self.read == self.staged.bytes {
            return Ok(None);
        }
        let mut bytes = vec![0; TRANSFER_CHUNK_BYTES.min((self.staged.bytes - self.read) as usize)];
        guarded(
            self.staged.expires_at,
            self.staged.authority.as_ref(),
            async {
                self.staged
                    .file
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|_| unavailable())
            },
        )
        .await?;
        self.read += bytes.len() as u64;
        // The authority check follows the private read, before any bytes are released.
        guarded(
            self.staged.expires_at,
            self.staged.authority.as_ref(),
            async { Ok(()) },
        )
        .await?;
        Ok(Some(bytes))
    }
}
fn validate_provider_headers(plan: &StoragePlan) -> Result<()> {
    let url = url::Url::parse(&plan.url).map_err(|_| PluginError::denied())?;
    let query: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    let checksum = plan
        .expected_sha256
        .as_ref()
        .map(|hex| {
            hex::decode(hex)
                .map(|bytes| STANDARD.encode(bytes))
                .map_err(|_| PluginError::denied())
        })
        .transpose()?;
    for (name, value) in plan
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .chain(query.iter().map(|(k, v)| (k.as_ref(), v.as_ref())))
    {
        let valid = match name {
            "content-type" => value == plan.attachment.content_type,
            "x-amz-checksum-sha256" => {
                checksum.as_deref() == Some(value) && plan.direction == StorageDirection::Upload
            }
            "x-amz-content-sha256" | "X-Amz-Content-Sha256" => {
                value == "UNSIGNED-PAYLOAD" || plan.expected_sha256.as_deref() == Some(value)
            }
            "x-amz-sdk-checksum-algorithm" => value == "SHA256",
            "if-match" | "if-none-match" => query
                .get("X-Amz-SignedHeaders")
                .is_some_and(|signed| signed.split(';').any(|h| h == name)),
            _ => true,
        };
        if !valid {
            return Err(PluginError::denied());
        }
    }
    if let Some(signed) = query.get("X-Amz-SignedHeaders") {
        let mut seen = std::collections::BTreeSet::new();
        for name in signed.split(';') {
            if !seen.insert(name)
                || !["host", "content-length"].contains(&name) && !plan.headers.contains_key(name)
            {
                return Err(PluginError::denied());
            }
        }
        if !seen.contains("host") {
            return Err(PluginError::denied());
        }
    }
    Ok(())
}
async fn guarded<T, F>(expires_at: i64, authority: &dyn StorageAuthority, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    async fn check(authority: &dyn StorageAuthority) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(1), authority.check())
            .await
            .map_err(|_| PluginError::denied())?
    }
    let left = expires_at
        .saturating_mul(1000)
        .saturating_sub(chrono::Utc::now().timestamp_millis());
    if left <= 0 {
        return Err(PluginError::denied());
    }
    let operation = async {
        check(authority).await?;
        let deadline = tokio::time::sleep(Duration::from_millis(left as u64));
        tokio::pin!(deadline);
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let result = loop {
            tokio::select! {result=&mut future=>break result, _=&mut deadline=>return Err(PluginError::denied()), _=interval.tick()=>check(authority).await?}
        }?;
        check(authority).await?;
        if expires_at <= chrono::Utc::now().timestamp() {
            return Err(PluginError::denied());
        }
        Ok(result)
    };
    tokio::time::timeout(Duration::from_millis(left as u64), operation)
        .await
        .map_err(|_| PluginError::denied())?
}

pub use crate::authority::storage_admission::{StorageAccess, StorageCaller};

pub fn upload_uncertain() -> PluginError {
    PluginError::new(
        webdock_plugin_protocol::ErrorCode::Unavailable,
        "Storage upload outcome is uncertain; inspect the object before retrying",
    )
}
