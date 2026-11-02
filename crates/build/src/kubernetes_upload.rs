//! Revocable result ingress. Source GET and release publication are separate authorities.
//! The gateway owns one dedicated, kernel-size-limited tmpfs. No S3 write capability reaches a Pod.
use crate::{
    Result, error, io,
    kubernetes_transfer::{AttemptTransfer, MAX_TRANSFER_RESULT, ResultIdentity, verify_result},
};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use url::Url;

const MAX_GRANTS: usize = 256;
const MAX_FRAME: usize = 1024 * 1024;
const FILESYSTEM_HEADROOM: u64 = 1024 * 1024;

/// Deliberately no Debug/Serialize: the URL contains a bearer capability.
pub struct AttemptUploadGrant {
    url: String,
}
impl AttemptUploadGrant {
    pub fn as_str(&self) -> &str {
        &self.url
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptUploadReceipt {
    pub sha256: String,
    pub bytes: u64,
    pub identity: ResultIdentity,
}
struct Receipt {
    metadata: AttemptUploadReceipt,
    file: File,
    reservation: u64,
}
struct Grant {
    attempt: AttemptTransfer,
    identity: Option<ResultIdentity>,
    expires: Instant,
    revoked: watch::Sender<bool>,
    in_flight: bool,
    receipt: Option<Receipt>,
}
#[derive(Default)]
struct State {
    grants: BTreeMap<String, Grant>,
    active: bool,
    reserved: u64,
}

pub struct AttemptUploadGateway {
    root: PathBuf,
    directory: File,
    device: u64,
    inode: u64,
    filesystem_bytes: u64,
    storage_budget: u64,
    origin: Url,
    max_bytes: u64,
    deadline: Duration,
    state: Mutex<State>,
    #[cfg(test)]
    test_root: bool,
}
fn lock(gateway: &AttemptUploadGateway) -> Result<std::sync::MutexGuard<'_, State>> {
    gateway
        .state
        .lock()
        .map_err(|_| error("upload_gateway_unavailable"))
}
fn key(attempt: &AttemptTransfer) -> Result<String> {
    Ok(attempt.keys()?.1)
}
fn same(a: &AttemptTransfer, b: &AttemptTransfer) -> bool {
    a.customer_id == b.customer_id
        && a.project_id == b.project_id
        && a.build_id == b.build_id
        && a.generation == b.generation
        && a.request.source_sha == b.request.source_sha
        && a.request.archive_sha256 == b.request.archive_sha256
}
fn hash_token(token: &str) -> Result<String> {
    if token.len() != 64 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(error("invalid_upload_grant"));
    }
    Ok(hex::encode(Sha256::digest(token.as_bytes())))
}
impl AttemptUploadGateway {
    /// Startup is closed unless root is a private dedicated tmpfs mount with a verified hard size.
    /// All retained receipts share this physical budget, including uploads interrupted by a task drop.
    pub fn open(
        root: &Path,
        filesystem_ceiling: u64,
        origin: &str,
        max_bytes: u64,
        timeout_seconds: u64,
    ) -> Result<Arc<Self>> {
        let directory = File::open(root).map_err(io)?;
        let meta = directory.metadata().map_err(io)?;
        let origin = Url::parse(origin).map_err(|_| error("invalid_upload_origin"))?;
        // SAFETY: geteuid has no arguments or preconditions.
        let owner = unsafe { libc::geteuid() };
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
            || !root.is_absolute()
            || root.canonicalize().map_err(io)? != root
            || !meta.is_dir()
            || meta.mode() & 0o777 != 0o700
            || meta.uid() != owner
            || max_bytes == 0
            || max_bytes > MAX_TRANSFER_RESULT
            || !(1..=300).contains(&timeout_seconds)
        {
            return Err(error("invalid_upload_gateway_configuration"));
        }
        // Root must be the actual mountpoint; a subdirectory on an unrelated tmpfs is insufficient.
        let mounts = std::fs::read_to_string("/proc/self/mountinfo").map_err(io)?;
        if !mounts.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.get(4).is_some_and(|v| Some(*v) == root.to_str())
                && fields.get(3) == Some(&"/")
                && line
                    .split_once(" - ")
                    .is_some_and(|(_, fs)| fs.starts_with("tmpfs "))
        }) {
            return Err(error("upload_hard_filesystem_required"));
        }
        let capacity = tmpfs_capacity(&directory)?;
        if capacity > filesystem_ceiling
            || capacity < max_bytes.saturating_add(FILESYSTEM_HEADROOM)
            || std::fs::read_dir(root).map_err(io)?.next().is_some()
        {
            return Err(error("invalid_upload_filesystem_budget"));
        }
        Ok(Arc::new(Self {
            root: root.into(),
            directory,
            device: meta.dev(),
            inode: meta.ino(),
            filesystem_bytes: capacity,
            storage_budget: max_bytes.div_ceil(4096) * 4096,
            origin,
            max_bytes,
            deadline: Duration::from_secs(timeout_seconds),
            state: Mutex::new(State::default()),
            #[cfg(test)]
            test_root: false,
        }))
    }
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
    pub fn origin(&self) -> &str {
        self.origin.as_str()
    }
    fn check_root(&self) -> Result<()> {
        #[cfg(test)]
        if self.test_root {
            return Ok(());
        }
        let current = std::fs::symlink_metadata(&self.root).map_err(io)?;
        // SAFETY: geteuid has no arguments or preconditions.
        let owner = unsafe { libc::geteuid() };
        if current.dev() != self.device
            || current.ino() != self.inode
            || !current.is_dir()
            || current.mode() & 0o777 != 0o700
            || current.uid() != owner
            || tmpfs_capacity(&self.directory)? != self.filesystem_bytes
        {
            return Err(error("upload_filesystem_changed"));
        }
        Ok(())
    }
    /// Issue once before handoff. Grants remain unusable until trusted Job UID binding succeeds.
    pub fn issue(&self, attempt: &AttemptTransfer, ttl_seconds: u64) -> Result<AttemptUploadGrant> {
        if !(1..=1800).contains(&ttl_seconds) {
            return Err(error("invalid_upload_expiry"));
        }
        self.check_root()?;
        self.reconcile_expired()?;
        let scope = key(attempt)?;
        let mut state = lock(self)?;
        if state.grants.len() >= MAX_GRANTS
            || state
                .grants
                .values()
                .any(|g| key(&g.attempt).is_ok_and(|k| k == scope))
        {
            return Err(error("upload_grant_conflict"));
        }
        let mut token = [0; 32];
        File::open("/dev/urandom")
            .map_err(io)?
            .read_exact(&mut token)
            .map_err(io)?;
        let token = hex::encode(token);
        let mut url = self
            .origin
            .join(&scope)
            .map_err(|_| error("invalid_upload_origin"))?;
        url.set_query(Some(&format!("grant={token}")));
        let (revoked, _) = watch::channel(false);
        state.grants.insert(
            hash_token(&token)?,
            Grant {
                attempt: attempt.clone(),
                identity: None,
                expires: Instant::now() + Duration::from_secs(ttl_seconds),
                revoked,
                in_flight: false,
                receipt: None,
            },
        );
        Ok(AttemptUploadGrant {
            url: url.to_string(),
        })
    }
    /// Call periodically even when no new requests arrive. Only exact owned unnamed files are closed.
    pub fn reconcile_expired(&self) -> Result<()> {
        let mut state = lock(self)?;
        let now = Instant::now();
        let mut released = 0;
        state.grants.retain(|_, grant| {
            if now < grant.expires {
                return true;
            }
            grant.revoked.send_replace(true);
            if let Some(receipt) = grant.receipt.take() {
                released += receipt.reservation;
                drop(receipt);
            }
            grant.in_flight
        });
        state.reserved -= released;
        Ok(())
    }
    pub fn bind(&self, attempt: &AttemptTransfer, job_uid: &str) -> Result<()> {
        self.check_root()?;
        let expected = ResultIdentity::for_attempt(attempt, job_uid)?;
        let mut state = lock(self)?;
        let grant = state
            .grants
            .values_mut()
            .find(|g| same(&g.attempt, attempt))
            .ok_or_else(|| error("upload_grant_unavailable"))?;
        if *grant.revoked.borrow() || Instant::now() >= grant.expires {
            return Err(error("upload_revoked"));
        }
        if grant.identity.as_ref().is_some_and(|i| i != &expected) {
            return Err(error("upload_job_conflict"));
        }
        grant.identity = Some(expected);
        Ok(())
    }
    /// This mutex is also the final receipt commit fence: revocation and commit have one order.
    pub fn revoke(&self, attempt: &AttemptTransfer) -> Result<()> {
        self.revoke_scope(
            &attempt.customer_id,
            &attempt.project_id,
            &attempt.build_id,
            attempt.generation,
        )
    }
    /// Cancellation must not depend on a surviving/parseable handoff file or source credentials.
    pub fn revoke_scope(
        &self,
        customer_id: &str,
        project_id: &str,
        build_id: &str,
        generation: u64,
    ) -> Result<()> {
        let mut state = lock(self)?;
        let mut released = 0;
        for grant in state.grants.values_mut().filter(|g| {
            g.attempt.customer_id == customer_id
                && g.attempt.project_id == project_id
                && g.attempt.build_id == build_id
                && g.attempt.generation == generation
        }) {
            grant.revoked.send_replace(true);
            if let Some(receipt) = grant.receipt.take() {
                released += receipt.reservation;
                drop(receipt); // close the unlinked file before returning its storage reservation
            }
        }
        state.reserved -= released;
        Ok(())
    }
    /// Retire exact scope after revocation; a token cannot become valid again on a later generation.
    pub fn retire(&self, attempt: &AttemptTransfer) -> Result<()> {
        self.retire_scope(
            &attempt.customer_id,
            &attempt.project_id,
            &attempt.build_id,
            attempt.generation,
        )
    }
    pub fn retire_scope(
        &self,
        customer_id: &str,
        project_id: &str,
        build_id: &str,
        generation: u64,
    ) -> Result<()> {
        self.revoke_scope(customer_id, project_id, build_id, generation)?;
        let mut state = lock(self)?;
        state.grants.retain(|_, g| {
            g.in_flight
                || g.attempt.customer_id != customer_id
                || g.attempt.project_id != project_id
                || g.attempt.build_id != build_id
                || g.attempt.generation != generation
        });
        Ok(())
    }
    pub fn read(&self, attempt: &AttemptTransfer, job_uid: &str) -> Result<Vec<u8>> {
        self.check_root()?;
        let expected = ResultIdentity::for_attempt(attempt, job_uid)?;
        let mut state = lock(self)?;
        let grant = state
            .grants
            .values_mut()
            .find(|g| same(&g.attempt, attempt))
            .ok_or_else(|| error("upload_grant_unavailable"))?;
        if *grant.revoked.borrow() || Instant::now() >= grant.expires {
            return Err(error("upload_revoked"));
        }
        let receipt = grant
            .receipt
            .as_mut()
            .filter(|r| r.metadata.identity == expected)
            .ok_or_else(|| error("upload_receipt_unavailable"))?;
        receipt.file.seek(SeekFrom::Start(0)).map_err(io)?;
        let mut bytes = Vec::new();
        (&receipt.file)
            .take(self.max_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() as u64 != receipt.metadata.bytes
            || hex::encode(Sha256::digest(&bytes)) != receipt.metadata.sha256
        {
            return Err(error("upload_receipt_corrupt"));
        }
        if Instant::now() >= grant.expires {
            return Err(error("upload_revoked"));
        }
        self.check_root()?;
        Ok(bytes)
    }
    /// Body length is only an early rejection hint; every streamed frame is checked before write.
    pub async fn upload<S, B>(
        self: &Arc<Self>,
        request_url: &str,
        content_length: Option<u64>,
        chunks: S,
    ) -> Result<AttemptUploadReceipt>
    where
        S: Stream<Item = Result<B>> + Send,
        B: AsRef<[u8]> + Send,
    {
        self.check_root()?;
        let url = self
            .origin
            .join(request_url)
            .map_err(|_| error("invalid_upload_grant"))?;
        if url.origin() != self.origin.origin() || url.fragment().is_some() {
            return Err(error("invalid_upload_grant"));
        }
        let query: Vec<_> = url.query_pairs().collect();
        if query.len() != 1 || query[0].0 != "grant" {
            return Err(error("invalid_upload_grant"));
        }
        let token_hash = hash_token(&query[0].1)?;
        let (mut reservation, attempt, identity, expires, mut revoked, prior) = {
            let mut state = lock(self)?;
            let grant = state
                .grants
                .get(&token_hash)
                .ok_or_else(|| error("invalid_upload_grant"))?;
            if url.path() != format!("/{}", key(&grant.attempt)?) {
                return Err(error("upload_scope_conflict"));
            }
            if *grant.revoked.borrow() || Instant::now() >= grant.expires {
                return Err(error("upload_revoked"));
            }
            let identity = grant
                .identity
                .clone()
                .ok_or_else(|| error("upload_job_unbound"))?;
            if content_length.is_some_and(|n| n > self.max_bytes) {
                return Err(error("result_transfer_limit"));
            }
            if state.active || grant.in_flight {
                return Err(error("upload_in_flight_conflict"));
            }
            let prior = grant.receipt.as_ref().map(|r| r.metadata.clone());
            let bytes = if prior.is_none() {
                self.max_bytes.div_ceil(4096) * 4096
            } else {
                0
            };
            if state.reserved.saturating_add(bytes) > self.storage_budget {
                return Err(error("upload_storage_limit"));
            }
            let attempt = grant.attempt.clone();
            let expires = grant.expires;
            let revoked = grant.revoked.subscribe();
            state.active = true;
            state.reserved += bytes;
            state.grants.get_mut(&token_hash).unwrap().in_flight = true;
            (
                UploadReservation {
                    gateway: self.clone(),
                    token_hash: token_hash.clone(),
                    bytes,
                    file: None,
                },
                attempt,
                identity,
                expires,
                revoked,
                prior,
            )
        };
        // Reservation is already held before opening storage or polling even the first body frame.
        if prior.is_none() {
            reservation.file = Some(self.temporary_file()?);
        }
        let deadline = tokio::time::Instant::from_std(expires.min(Instant::now() + self.deadline));
        let stream_revoked = revoked.clone();
        let work = async {
            let mut count = 0u64;
            let mut hash = Sha256::new();
            futures_util::pin_mut!(chunks);
            while let Some(chunk) = chunks.next().await {
                // Ready/empty hostile streams must not starve revocation or the transfer deadline.
                tokio::task::yield_now().await;
                if *stream_revoked.borrow() || Instant::now() >= expires {
                    return Err(error("upload_revoked"));
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(error("upload_deadline"));
                }
                let frame = chunk?;
                let chunk = frame.as_ref();
                count = count
                    .checked_add(chunk.len() as u64)
                    .filter(|n| *n <= self.max_bytes)
                    .ok_or_else(|| error("result_transfer_limit"))?;
                if chunk.len() > MAX_FRAME {
                    return Err(error("result_transfer_limit"));
                }
                hash.update(chunk);
                if let Some(file) = &mut reservation.file {
                    file.write_all(chunk).map_err(io)?;
                }
            }
            if content_length.is_some_and(|n| n != count) {
                return Err(error("upload_length_mismatch"));
            }
            let metadata = AttemptUploadReceipt {
                sha256: hex::encode(hash.finalize()),
                bytes: count,
                identity,
            };
            if let Some(prior) = &prior {
                if prior != &metadata {
                    return Err(error("upload_receipt_conflict"));
                }
            } else {
                let file = reservation.file.as_mut().unwrap();
                file.seek(SeekFrom::Start(0)).map_err(io)?;
                let mut bytes = Vec::new();
                (&*file)
                    .take(self.max_bytes + 1)
                    .read_to_end(&mut bytes)
                    .map_err(io)?;
                // Failed customer builds still carry a complete identity-validated receipt and bounded logs.
                match verify_result(&attempt, &metadata.identity.job_uid, &bytes) {
                    Ok(_) => {}
                    Err(e) if e.code == "execution_failed" => {}
                    Err(e) => return Err(e),
                }
                drop(bytes);
            }
            self.check_root()?;
            let mut state = lock(self)?;
            let grant = state
                .grants
                .get_mut(&token_hash)
                .ok_or_else(|| error("upload_revoked"))?;
            if *grant.revoked.borrow()
                || Instant::now() >= expires
                || tokio::time::Instant::now() >= deadline
            {
                return Err(error("upload_revoked"));
            }
            if grant.identity.as_ref() != Some(&metadata.identity) {
                return Err(error("upload_job_conflict"));
            }
            if prior.is_none() {
                grant.receipt = Some(Receipt {
                    metadata: metadata.clone(),
                    file: reservation.file.take().unwrap(),
                    reservation: reservation.bytes,
                });
                reservation.bytes = 0;
            }
            Ok(metadata)
        };
        tokio::select! {
            biased;
            _ = revoked.changed() => Err(error("upload_revoked")),
            _ = tokio::time::sleep_until(deadline) => Err(error("upload_deadline")),
            result = work => result,
        }
    }
    fn temporary_file(&self) -> Result<File> {
        // O_TMPFILE has no pathname to orphan; closing its last fd is exact synchronous cleanup.
        // SAFETY: the pinned directory fd is live; the NUL-terminated name and flags are valid.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                c".".as_ptr(),
                libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(error("upload_storage_unavailable"));
        }
        // SAFETY: openat returned a fresh owned fd above; File becomes its sole owner.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(test)]
    fn usage(&self) -> (bool, u64) {
        let s = self.state.lock().unwrap();
        (s.active, s.reserved)
    }
}
fn tmpfs_capacity(file: &File) -> Result<u64> {
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: file is live and stat points to sufficient writable storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(error("upload_filesystem_unavailable"));
    }
    // SAFETY: successful fstatfs initialized every field used below.
    let stat = unsafe { stat.assume_init() };
    if i128::from(stat.f_type) != i128::from(libc::TMPFS_MAGIC) {
        return Err(error("upload_hard_filesystem_required"));
    }
    stat.f_blocks
        .checked_mul(stat.f_bsize as u64)
        .ok_or_else(|| error("invalid_upload_filesystem_budget"))
}
struct UploadReservation {
    gateway: Arc<AttemptUploadGateway>,
    token_hash: String,
    bytes: u64,
    file: Option<File>,
}
impl Drop for UploadReservation {
    fn drop(&mut self) {
        drop(self.file.take());
        // Poisoning permanently closes the gateway instead of accidentally releasing authority.
        if let Ok(mut state) = self.gateway.state.lock() {
            state.active = false;
            state.reserved -= self.bytes;
            if let Some(grant) = state.grants.get_mut(&self.token_hash) {
                grant.in_flight = false;
            }
        }
    }
}

/// Mount this router on its own isolated listener; never merge it with Core's authenticated API.
#[cfg(feature = "service")]
pub fn router(gateway: Arc<AttemptUploadGateway>) -> axum::Router {
    use axum::{
        extract::{Request, State},
        http::StatusCode,
        routing::put,
    };
    async fn upload(
        State(gateway): State<Arc<AttemptUploadGateway>>,
        request: Request,
    ) -> (StatusCode, &'static str) {
        let content_length = match request.headers().get(axum::http::header::CONTENT_LENGTH) {
            Some(value) => match value.to_str().ok().and_then(|v| v.parse().ok()) {
                Some(n) => Some(n),
                None => return (StatusCode::BAD_REQUEST, "invalid_upload_length"),
            },
            None => None,
        };
        if request
            .headers()
            .contains_key(axum::http::header::CONTENT_ENCODING)
        {
            return (StatusCode::BAD_REQUEST, "invalid_upload_encoding");
        }
        let uri = request.uri().to_string();
        let chunks = request
            .into_body()
            .into_data_stream()
            .map(|c| c.map_err(|_| error("upload_body_failed")));
        match gateway.upload(&uri, content_length, chunks).await {
            Ok(_) => (StatusCode::OK, "uploaded"),
            Err(e) => (
                match e.code {
                    "result_transfer_limit" => StatusCode::PAYLOAD_TOO_LARGE,
                    "upload_in_flight_conflict" | "upload_receipt_conflict" => StatusCode::CONFLICT,
                    "upload_deadline" => StatusCode::REQUEST_TIMEOUT,
                    _ => StatusCode::FORBIDDEN,
                },
                e.code,
            ),
        }
    }
    axum::Router::new()
        .route("/{*path}", put(upload))
        .with_state(gateway)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    fn fixture(
        max_bytes: u64,
    ) -> (
        Arc<AttemptUploadGateway>,
        AttemptTransfer,
        tempfile::TempDir,
    ) {
        // Exercise real O_TMPFILE storage; container overlay filesystems need not support it.
        let root = tempfile::tempdir_in("/dev/shm").unwrap();
        let directory = File::open(root.path()).unwrap();
        assert!(tmpfs_capacity(&directory).is_ok());
        let meta = directory.metadata().unwrap();
        let gate = Arc::new(AttemptUploadGateway {
            root: root.path().into(),
            directory,
            device: meta.dev(),
            inode: meta.ino(),
            filesystem_bytes: max_bytes * 4,
            storage_budget: max_bytes.div_ceil(4096) * 4096,
            origin: Url::parse("https://upload.example").unwrap(),
            max_bytes,
            deadline: Duration::from_secs(1),
            state: Mutex::new(State::default()),
            test_root: true,
        });
        (
            gate,
            crate::kubernetes_transfer::tests::attempt(
                "https://upload.example",
                &crate::kubernetes_transfer::tests::source(),
            ),
            root,
        )
    }
    #[test]
    fn ordinary_disk_root_is_never_a_hard_upload_budget() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            AttemptUploadGateway::open(root.path(), 1024 * 1024, "https://upload.example", 4096, 5)
                .is_err()
        );
    }
    #[tokio::test]
    async fn overflow_without_length_never_writes_past_grant_and_releases_reservations() {
        let (gate, attempt, _root) = fixture(4096);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let chunks = stream::iter(vec![Ok(vec![1; 4096]), Ok(vec![2; 1])]);
        let err = gate.upload(url.as_str(), None, chunks).await.unwrap_err();
        assert_eq!(err.code, "result_transfer_limit");
        assert_eq!(gate.usage(), (false, 0));
    }
    #[tokio::test]
    async fn revoked_and_unbound_capabilities_never_poll_a_body() {
        let (gate, attempt, _root) = fixture(4096);
        let url = gate.issue(&attempt, 60).unwrap();
        let body = stream::poll_fn(|_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
            panic!("must reject before reading")
        });
        assert_eq!(
            gate.upload(url.as_str(), None, body)
                .await
                .unwrap_err()
                .code,
            "upload_job_unbound"
        );
        gate.revoke(&attempt).unwrap();
        let body = stream::poll_fn(|_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
            panic!("must reject before reading")
        });
        assert_eq!(
            gate.upload(url.as_str(), None, body)
                .await
                .unwrap_err()
                .code,
            "upload_revoked"
        );
        assert!(gate.bind(&attempt, "job-uid").is_err());
    }
    fn bundle(attempt: &AttemptTransfer, uid: &str, log: &str) -> Vec<u8> {
        crate::artifact::Archive::from_files(BTreeMap::from([
            (
                "attempt.json".into(),
                serde_json::to_vec(&ResultIdentity::for_attempt(attempt, uid).unwrap()).unwrap(),
            ),
            ("status.json".into(), br#"{"success":false}"#.to_vec()),
            ("build.log".into(), log.as_bytes().to_vec()),
            ("events.jsonl".into(), vec![]),
        ]))
        .unwrap()
        .encode()
        .unwrap()
    }
    #[tokio::test]
    async fn one_immutable_receipt_replays_only_identical_bytes_and_retirement_closes_storage() {
        let (gate, attempt, root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let bytes = bundle(&attempt, "job-uid", "first");
        let receipt = gate
            .upload(url.as_str(), None, stream::iter([Ok(bytes.clone())]))
            .await
            .unwrap();
        assert_eq!(gate.read(&attempt, "job-uid").unwrap(), bytes);
        assert_eq!(gate.usage(), (false, 16 * 1024));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        let mut second = attempt.clone();
        second.customer_id = "second-tenant".into();
        let second_url = gate.issue(&second, 60).unwrap();
        gate.bind(&second, "second-job").unwrap();
        let unread = stream::poll_fn(|_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
            panic!("storage not reserved")
        });
        assert_eq!(
            gate.upload(second_url.as_str(), None, unread)
                .await
                .unwrap_err()
                .code,
            "upload_storage_limit"
        );
        let replay = gate
            .upload(
                url.as_str(),
                Some(bytes.len() as u64),
                stream::iter([Ok(bytes.clone())]),
            )
            .await
            .unwrap();
        assert_eq!(receipt, replay);
        let different = bundle(&attempt, "job-uid", "other");
        assert_eq!(
            gate.upload(url.as_str(), None, stream::iter([Ok(different)]))
                .await
                .unwrap_err()
                .code,
            "upload_receipt_conflict"
        );
        assert_eq!(gate.read(&attempt, "job-uid").unwrap(), bytes);
        gate.retire(&attempt).unwrap();
        assert_eq!(gate.usage(), (false, 0));
        assert!(gate.read(&attempt, "job-uid").is_err());
        assert!(
            gate.upload(url.as_str(), None, stream::iter([Ok(bytes)]))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn false_lengths_and_foreign_scopes_never_create_receipts() {
        let (gate, attempt, _root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let bytes = bundle(&attempt, "job-uid", "");
        assert_eq!(
            gate.upload(url.as_str(), Some(1), stream::iter([Ok(bytes.clone())]))
                .await
                .unwrap_err()
                .code,
            "upload_length_mismatch"
        );
        let foreign = url.as_str().replace("/customer/", "/another/");
        assert_eq!(
            gate.upload(&foreign, None, stream::iter([Ok(bytes.clone())]))
                .await
                .unwrap_err()
                .code,
            "upload_scope_conflict"
        );
        let foreign = bundle(&attempt, "another-job", "");
        assert_eq!(
            gate.upload(url.as_str(), None, stream::iter([Ok(foreign)]))
                .await
                .unwrap_err()
                .code,
            "transfer_identity_mismatch"
        );
        assert!(gate.bind(&attempt, "another-job").is_err());
        assert!(gate.read(&attempt, "job-uid").is_err());
        assert_eq!(gate.usage(), (false, 0));
    }
    #[tokio::test]
    async fn concurrent_grant_and_other_tenant_are_rejected_before_body_poll_and_drop_cleans_up() {
        let (gate, attempt, root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let mut foreign = attempt.clone();
        foreign.customer_id = "another".into();
        let other = gate.issue(&foreign, 60).unwrap();
        gate.bind(&foreign, "another-job").unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let mut started = Some(started);
        let chunks = stream::poll_fn(move |_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
            if let Some(started) = started.take() {
                let _ = started.send(());
            }
            std::task::Poll::Pending
        });
        let task_gate = gate.clone();
        let task_url = url.as_str().to_owned();
        let running = tokio::spawn(async move { task_gate.upload(&task_url, None, chunks).await });
        ready.await.unwrap();
        for url in [url.as_str(), other.as_str()] {
            let unread = stream::poll_fn(|_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
                panic!("unreserved body polled")
            });
            assert_eq!(
                gate.upload(url, None, unread).await.unwrap_err().code,
                "upload_in_flight_conflict"
            );
        }
        running.abort();
        let _ = running.await;
        assert_eq!(gate.usage(), (false, 0));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn cancellation_mid_body_wakes_pending_reader_and_final_commit_is_fenced() {
        let (gate, attempt, _root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let mut started = Some(started);
        let chunks = stream::poll_fn(move |_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
            if let Some(started) = started.take() {
                let _ = started.send(());
                return std::task::Poll::Ready(Some(Ok(vec![0; 512])));
            }
            std::task::Poll::Pending
        });
        let task_gate = gate.clone();
        let task_url = url.as_str().to_owned();
        let running = tokio::spawn(async move { task_gate.upload(&task_url, None, chunks).await });
        ready.await.unwrap();
        gate.revoke(&attempt).unwrap();
        assert_eq!(running.await.unwrap().unwrap_err().code, "upload_revoked");
        assert_eq!(gate.usage(), (false, 0));
        let (gate, attempt, _root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let mut bytes = Some(bundle(&attempt, "job-uid", "complete"));
        let body_gate = gate.clone();
        let body_attempt = attempt.clone();
        let chunks = stream::poll_fn(move |_| {
            if let Some(bytes) = bytes.take() {
                return std::task::Poll::Ready(Some(Ok(bytes)));
            }
            // Revocation wins the race precisely after EOF and before the receipt commit fence.
            body_gate.revoke(&body_attempt).unwrap();
            std::task::Poll::Ready(None)
        });
        assert_eq!(
            gate.upload(url.as_str(), None, chunks)
                .await
                .unwrap_err()
                .code,
            "upload_revoked"
        );
        assert!(gate.read(&attempt, "job-uid").is_err());
        assert_eq!(gate.usage(), (false, 0));
    }
    #[tokio::test]
    async fn deadlines_expiry_and_oversized_lengths_release_storage_without_secret_errors() {
        let (gate, attempt, _root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let unread = stream::poll_fn(|_| -> std::task::Poll<Option<Result<Vec<u8>>>> {
            panic!("oversized header body polled")
        });
        let err = gate
            .upload(url.as_str(), Some(16 * 1024 + 1), unread)
            .await
            .unwrap_err();
        assert_eq!(err.code, "result_transfer_limit");
        assert!(err.logs.is_empty());
        assert!(!err.to_string().contains("grant="));
        let chunks = stream::pending::<Result<Vec<u8>>>();
        assert_eq!(
            gate.upload(url.as_str(), None, chunks)
                .await
                .unwrap_err()
                .code,
            "upload_deadline"
        );
        assert_eq!(gate.usage(), (false, 0));
        let bytes = bundle(&attempt, "job-uid", "");
        gate.upload(url.as_str(), None, stream::iter([Ok(bytes)]))
            .await
            .unwrap();
        for grant in gate.state.lock().unwrap().grants.values_mut() {
            grant.expires = Instant::now();
        }
        gate.reconcile_expired().unwrap();
        assert_eq!(gate.usage(), (false, 0));
        assert!(gate.read(&attempt, "job-uid").is_err());
    }
    #[cfg(feature = "service")]
    #[tokio::test]
    async fn isolated_http_route_streams_chunked_overflow_and_never_exposes_other_core_routes() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let (gate, attempt, _root) = fixture(4096);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let parsed = Url::parse(url.as_str()).unwrap();
        let uri = format!("{}?{}", parsed.path(), parsed.query().unwrap());
        let body = Body::from_stream(stream::iter([
            Ok::<_, std::io::Error>(vec![0; 4096]),
            Ok(vec![0; 1]),
        ]));
        let response = router(gate.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(&uri)
                    .header("transfer-encoding", "chunked")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(gate.usage(), (false, 0));
        let body = Body::from_stream(stream::iter([Ok::<_, std::io::Error>(vec![0; 512])]));
        let response = router(gate.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(&uri)
                    .header("content-length", "1")
                    .body(body)
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(gate.usage(), (false, 0));
        let response = router(gate)
            .oneshot(
                Request::builder()
                    .uri("/api/operator/builds")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
    #[tokio::test]
    async fn endless_ready_empty_frames_cannot_starve_the_deadline() {
        let (mut gate, attempt, _root) = fixture(4096);
        Arc::get_mut(&mut gate).unwrap().deadline = Duration::from_millis(10);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let chunks = stream::repeat_with(|| Ok(Vec::<u8>::new()));
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            gate.upload(url.as_str(), None, chunks),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err().code, "upload_deadline");
        assert_eq!(gate.usage(), (false, 0));
    }
    #[tokio::test]
    async fn scope_only_revocation_survives_lost_handoff_and_never_retires_a_foreign_tenant() {
        let (gate, attempt, _root) = fixture(16 * 1024);
        let url = gate.issue(&attempt, 60).unwrap();
        gate.bind(&attempt, "job-uid").unwrap();
        let mut foreign = attempt.clone();
        foreign.customer_id = "other-customer".into();
        let foreign_url = gate.issue(&foreign, 60).unwrap();
        gate.bind(&foreign, "foreign-job").unwrap();
        // Only durable queue scope is available here; no AttemptTransfer is loaded to cancel.
        gate.revoke_scope("customer", "project", "build", 1)
            .unwrap();
        assert_eq!(
            gate.upload(
                url.as_str(),
                None,
                stream::iter([Ok(bundle(&attempt, "job-uid", ""))])
            )
            .await
            .unwrap_err()
            .code,
            "upload_revoked"
        );
        let bytes = bundle(&foreign, "foreign-job", "");
        gate.upload(
            foreign_url.as_str(),
            None,
            stream::iter([Ok(bytes.clone())]),
        )
        .await
        .unwrap();
        gate.retire_scope("customer", "project", "build", 1)
            .unwrap();
        assert_eq!(gate.read(&foreign, "foreign-job").unwrap(), bytes);
        gate.retire_scope("other-customer", "project", "build", 1)
            .unwrap();
        assert_eq!(gate.usage(), (false, 0));
    }
}
