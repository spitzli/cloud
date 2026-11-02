//! Independent operator cold recovery. Standard OpenSSH signatures, never a browser session.
//! A capsule grants data restoration to one quarantined target, never sending or cutover.
use crate::config::{private_directory, read_private};
use base64::{Engine, engine::general_purpose::STANDARD};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use webdock_supervisor::{ExecutionError, Result};

const SET_NAMESPACE: &str = "webdock-recovery-set-v1";
const CAPSULE_NAMESPACE: &str = "webdock-recovery-capsule-v1";
pub use webdock_agent_protocol::recovery::custody::{
    AdmittedRecoverySet, ColdRecoveryCapsule, SignedDocument,
};
#[derive(Clone, Debug)]
pub struct VerifiedColdRecovery {
    pub(crate) set: AdmittedRecoverySet,
    pub(crate) capsule_sha256: String,
    pub(crate) nonce: String,
    pub(crate) actor_id: String,
    pub(crate) expires_at: i64,
}

fn safe_identity(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-.@:".contains(&c))
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn private(path: &Path) -> Result<Vec<u8>> {
    read_private(path, 262144).map_err(|_| ExecutionError::Ownership)
}
fn root(path: &Path) -> Result<()> {
    private_directory(path).map_err(|_| ExecutionError::Ownership)
}
fn sync_parent(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| ExecutionError::Runtime)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| ExecutionError::Conflict)?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|_| ExecutionError::Runtime)?;
    sync_parent(path.parent().ok_or(ExecutionError::Ownership)?)
}
fn run(mut command: Command, body: Option<&[u8]>) -> Result<()> {
    command
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.env("SSH_ASKPASS_REQUIRE", "never");
    let mut process = command.spawn().map_err(|_| ExecutionError::Runtime)?;
    if let Some(bytes) = body
        && let Some(mut input) = process.stdin.take()
        && input.write_all(bytes).is_err()
    {
        let _ = process.kill();
        let _ = process.wait();
        return Err(ExecutionError::Runtime);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = process.try_wait().map_err(|_| ExecutionError::Runtime)? {
            return if status.success() {
                Ok(())
            } else {
                Err(ExecutionError::Denied)
            };
        }
        if Instant::now() >= deadline {
            let _ = process.kill();
            let _ = process.wait();
            return Err(ExecutionError::Expired);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// Called by Core only after the referenced set's admission transaction committed.
/// Signer is an operator-installed independent custody path, never an API field.
pub fn seal_admitted_set(
    set: &AdmittedRecoverySet,
    signer: &Path,
    staging: &Path,
) -> Result<SignedDocument> {
    sign(set, signer, staging, SET_NAMESPACE)
}
/// Actual operator custody signs a new target/deadline, using the already admitted set.
pub fn seal_operator_capsule(
    capsule: &ColdRecoveryCapsule,
    signer: &Path,
    staging: &Path,
) -> Result<SignedDocument> {
    sign(capsule, signer, staging, CAPSULE_NAMESPACE)
}
fn sign<T: Serialize>(
    value: &T,
    signer: &Path,
    staging: &Path,
    namespace: &str,
) -> Result<SignedDocument> {
    root(staging)?;
    private(signer)?;
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ExecutionError::Invalid("recovery signature document"))?;
    if bytes.len() > 131072 {
        return Err(ExecutionError::Invalid("recovery signature limit"));
    }
    let temporary = tempfile::Builder::new()
        .prefix("sign-")
        .tempdir_in(staging)
        .map_err(|_| ExecutionError::Runtime)?;
    let input = temporary.path().join("document");
    write_new(&input, &bytes)?;
    let mut command = Command::new("/usr/bin/ssh-keygen");
    command
        .args(["-Y", "sign", "-q", "-f"])
        .arg(signer)
        .args(["-n", namespace])
        .arg(&input);
    run(command, None)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        input.with_extension("sig"),
        std::fs::Permissions::from_mode(0o600),
    )
    .map_err(|_| ExecutionError::Runtime)?;
    let signature = private(&input.with_extension("sig"))?;
    Ok(SignedDocument {
        payload: STANDARD.encode(bytes),
        signature: STANDARD.encode(signature),
    })
}
pub(crate) fn verify<T: serde::de::DeserializeOwned>(
    doc: &SignedDocument,
    allowed_signers: &Path,
    identity: &str,
    staging: &Path,
    namespace: &str,
) -> Result<T> {
    if !safe_identity(identity) || doc.payload.len() > 262144 || doc.signature.len() > 16384 {
        return Err(ExecutionError::Denied);
    }
    root(staging)?;
    private(allowed_signers)?;
    let body = STANDARD
        .decode(&doc.payload)
        .map_err(|_| ExecutionError::Denied)?;
    let signature = STANDARD
        .decode(&doc.signature)
        .map_err(|_| ExecutionError::Denied)?;
    if STANDARD.encode(&body) != doc.payload || STANDARD.encode(&signature) != doc.signature {
        return Err(ExecutionError::Denied);
    }
    let temporary = tempfile::Builder::new()
        .prefix("verify-")
        .tempdir_in(staging)
        .map_err(|_| ExecutionError::Runtime)?;
    let input = temporary.path().join("signature");
    write_new(&input, &signature)?;
    let mut command = Command::new("/usr/bin/ssh-keygen");
    command
        .args(["-Y", "verify", "-q", "-f"])
        .arg(allowed_signers)
        .args(["-I", identity, "-n", namespace, "-s"])
        .arg(input);
    run(command, Some(&body))?;
    serde_json::from_slice(&body).map_err(|_| ExecutionError::Invalid("recovery signed document"))
}
/// The target profile/identity and trust files come from operator-owned native config.
/// Later source revocation/history cannot be inferred offline; quarantine is mandatory.
#[expect(
    clippy::too_many_arguments,
    reason = "Separate issuer/operator trust and destination/nonce/time/capacity bindings are explicit security inputs."
)]
pub fn verify_capsule(
    document: &SignedDocument,
    core_signers: &Path,
    operator_signers: &Path,
    target_profile: &str,
    target_sha256: &str,
    target_capacity: u64,
    staging: &Path,
    now: i64,
) -> Result<VerifiedColdRecovery> {
    if document.payload.len() > 262144 || document.signature.len() > 16384 {
        return Err(ExecutionError::Invalid("cold capsule size"));
    }
    // The untrusted identity selects only an entry in the fixed allowlist; the signature
    // must verify as that exact independently enrolled operator before it is used.
    let body = STANDARD
        .decode(&document.payload)
        .map_err(|_| ExecutionError::Denied)?;
    if body.len() > 131072 {
        return Err(ExecutionError::Invalid("cold capsule size"));
    }
    let untrusted: ColdRecoveryCapsule =
        serde_json::from_slice(&body).map_err(|_| ExecutionError::Denied)?;
    let capsule: ColdRecoveryCapsule = verify(
        document,
        operator_signers,
        &untrusted.actor_id,
        staging,
        CAPSULE_NAMESPACE,
    )?;
    let set: AdmittedRecoverySet = verify(
        &capsule.admitted_set,
        core_signers,
        "webdock-core-recovery",
        staging,
        SET_NAMESPACE,
    )?;
    if capsule.version != 1
        || set.version != 1
        || !capsule.quarantine_only
        || capsule.target_profile_id != target_profile
        || capsule.target_identity_sha256 != target_sha256
        || capsule.capacity_bytes != target_capacity
        || set.capacity_bytes > target_capacity
        || capsule.issued_at < 0
        || capsule.issued_at > now
        || capsule.expires_at <= now
        || capsule
            .issued_at
            .checked_add(300)
            .is_none_or(|max| capsule.expires_at > max)
        || !(67108864..=64 * 1024 * 1024 * 1024).contains(&target_capacity)
        || set.capacity_bytes < 67108864
        || set.completed_at > capsule.issued_at
        || set.proof["version"] != 1
        || set.proof["objects"]
            .as_array()
            .is_none_or(|objects| objects.is_empty())
        || !uuid::Uuid::parse_str(&capsule.target_nonce)
            .is_ok_and(|n| n.to_string() == capsule.target_nonce && !n.is_nil())
        || ![&set.set_id, &set.policy_id, &set.operation_id]
            .into_iter()
            .all(|id| {
                !id.starts_with('0')
                    && id.len() <= 19
                    && id.bytes().all(|c| c.is_ascii_digit())
                    && id.parse::<i64>().is_ok_and(|n| n > 0)
            })
    {
        return Err(ExecutionError::Denied);
    }
    Ok(VerifiedColdRecovery {
        set,
        capsule_sha256: sha(&body),
        nonce: capsule.target_nonce,
        actor_id: capsule.actor_id,
        expires_at: capsule.expires_at,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplayRecord {
    version: u8,
    capsule_sha256: String,
    nonce: String,
    state: String,
    receipt_sha256: Option<String>,
}
/// Local durable replay custody. It makes no globally-single-use claim for disconnected nodes.
pub struct ReplayJournal {
    directory: PathBuf,
    _lock: File,
}
impl ReplayJournal {
    pub fn open(directory: &Path) -> Result<Self> {
        root(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(directory.join("lock"))
            .map_err(|_| ExecutionError::Runtime)?;
        private(&directory.join("lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| ExecutionError::Conflict)?;
        Ok(Self {
            directory: directory.into(),
            _lock: lock,
        })
    }
    pub fn begin(&self, verified: &VerifiedColdRecovery) -> Result<()> {
        let record = ReplayRecord {
            version: 1,
            capsule_sha256: verified.capsule_sha256.clone(),
            nonce: verified.nonce.clone(),
            state: "pending".into(),
            receipt_sha256: None,
        };
        write_new(
            &self.directory.join(format!("{}.json", verified.nonce)),
            &serde_json::to_vec(&record).map_err(|_| ExecutionError::Runtime)?,
        )
    }
    pub fn finish(
        &self,
        verified: &VerifiedColdRecovery,
        receipt: &serde_json::Value,
    ) -> Result<()> {
        if webdock_supervisor::timestamp().map_err(|_| ExecutionError::Runtime)? as i64
            >= verified.expires_at
        {
            return Err(ExecutionError::Expired);
        }
        let path = self.directory.join(format!("{}.json", verified.nonce));
        let mut record: ReplayRecord =
            serde_json::from_slice(&private(&path)?).map_err(|_| ExecutionError::Journal)?;
        if record.version != 1
            || record.nonce != verified.nonce
            || record.capsule_sha256 != verified.capsule_sha256
            || record.state != "pending"
        {
            return Err(ExecutionError::Fenced);
        }
        record.state = "completed".into();
        record.receipt_sha256 = Some(sha(
            &serde_json::to_vec(receipt).map_err(|_| ExecutionError::Runtime)?
        ));
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|_| ExecutionError::Runtime)?;
        temporary
            .write_all(&serde_json::to_vec(&record).map_err(|_| ExecutionError::Runtime)?)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|_| ExecutionError::Runtime)?;
        temporary
            .persist(path)
            .map_err(|_| ExecutionError::Runtime)?;
        sync_parent(&self.directory)
    }
}
