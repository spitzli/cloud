//! Native operator recovery: real enrolled Core leases online, independently signed cold capsules.
mod archive;
pub mod capsule;
pub mod config;
#[cfg(feature = "docker")]
mod docker;
pub mod effect_fence;
pub mod gateway;
#[cfg(feature = "kubernetes")]
mod kubernetes;
mod mail;
mod restore_authority;
use crate::storage::{
    StorageClient, StorageConfig, StorageGuard, StoragePacket,
    recovery_io::{ApplicationRecoveryPacket, ApplicationStagingOrigin},
    recovery_pool::{
        RecoveryReservationAuthority, RecoveryReservationReceipt, RecoveryReserveRequest,
        reservation_marker, reservation_result_variant,
    },
};
use config::{
    RecoveryProfileConfig, RecoveryRuntimeConfig, RecoverySource, RecoveryTarget,
    TargetConfiguration,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use webdock_agent_protocol::recovery::{RecoveryAction, RecoveryClaim, RecoveryProfile};
use webdock_object_storage::ObjectReceipt;
use webdock_supervisor::{ExecutionError, Result};

async fn traced_reservation_rpc<T>(
    work: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    reservation_marker("agent", "work_first_poll", "none");
    let result = work.await;
    reservation_marker("agent", "rpc_result", reservation_result_variant(&result));
    result
}
#[cfg(test)]
mod reservation_diagnostic_tests {
    use super::*;
    #[tokio::test]
    async fn reservation_diagnostic_future_is_lazy_and_preserves_result() {
        let polls = AtomicBool::new(false);
        let work = async {
            polls.store(true, Ordering::SeqCst);
            Err::<(), _>(ExecutionError::Invalid("PRIVATE"))
        };
        let traced = traced_reservation_rpc(work);
        assert!(!polls.load(Ordering::SeqCst));
        drop(traced);
        assert!(!polls.load(Ordering::SeqCst));
        let result = traced_reservation_rpc(async {
            polls.store(true, Ordering::SeqCst);
            Err::<(), _>(ExecutionError::Invalid("PRIVATE"))
        })
        .await;
        assert!(polls.load(Ordering::SeqCst));
        assert!(matches!(result, Err(ExecutionError::Invalid("PRIVATE"))));
    }
}
#[async_trait::async_trait]
pub trait RecoveryAuthority: Send + Sync {
    async fn guard(&self, _claim: &RecoveryClaim) -> Result<()> {
        Err(ExecutionError::Denied)
    }
    async fn checkpoint(
        &self,
        _claim: &RecoveryClaim,
        _phase: &str,
        _value: Value,
    ) -> Result<Value> {
        Err(ExecutionError::Denied)
    }
}
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum AppEffectPhase {
    Preflight,
    Stop,
    Capture,
    Upload,
    Resume,
    Release,
}
pub(crate) fn app_backup(c: &RecoveryClaim) -> bool {
    c.spec.profile == RecoveryProfile::NtfySqliteOfflineV1
        && c.spec.action == RecoveryAction::Backup
}
pub(crate) fn app_checkpoint(c: &RecoveryClaim, phase: &str, value: &Value) -> bool {
    app_backup(c)
        && phase == "checkpoint"
        && matches!(
            value["phase"].as_str(),
            Some(
                "preflight"
                    | "app-admit"
                    | "app-stopped"
                    | "app-captured"
                    | "uploaded-verified"
                    | "app-resume-intent"
                    | "app-resumed"
            )
        )
}
fn app_guard_claim(
    c: &RecoveryClaim,
    phase: AppEffectPhase,
    fingerprint: &str,
) -> Result<RecoveryClaim> {
    if !app_backup(c)
        || fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err(ExecutionError::Denied);
    }
    let mut guarded = c.clone();
    guarded.checkpoint = Some(json!({"appGuard":{"phase":phase,"fenceFingerprint":fingerprint}}));
    Ok(guarded)
}
struct AppSourceSession {
    binding: effect_fence::AppFenceBinding,
    source_fence: effect_fence::SameFencePermit,
    drained_participants: Vec<Value>,
}
pub(crate) fn app_resume_binding(
    c: &RecoveryClaim,
    checkpoint: &Value,
    plan: &Value,
) -> Result<effect_fence::AppFenceBinding> {
    if !app_backup(c)
        || c.spec.validate().is_err()
        || c.lease.validate_shape().is_err()
        || c.checkpoint
            .as_ref()
            .is_some_and(|v| v.get("appGuard").is_some())
        || checkpoint.as_object().is_none_or(|v| v.len() != 4)
        || checkpoint["phase"] != "app-resumed"
        || checkpoint["providerOutcome"] != "confirmed-complete"
        || plan["sourceFence"]["sourceKind"] != "app"
        || plan["sourceFence"]["state"] != "released"
    {
        return Err(ExecutionError::Denied);
    }
    let b: effect_fence::AppFenceBinding =
        serde_json::from_value(plan["sourceFence"]["binding"].clone())
            .map_err(|_| ExecutionError::Ownership)?;
    if b.operation_id != c.spec.operation_id
        || b.generation != c.lease.generation
        || b.policy_id != c.spec.policy_id
        || b.policy_revision != c.spec.policy_revision
        || b.epoch != c.spec.epoch
        || b.cluster_id != c.spec.cluster_id
        || c.spec.source["appID"] != b.app_id
        || c.spec.source["projectID"] != b.project_id
        || c.spec.source["customerID"] != b.customer_id
        || c.spec.source["revision"] != b.source_revision
        || c.spec.source["runtimeUID"] != b.source_uid
        || checkpoint["runtimeUID"] != b.source_uid
        || checkpoint["fenceFingerprint"] != b.fence_fingerprint
    {
        return Err(ExecutionError::Ownership);
    }
    Ok(b)
}
async fn unsubmitted_capture_failure<T>(
    submitted: bool,
    primary: ExecutionError,
    resume: impl std::future::Future<Output = Result<()>>,
) -> Result<T> {
    if !submitted {
        let _ = resume.await;
    }
    Err(primary)
}
#[cfg(test)]
mod app_guard_tests {
    use super::*;
    fn claim() -> RecoveryClaim {
        serde_json::from_value(json!({"spec":{"version":1,"operationID":"123","policyID":"124","policyRevision":1,"clusterID":"125","profile":"ntfy-sqlite-offline-v1","action":"backup","source":{},"target":null,"recoverySetID":null,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","capacityBytes":67108864},"lease":{"generation":1,"token":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","expiresAt":"2099-01-01T00:00:00Z"},"checkpoint":{"phase":"retained-original"}})).unwrap()
    }
    #[test]
    fn app_effect_envelope_only_clones_and_never_changes_mail_or_original_claim() {
        let mut c = claim();
        let original = serde_json::to_value(&c).unwrap();
        for phase in [
            AppEffectPhase::Preflight,
            AppEffectPhase::Stop,
            AppEffectPhase::Capture,
            AppEffectPhase::Upload,
            AppEffectPhase::Resume,
            AppEffectPhase::Release,
        ] {
            let guarded = app_guard_claim(&c, phase, &"a".repeat(64)).unwrap();
            let mut guarded = serde_json::to_value(guarded).unwrap();
            assert_eq!(
                guarded["checkpoint"]["appGuard"].as_object().unwrap().len(),
                2
            );
            guarded["checkpoint"] = original["checkpoint"].clone();
            assert_eq!(guarded, original);
        }
        assert_eq!(serde_json::to_value(&c).unwrap(), original);
        assert!(app_guard_claim(&c, AppEffectPhase::Stop, &"A".repeat(64)).is_err());
        c.spec.profile = RecoveryProfile::MailInstallationV1;
        let mail = serde_json::to_vec(&c).unwrap();
        assert!(app_guard_claim(&c, AppEffectPhase::Stop, &"a".repeat(64)).is_err());
        assert_eq!(serde_json::to_vec(&c).unwrap(), mail);
        assert!(!app_checkpoint(
            &c,
            "checkpoint",
            &json!({"phase":"app-admit"})
        ));
    }
    #[test]
    fn app_resume_ack_binds_original_claim_checkpoint_and_released_response() {
        let mut c = claim();
        c.spec.operation_id = "129".into();
        c.spec.policy_id = "127".into();
        c.spec.cluster_id = "126".into();
        c.spec.source = json!({"appID":"123","projectID":"124","customerID":"125","revision":1,"runtimeUID":"a".repeat(64)});
        let binding = json!({"appID":"123","projectID":"124","customerID":"125","clusterID":"126","policyID":"127","policyRevision":1,"agentGeneration":1,"sourceRevision":1,"sourceUID":"a".repeat(64),"specSHA256":"b".repeat(64),"environmentSHA256":"c".repeat(64),"sourceOperationID":"128","sourceOperationGeneration":1,"sourceOperationStatus":"succeeded","sourceOperationFingerprint":"d".repeat(64),"operationID":"129","generation":1,"epoch":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","fenceFingerprint":"e".repeat(64)});
        let checkpoint = json!({"phase":"app-resumed","providerOutcome":"confirmed-complete","runtimeUID":"a".repeat(64),"fenceFingerprint":"e".repeat(64)});
        let plan = json!({"sourceFence":{"sourceKind":"app","state":"released","binding":binding}});
        let original = serde_json::to_vec(&c).unwrap();
        assert!(app_resume_binding(&c, &checkpoint, &plan).is_ok());
        for invalid in [Value::Null, json!("wrong"), json!("uncertain")] {
            let mut changed = checkpoint.clone();
            changed["providerOutcome"] = invalid;
            assert!(app_resume_binding(&c, &changed, &plan).is_err());
        }
        let mut missing = checkpoint.clone();
        missing.as_object_mut().unwrap().remove("providerOutcome");
        assert!(app_resume_binding(&c, &missing, &plan).is_err());
        for key in ["generation", "sourceRevision"] {
            let mut changed = plan.clone();
            changed["sourceFence"]["binding"][key] = json!(2);
            assert!(app_resume_binding(&c, &checkpoint, &changed).is_err());
        }
        let mut changed = plan.clone();
        changed["sourceFence"]["state"] = json!("resuming");
        assert!(app_resume_binding(&c, &checkpoint, &changed).is_err());
        assert_eq!(serde_json::to_vec(&c).unwrap(), original);
    }
    #[tokio::test]
    async fn denied_stale_capture_purpose_never_reaches_the_provider_effect() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct CanonicalResume;
        #[async_trait::async_trait]
        impl RecoveryAuthority for CanonicalResume {
            async fn guard(&self, c: &RecoveryClaim) -> Result<()> {
                if c.checkpoint
                    .as_ref()
                    .is_some_and(|v| v["appGuard"]["phase"] == "resume")
                {
                    Ok(())
                } else {
                    Err(ExecutionError::Fenced)
                }
            }
        }
        let c = claim();
        let stale = app_guard_claim(&c, AppEffectPhase::Capture, &"a".repeat(64)).unwrap();
        let guard = StorageGuard::new(
            chrono::Utc::now() + chrono::Duration::seconds(60),
            Arc::new(AtomicBool::new(false)),
        );
        let calls = AtomicUsize::new(0);
        let allowed =
            restore_authority::before_effect(Some((&stale, &CanonicalResume)), &guard).await;
        if allowed.is_ok() {
            calls.fetch_add(1, Ordering::SeqCst);
        }
        assert!(allowed.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn compensation_never_resumes_submitted_work_or_masks_the_primary_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let result: Result<()> =
            unsubmitted_capture_failure(true, ExecutionError::Ownership, async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await;
        assert!(matches!(result, Err(ExecutionError::Ownership)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let result: Result<()> =
            unsubmitted_capture_failure(false, ExecutionError::Fenced, async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ExecutionError::Denied)
            })
            .await;
        assert!(matches!(result, Err(ExecutionError::Fenced)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
pub struct NativeRecoveryExecutor {
    path: PathBuf,
    config_sha256: String,
    config: RecoveryRuntimeConfig,
    storage: StorageClient,
    pool: StorageConfig,
    pool_identity: crate::storage::recovery_pool::RecoveryPoolIdentity,
    mail_effect_fence: effect_fence::MailEffectFence,
    app_fences: Option<Arc<effect_fence::AppEffectFenceRegistry>>,
    #[cfg(feature = "docker")]
    docker: bollard::Docker,
}
impl NativeRecoveryExecutor {
    pub(crate) async fn finish_app_resume_ack(
        &self,
        c: &RecoveryClaim,
        checkpoint: &Value,
        plan: &Value,
        a: &dyn RecoveryAuthority,
    ) -> Result<()> {
        let binding = app_resume_binding(c, checkpoint, plan)?;
        let guarded = app_guard_claim(c, AppEffectPhase::Release, &binding.fence_fingerprint)?;
        self.app_fences
            .as_ref()
            .ok_or(ExecutionError::Fenced)?
            .finish_release_ack(binding, self.guard(&guarded, a))
            .await
    }
    pub async fn open(path: PathBuf) -> Result<Self> {
        Self::open_inner(path, None).await
    }
    pub(crate) async fn open_with_app_fences(
        path: PathBuf,
        fences: Arc<effect_fence::AppEffectFenceRegistry>,
    ) -> Result<Self> {
        Self::open_inner(path, Some(fences)).await
    }
    async fn open_inner(
        path: PathBuf,
        app_fences: Option<Arc<effect_fence::AppEffectFenceRegistry>>,
    ) -> Result<Self> {
        let config = RecoveryRuntimeConfig::load(&path)?;
        let config_sha256 = config::fingerprint(&config)?;
        let storage = StorageClient::new(config.storage_socket.clone());
        let pool = storage.information().await?;
        let pool_identity = storage.recovery_pool_identity().await?;
        if pool_identity.configuration != pool {
            return Err(ExecutionError::Ownership);
        }
        if config.custody_directory.starts_with(&pool.root)
            || pool.root.starts_with(&config.custody_directory)
        {
            return Err(ExecutionError::Invalid(
                "independent recovery custody outside source pool required",
            ));
        }
        #[cfg(feature = "docker")]
        let docker = docker::connect(&config.docker_socket).await?;
        #[cfg(feature = "docker")]
        if docker
            .info()
            .await
            .map_err(|_| ExecutionError::Runtime)?
            .name
            .as_deref()
            != Some(&pool.node)
        {
            return Err(ExecutionError::Ownership);
        }
        let mail_effect_fence = effect_fence::MailEffectFence::open(
            &config.replay_directory.join("source-effects.json"),
        )?;
        Ok(Self {
            path,
            config_sha256,
            config,
            storage,
            pool,
            pool_identity,
            mail_effect_fence,
            app_fences,
            #[cfg(feature = "docker")]
            docker,
        })
    }
    pub fn mail_effect_fence(&self) -> effect_fence::MailEffectFence {
        self.mail_effect_fence.clone()
    }
    pub fn capabilities(&self) -> Result<Value> {
        self.verify_configuration()?;
        let mut value = self.config.public_capabilities()?;
        value["version"] = json!(2);
        if let Some(fences) = &self.app_fences {
            fences.available()?;
            value["appEffectFenceVersion"] = json!(1);
        }
        for profile in value["profiles"]
            .as_array_mut()
            .ok_or(ExecutionError::Ownership)?
        {
            profile["storageIdentity"] =
                serde_json::to_value(&self.pool_identity).map_err(|_| ExecutionError::Ownership)?;
        }
        for (declared, target) in value["targets"]
            .as_array_mut()
            .ok_or(ExecutionError::Ownership)?
            .iter_mut()
            .zip(&self.config.targets)
        {
            declared["sha256"] = json!(config::fingerprint(&(target, &self.pool_identity))?);
            declared["storageIdentity"] =
                serde_json::to_value(&self.pool_identity).map_err(|_| ExecutionError::Ownership)?;
        }
        Ok(value)
    }
    fn verify_configuration(&self) -> Result<()> {
        if config::fingerprint(&RecoveryRuntimeConfig::load(&self.path)?)? != self.config_sha256 {
            return Err(ExecutionError::Fenced);
        }
        Ok(())
    }
    async fn guard(&self, c: &RecoveryClaim, a: &dyn RecoveryAuthority) -> Result<()> {
        c.validate().map_err(|_| ExecutionError::Expired)?;
        a.guard(c).await?;
        if self.storage.recovery_pool_identity().await? != self.pool_identity {
            return Err(ExecutionError::Ownership);
        }
        self.verify_configuration()
    }
    async fn guarded<T>(
        &self,
        c: &RecoveryClaim,
        a: &dyn RecoveryAuthority,
        cancelled: Arc<AtomicBool>,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        self.guard(c, a).await?;
        tokio::pin!(work);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.tick().await;
        loop {
            tokio::select! {
                result=&mut work=>{let result=result?;self.guard(c,a).await?;return Ok(result);},
                _=interval.tick()=>if let Err(error)=self.guard(c,a).await{cancelled.store(true,Ordering::SeqCst);return Err(error);},
            }
        }
    }
    // Offline helpers must finish exact cleanup even after authority is lost. The
    // cancellation flag denies further source effects; only cleanup may continue.
    #[cfg(feature = "kubernetes")]
    async fn guarded_cleanup<T>(
        &self,
        c: &RecoveryClaim,
        a: &dyn RecoveryAuthority,
        cancelled: Arc<AtomicBool>,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        self.guard(c, a).await?;
        tokio::pin!(work);
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.tick().await;
        loop {
            tokio::select! {
                result = &mut work => { let result = result?; self.guard(c,a).await?; return Ok(result); },
                _ = interval.tick() => if let Err(error) = self.guard(c,a).await {
                    cancelled.store(true, Ordering::SeqCst);
                    let _ = tokio::time::timeout(Duration::from_secs(60), &mut work).await;
                    return Err(error);
                },
            }
        }
    }
    pub async fn execute(&self, c: &RecoveryClaim, a: &dyn RecoveryAuthority) -> Result<Value> {
        if app_backup(c) {
            c.validate().map_err(|_| ExecutionError::Expired)?;
            self.verify_configuration()?;
        } else {
            self.guard(c, a).await?;
        }
        if c.spec.action == RecoveryAction::Backup
            && self.config.mode == config::RecoveryMode::RestoreOnly
        {
            return Err(ExecutionError::Denied);
        }
        let profile = self
            .config
            .profiles
            .iter()
            .find(|p| p.profile == c.spec.profile && c.spec.source["profileID"] == p.id)
            .ok_or(ExecutionError::Denied)?;
        if c.spec.action == RecoveryAction::Restore {
            let declared = c.spec.target.as_ref().ok_or(ExecutionError::Ownership)?;
            let target = self
                .config
                .targets
                .iter()
                .find(|target| {
                    declared["profileID"] == target.id && target.profile == c.spec.profile
                })
                .ok_or(ExecutionError::Ownership)?;
            config::authorize_target(target, declared, &self.pool_identity)?;
        }
        let plan = a
            .checkpoint(c, "checkpoint", json!({"phase":"preflight"}))
            .await?;
        if plan["destination"]["sha256"] != config::fingerprint(profile)? {
            return Err(ExecutionError::Fenced);
        }
        let request: RecoveryReserveRequest = serde_json::from_value(plan["reservation"].clone())
            .map_err(|_| ExecutionError::Denied)?;
        if request.pool != self.pool_identity {
            return Err(ExecutionError::Ownership);
        }
        let session = if app_backup(c) {
            Some(self.admit_ntfy_source(c, a, profile, &plan).await?)
        } else {
            None
        };
        let reserve_claim = session
            .as_ref()
            .map(|s| app_guard_claim(c, AppEffectPhase::Preflight, &s.binding.fence_fingerprint))
            .transpose()?;
        reservation_marker("agent", "outer_enter", "none");
        let reservation = self
            .guarded(
                reserve_claim.as_ref().unwrap_or(c),
                a,
                Arc::new(AtomicBool::new(false)),
                traced_reservation_rpc(self.storage.reserve_recovery(&request)),
            )
            .await;
        reservation_marker(
            "agent",
            "guarded_result",
            reservation_result_variant(&reservation),
        );
        let reservation = reservation?;
        if reservation.pool != self.pool_identity
            || reservation.profile != c.spec.profile
            || reservation.action != c.spec.action
            || reservation.operation_id.as_ref().map(|id| id.as_str())
                != Some(c.spec.operation_id.as_str())
            || reservation.generation != Some(c.lease.generation)
        {
            return Err(ExecutionError::Ownership);
        }
        match (c.spec.profile, c.spec.action) {
            (RecoveryProfile::NtfySqliteOfflineV1, RecoveryAction::Backup) => {
                self.backup_ntfy(
                    c,
                    a,
                    profile,
                    session.ok_or(ExecutionError::Fenced)?,
                    &reservation,
                )
                .await
            }
            (RecoveryProfile::MailInstallationV1, RecoveryAction::Backup) => {
                mail::backup(self, c, a, profile, &plan, &reservation).await
            }
            (_, RecoveryAction::Restore) => {
                let target_id = c
                    .spec
                    .target
                    .as_ref()
                    .and_then(|v| v["profileID"].as_str())
                    .ok_or(ExecutionError::Invalid("recovery target"))?;
                let target = self
                    .config
                    .targets
                    .iter()
                    .find(|t| t.id == target_id && t.profile == c.spec.profile)
                    .ok_or(ExecutionError::Denied)?;
                config::authorize_target(
                    target,
                    c.spec.target.as_ref().ok_or(ExecutionError::Denied)?,
                    &self.pool_identity,
                )?;
                let proof = plan["recoverySet"]["proof"].clone();
                self.restore(
                    c.spec.profile,
                    profile,
                    target,
                    &proof,
                    c.spec.capacity_bytes,
                    c.lease.expires_at,
                    &reservation,
                    ApplicationStagingOrigin::OnlineRestore {
                        operation_id: webdock_core::id::RecordId::parse(&c.spec.operation_id)
                            .map_err(|_| ExecutionError::Ownership)?,
                        generation: c.lease.generation,
                        target_nonce: target.nonce.clone(),
                    },
                    Some((c, a)),
                )
                .await
            }
        }
    }
    pub async fn execute_cold(
        &self,
        document: &capsule::SignedDocument,
        target_id: &str,
    ) -> Result<Value> {
        self.verify_configuration()?;
        if self.storage.recovery_pool_identity().await? != self.pool_identity {
            return Err(ExecutionError::Ownership);
        }
        let target = self
            .config
            .targets
            .iter()
            .find(|t| t.id == target_id)
            .ok_or(ExecutionError::Denied)?;
        let target_sha = config::fingerprint(&(target, &self.pool_identity))?;
        let verified = capsule::verify_capsule(
            document,
            &self.config.core_signers,
            &self.config.operator_signers,
            &target.id,
            &target_sha,
            target.capacity_bytes,
            &self.config.staging_directory,
            chrono::Utc::now().timestamp(),
        )?;
        if verified.nonce != target.nonce || verified.set.profile != target.profile {
            return Err(ExecutionError::Fenced);
        }
        let profile = self
            .config
            .profiles
            .iter()
            .find(|p| p.id == verified.set.profile_id && p.profile == verified.set.profile)
            .ok_or(ExecutionError::Denied)?;
        let journal = capsule::ReplayJournal::open(&self.config.replay_directory)?;
        journal.begin(&verified)?;
        let deadline = chrono::DateTime::from_timestamp(verified.expires_at, 0)
            .ok_or(ExecutionError::Expired)?;
        let reservation = self
            .storage
            .reserve_recovery(&RecoveryReserveRequest {
                authority: RecoveryReservationAuthority::OperatorCapsule {
                    document: document.clone(),
                },
                target_profile_id: target.id.clone(),
                target_profile_sha256: target_sha.clone(),
                target_nonce: verified.nonce.clone(),
                pool: self.pool_identity.clone(),
            })
            .await?;
        if reservation.origin != "operator-capsule"
            || reservation.pool != self.pool_identity
            || reservation.target_nonce != verified.nonce
            || reservation.source_set_id.as_ref().map(|id| id.as_str())
                != Some(verified.set.set_id.as_str())
        {
            return Err(ExecutionError::Ownership);
        }
        let mut admitted_proof = verified.set.proof.clone();
        admitted_proof["operationID"] = json!(verified.set.operation_id);
        admitted_proof["policyID"] = json!(verified.set.policy_id);
        let mut proof = self
            .restore(
                verified.set.profile,
                profile,
                target,
                &admitted_proof,
                verified.set.capacity_bytes,
                deadline,
                &reservation,
                ApplicationStagingOrigin::OperatorCapsule {
                    target_nonce: verified.nonce.clone(),
                    admitted_set_id: webdock_core::id::RecordId::parse(&verified.set.set_id)
                        .map_err(|_| ExecutionError::Ownership)?,
                },
                None,
            )
            .await?;
        proof["offlineOperatorID"] = json!(verified.actor_id);
        proof["offlineTrustBoundary"] = json!(
            "quarantine-only; later source revocation/delivery history unknown; replay custody local to this target"
        );
        journal.finish(&verified, &proof)?;
        Ok(proof)
    }
    pub fn target_identity(&self, id: &str) -> Result<String> {
        let target = self
            .config
            .targets
            .iter()
            .find(|t| t.id == id)
            .ok_or(ExecutionError::Denied)?;
        config::fingerprint(&(target, &self.pool_identity))
    }
    async fn admit_ntfy_source(
        &self,
        c: &RecoveryClaim,
        a: &dyn RecoveryAuthority,
        profile: &RecoveryProfileConfig,
        plan: &Value,
    ) -> Result<AppSourceSession> {
        let RecoverySource::Ntfy {
            runtime,
            runtime_id,
        } = &profile.source
        else {
            return Err(ExecutionError::Denied);
        };
        if c.spec.source["appID"] != runtime.resource_id.as_str()
            || c.spec.source["projectID"] != runtime.project_id.as_str()
            || c.spec.source["runtimeUID"] != *runtime_id
            || c.spec.source["revision"] != runtime.revision
            || c.spec.source["spec"]["image"] != runtime.image
            || c.spec.source["spec"]["args"] != json!(runtime.arguments)
        {
            return Err(ExecutionError::Ownership);
        }
        let source_view = &plan["sourceFence"];
        let mut participants = source_view["enrolledExecutors"]
            .as_array()
            .ok_or(ExecutionError::Ownership)?
            .clone();
        for participant in &mut participants {
            if participant["identity"] == source_view["agentIdentity"]["identity"]
                && participant["uid"] == source_view["agentIdentity"]["uid"]
            {
                participant["kind"] = json!("guarded-agent");
                participant["effectGuardVersion"] = json!("app-1");
            } else {
                participant["kind"] = json!("legacy-worker");
            }
        }
        let admitted = a
            .checkpoint(
                c,
                "checkpoint",
                json!({"phase":"app-admit","participants":participants}),
            )
            .await?;
        let binding: effect_fence::AppFenceBinding =
            serde_json::from_value(admitted["sourceFence"]["binding"].clone())
                .map_err(|_| ExecutionError::Ownership)?;
        if binding.app_id != runtime.resource_id.as_str()
            || binding.project_id != runtime.project_id.as_str()
            || binding.cluster_id != c.spec.cluster_id
            || c.spec.source["customerID"] != binding.customer_id
            || binding.source_revision != runtime.revision
            || binding.source_uid != *runtime_id
            || binding.operation_id != c.spec.operation_id
            || binding.generation != c.lease.generation
            || binding.epoch != c.spec.epoch
            || binding.policy_id != c.spec.policy_id
            || binding.policy_revision != c.spec.policy_revision
            || runtime
                .hosted_access
                .as_ref()
                .and_then(|v| v.environment.as_ref())
                .is_some_and(|env| env.source_sha256 != binding.environment_sha256)
        {
            return Err(ExecutionError::Ownership);
        }
        let preflight = app_guard_claim(c, AppEffectPhase::Preflight, &binding.fence_fingerprint)?;
        let source_fence = self
            .app_fences
            .as_ref()
            .ok_or(ExecutionError::Fenced)?
            .close_authorized(binding.clone(), self.guard(&preflight, a))
            .await?;
        source_fence.drained()?;
        // This executor cannot declare another/legacy process or remote request stopped.
        if participants.iter().any(|p| p["kind"] != "guarded-agent") {
            return Err(ExecutionError::Fenced);
        }
        let drained_participants = participants
            .iter()
            .map(|p| {
                let mut p = p.clone();
                p["state"] = json!("guarded-drained");
                p["uncertain"] = json!(false);
                p
            })
            .collect::<Vec<_>>();
        Ok(AppSourceSession {
            binding,
            source_fence,
            drained_participants,
        })
    }
    async fn resume_ntfy_source(
        &self,
        c: &RecoveryClaim,
        a: &dyn RecoveryAuthority,
        runtime: &webdock_supervisor::RuntimeSpec,
        stopped: &docker::StoppedApplication,
        session: &AppSourceSession,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let binding = &session.binding;
        let source_fence = &session.source_fence;
        let runtime_id = &binding.source_uid;
        // This verifies an existing explicit Core intent; the error path never creates it.
        let resume_claim = app_guard_claim(c, AppEffectPhase::Resume, &binding.fence_fingerprint)?;
        self.guard(&resume_claim, a).await?;
        source_fence.drained()?;
        self.guarded(
            &resume_claim,
            a,
            cancelled.clone(),
            docker::resume_application(&self.docker, runtime, stopped, source_fence),
        )
        .await?;
        self.guard(&resume_claim, a).await?;
        source_fence.record_resume_completion(binding)?;
        let resume_checkpoint = json!({"phase":"app-resumed","runtimeUID":runtime_id,
                "fenceFingerprint":binding.fence_fingerprint,"providerOutcome":"confirmed-complete"});
        let resumed = a
            .checkpoint(c, "checkpoint", resume_checkpoint.clone())
            .await?;
        self.finish_app_resume_ack(c, &resume_checkpoint, &resumed, a)
            .await?;
        Ok(())
    }
    async fn backup_ntfy(
        &self,
        c: &RecoveryClaim,
        a: &dyn RecoveryAuthority,
        profile: &RecoveryProfileConfig,
        session: AppSourceSession,
        reservation: &RecoveryReservationReceipt,
    ) -> Result<Value> {
        #[cfg(not(feature = "docker"))]
        {
            let _ = (c, a, profile, session, reservation);
            Err(ExecutionError::Unsupported(
                "ntfy native Docker qualification",
            ))
        }
        #[cfg(feature = "docker")]
        {
            let RecoverySource::Ntfy {
                runtime,
                runtime_id,
            } = &profile.source
            else {
                return Err(ExecutionError::Denied);
            };
            let AppSourceSession {
                binding,
                source_fence,
                drained_participants,
            } = &session;
            let cancelled = Arc::new(AtomicBool::new(false));
            let guard = StorageGuard::new(c.lease.expires_at, cancelled.clone());
            let stop_claim = app_guard_claim(c, AppEffectPhase::Stop, &binding.fence_fingerprint)?;
            let stopped = self
                .guarded(
                    &stop_claim,
                    a,
                    cancelled.clone(),
                    docker::stop_application(
                        &self.docker,
                        runtime,
                        runtime_id,
                        &self.pool,
                        source_fence,
                    ),
                )
                .await?;
            let source = StoragePacket::new(
                runtime.resource_id.as_str(),
                runtime.project_id.as_str(),
                &c.spec.cluster_id,
                runtime.revision,
                runtime
                    .volume
                    .as_ref()
                    .and_then(|v| v.size_bytes)
                    .ok_or(ExecutionError::Ownership)?,
                c.lease.expires_at.timestamp() as u64,
            )?;
            let mut packet = ApplicationRecoveryPacket {
                reservation: Some(crate::storage::recovery_io::RecoveryReservationRef {
                    reservation_id: reservation.reservation_id.clone(),
                    grant_sha256: reservation.grant_sha256.clone(),
                }),
                staging_origin: None,
                operation_id: webdock_core::id::RecordId::parse(&c.spec.operation_id)
                    .map_err(|_| ExecutionError::Ownership)?,
                generation: c.lease.generation,
                source,
                target: None,
                source_uuid: uuid::Uuid::nil().to_string(),
                manifest_sha256: None,
            };
            // The helper independently derives the real allocation from the signed source
            // packet; no client-provided filesystem identity can authorize this lookup.
            let record = self.storage.application_record(&packet).await?;
            packet.source_uuid = record.uuid.clone();
            a.checkpoint(
                c,
                "checkpoint",
                json!({"phase":"app-stopped","runtimeUID":runtime_id,
                "sourceUUID":record.uuid,"writersAbsent":true,"sourceRetained":true,
                "fenceFingerprint":binding.fence_fingerprint,"participants":drained_participants}),
            )
            .await?;
            source_fence.drained()?;
            let capture_claim =
                app_guard_claim(c, AppEffectPhase::Capture, &binding.fence_fingerprint)?;
            let submitted = AtomicBool::new(false);
            let snapshot = self
                .guarded(&capture_claim, a, cancelled.clone(), async {
                    submitted.store(true, Ordering::SeqCst);
                    self.storage.snapshot_application(&packet).await
                })
                .await;
            let snapshot = match snapshot {
                Ok(snapshot) => snapshot,
                Err(primary) => {
                    return unsubmitted_capture_failure(
                        submitted.load(Ordering::SeqCst),
                        primary,
                        self.resume_ntfy_source(
                            c,
                            a,
                            runtime,
                            &stopped,
                            &session,
                            cancelled.clone(),
                        ),
                    )
                    .await;
                }
            };
            a.checkpoint(c,"checkpoint",json!({"phase":"app-captured","runtimeUID":runtime_id,
                "sourceUUID":record.uuid,"writersAbsent":true,"manifestSHA256":snapshot.manifest_sha256,
                "fenceFingerprint":binding.fence_fingerprint,"participants":drained_participants})).await?;
            let upload_claim =
                app_guard_claim(c, AppEffectPhase::Upload, &binding.fence_fingerprint)?;
            self.guard(&upload_claim, a).await?;
            let directory = reservation.staging_directory.join("backup-work");
            std::fs::create_dir(&directory).map_err(|_| ExecutionError::Conflict)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| ExecutionError::Runtime)?;
            let plain = directory.join("data.tar");
            archive::pack(&snapshot.directory, &plain, &guard, c.spec.capacity_bytes)?;
            let encrypted = directory.join("data.age");
            self.guarded(
                &upload_claim,
                a,
                cancelled.clone(),
                archive::age(&plain, &encrypted, profile, false),
            )
            .await?;
            self.guard(&upload_claim, a).await?;
            let material = directory.join("material.json");
            archive::capture_material(profile, stopped.environment.clone(), &material)?;
            let envelope = self.config.custody_directory.join(format!(
                "{}-{}-material.age",
                c.spec.operation_id, c.lease.generation
            ));
            self.guarded(
                &upload_claim,
                a,
                cancelled.clone(),
                archive::age(&material, &envelope, profile, false),
            )
            .await?;
            let key_receipt = archive::receipt(&envelope, &profile.key_custody_id)?;
            let store = self.config.store(profile, false).await?;
            let file = tokio::fs::File::open(&encrypted)
                .await
                .map_err(|_| ExecutionError::Runtime)?;
            let object = self
                .guarded(&upload_claim, a, cancelled.clone(), async {
                    store
                        .upload(
                            &format!("recovery/{}/{}", profile.id, c.spec.policy_id),
                            file,
                            c.spec.capacity_bytes,
                        )
                        .await
                        .map_err(|_| ExecutionError::Runtime)
                })
                .await?;
            a.checkpoint(c,"checkpoint",json!({"phase":"uploaded-verified","objects":[object],"manifestSha256":snapshot.manifest_sha256,"sourceVolume":snapshot.volume,"keyMaterial":key_receipt,"fenceFingerprint":binding.fence_fingerprint})).await?;
            a.checkpoint(
                c,
                "checkpoint",
                json!({"phase":"app-resume-intent","runtimeUID":runtime_id,
                "fenceFingerprint":binding.fence_fingerprint}),
            )
            .await?;
            self.resume_ntfy_source(c, a, runtime, &stopped, &session, cancelled.clone())
                .await?;
            Ok(
                json!({"version":1,"profile":c.spec.profile,"operationID":c.spec.operation_id,"policyID":c.spec.policy_id,"objects":[object],"source":{"volume":snapshot.volume,"packet":packet.source,"runtime":runtime,"runtimeID":runtime_id},"manifestSha256":snapshot.manifest_sha256,"sourceFenceFingerprint":binding.fence_fingerprint,"keyMaterial":key_receipt,"sourceResumed":stopped.wanted_running,"sourceRetained":true,"rpo":"all writers stopped before SQLite backup"}),
            )
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Restore receives an independently authorized profile, target, receipts, budget, deadline and optional live Core authority."
    )]
    async fn restore(
        &self,
        profile_kind: RecoveryProfile,
        profile: &RecoveryProfileConfig,
        target: &RecoveryTarget,
        proof: &Value,
        capacity: u64,
        deadline: chrono::DateTime<chrono::Utc>,
        reservation: &RecoveryReservationReceipt,
        origin: ApplicationStagingOrigin,
        online: Option<(&RecoveryClaim, &dyn RecoveryAuthority)>,
    ) -> Result<Value> {
        if deadline <= chrono::Utc::now()
            || capacity > target.capacity_bytes
            || proof["version"] != 1
        {
            return Err(ExecutionError::Denied);
        }
        if profile_kind == RecoveryProfile::MailInstallationV1 {
            return mail::restore(
                self,
                profile,
                target,
                proof,
                capacity,
                deadline,
                online,
                reservation,
            )
            .await;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = StorageGuard::new(deadline, cancelled.clone());
        let receipt: ObjectReceipt = serde_json::from_value(
            proof["objects"]
                .as_array()
                .and_then(|v| v.first())
                .cloned()
                .ok_or(ExecutionError::Invalid("recovery receipt"))?,
        )
        .map_err(|_| ExecutionError::Invalid("recovery receipt"))?;
        let key_receipt: archive::CustodyReceipt =
            serde_json::from_value(proof["keyMaterial"].clone())
                .map_err(|_| ExecutionError::Invalid("independent recovery material"))?;
        if key_receipt.id != profile.key_custody_id {
            return Err(ExecutionError::Denied);
        }
        let key_path = archive::material_path(&self.config.custody_directory, &key_receipt)?;
        let staging = reservation.staging_directory.join("restore-work");
        std::fs::create_dir(&staging).map_err(|_| ExecutionError::Conflict)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| ExecutionError::Runtime)?;
        let material = staging.join("material.json");
        archive::age(&key_path, &material, profile, true).await?;
        guard.check()?;
        let material: archive::RecoveryMaterial =
            serde_json::from_slice(&config::private(&material, 8 * 1024 * 1024)?)
                .map_err(|_| ExecutionError::Invalid("recovery material"))?;
        let encrypted = staging.join("data.age");
        let store = self.config.store(profile, true).await?;
        let mut output = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&encrypted)
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        store
            .download(
                &format!(
                    "recovery/{}/{}",
                    profile.id,
                    proof["policyID"].as_str().unwrap_or("unbound")
                ),
                &receipt,
                &mut output,
                capacity,
            )
            .await
            .map_err(|_| ExecutionError::Ownership)?;
        output
            .sync_all()
            .await
            .map_err(|_| ExecutionError::Runtime)?;
        drop(output);
        let plain = staging.join("data.tar");
        archive::age(&encrypted, &plain, profile, true).await?;
        guard.check()?;
        if let Some((c, a)) = online {
            self.guard(c, a).await?;
        }
        let source: crate::storage::VolumeRecord =
            serde_json::from_value(proof["source"]["volume"].clone())
                .map_err(|_| ExecutionError::Ownership)?;
        let TargetConfiguration::Ntfy { runtime } = &target.configuration else {
            return Err(ExecutionError::Invalid("ntfy fresh target"));
        };
        let source_packet = StoragePacket::new(
            source.app_id.as_str(),
            source.project_id.as_str(),
            source.cluster_id.as_str(),
            proof["source"]["packet"]["revision"]
                .as_u64()
                .ok_or(ExecutionError::Ownership)?,
            source.bytes,
            deadline.timestamp() as u64,
        )?;
        let target_packet = StoragePacket::new(
            runtime.resource_id.as_str(),
            runtime.project_id.as_str(),
            self.pool.cluster_id.as_str(),
            runtime.revision,
            runtime
                .volume
                .as_ref()
                .and_then(|v| v.size_bytes)
                .ok_or(ExecutionError::Ownership)?,
            deadline.timestamp() as u64,
        )?;
        let (operation_id, generation) = match &origin {
            ApplicationStagingOrigin::OnlineRestore {
                operation_id,
                generation,
                target_nonce,
            } => {
                let (claim, _) = online.ok_or(ExecutionError::Denied)?;
                if operation_id.as_str() != claim.spec.operation_id
                    || *generation != claim.lease.generation
                    || *target_nonce != target.nonce
                {
                    return Err(ExecutionError::Ownership);
                }
                (operation_id.clone(), *generation)
            }
            ApplicationStagingOrigin::OperatorCapsule { target_nonce, .. } => {
                if online.is_some() || *target_nonce != target.nonce {
                    return Err(ExecutionError::Ownership);
                }
                (
                    webdock_core::id::RecordId::parse(
                        proof["operationID"]
                            .as_str()
                            .ok_or(ExecutionError::Ownership)?,
                    )
                    .map_err(|_| ExecutionError::Ownership)?,
                    1,
                )
            }
        };
        let packet = ApplicationRecoveryPacket {
            reservation: Some(crate::storage::recovery_io::RecoveryReservationRef {
                reservation_id: reservation.reservation_id.clone(),
                grant_sha256: reservation.grant_sha256.clone(),
            }),
            staging_origin: Some(origin),
            operation_id,
            generation,
            source: source_packet,
            target: Some(target_packet.clone()),
            source_uuid: source.uuid.clone(),
            manifest_sha256: proof["manifestSha256"].as_str().map(str::to_owned),
        };
        let directory = self.storage.application_staging(&packet).await?;
        archive::unpack(&plain, &directory, &guard, capacity)?;
        let installed = self.storage.install_application(&packet).await?;
        guard.check()?;
        #[cfg(not(feature = "docker"))]
        return Err(ExecutionError::Unsupported("ntfy Docker target"));
        #[cfg(feature = "docker")]
        {
            let runtime_id = docker::start_restored_application(
                &self.docker,
                target,
                &self.pool,
                &self.storage,
                &target_packet,
                &material.environment,
                1,
            )
            .await?;
            let gateway_id = docker::start_gateway(
                &self.docker,
                target,
                &runtime_id,
                &staging,
                false,
                (deadline - chrono::Utc::now()).num_seconds().max(1) as u64,
            )
            .await?;
            if let Some((c, a)) = online {
                self.guard(c, a).await?;
            }
            Ok(
                json!({"version":1,"dispatch":"quarantined","objects":[receipt],"targetVolume":installed.volume,"sourceUUID":source.uuid,"runtimeID":runtime_id,"gatewayRuntimeID":gateway_id,"targetNonce":target.nonce,"networkMode":"none"}),
            )
        }
    }
}
