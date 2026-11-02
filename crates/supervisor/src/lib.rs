//! Durable, provider-neutral command coordination. This crate has no Engine or cluster access.
#[cfg(feature = "journal")]
mod journal;
mod material;
mod services;
pub use services::*;
mod spec;
#[cfg(feature = "journal")]
pub use journal::{Claim, Journal};
pub use material::*;
use serde::{Deserialize, Serialize};
pub use spec::*;
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Invalid runtime contract: {0}")]
    Invalid(&'static str),
    #[error("Execution authority denied")]
    Denied,
    #[error("Execution lease expired")]
    Expired,
    #[error("Execution generation fenced")]
    Fenced,
    #[error("Operation input conflicts with its durable receipt")]
    Conflict,
    #[error("Runtime resource ownership mismatch")]
    Ownership,
    #[error("Unsupported runtime capability: {0}")]
    Unsupported(&'static str),
    #[error("Runtime request failed; reconciliation required")]
    Runtime,
    #[error("Durable command journal unavailable")]
    Journal,
}
pub type Result<T, E = ExecutionError> = std::result::Result<T, E>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Apply,
    Stop,
    Retire,
    PurgeStorage,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Command {
    pub operation_id: String,
    pub generation: u64,
    pub lease_until: u64,
    pub authorization: String,
    pub action: Action,
    pub spec: RuntimeSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_runtime_id: Option<String>,
    #[serde(skip)]
    pub material: Option<RuntimeMaterial>,
}
impl Command {
    pub fn validate(&self) -> Result<()> {
        self.spec.validate()?;
        if self.action == Action::PurgeStorage
            && (self.spec.volume.is_none() || self.expected_runtime_id.is_some())
        {
            return Err(ExecutionError::Invalid(
                "purge requires retained storage and absent runtime binding",
            ));
        }
        if self.expected_runtime_id.as_ref().is_some_and(|value| {
            value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }) {
            return Err(ExecutionError::Invalid("expected runtime identity"));
        }
        if self.operation_id.is_empty()
            || self.operation_id.len() > 64
            || !self
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || self.generation == 0
            || self.generation > 9_007_199_254_740_991
            || self.authorization.is_empty()
            || self.authorization.len() > 16384
        {
            return Err(ExecutionError::Invalid("command identity"));
        }
        Ok(())
    }
    pub fn payload_fingerprint(&self) -> Result<String> {
        digest(&(self.action, &self.spec, &self.expected_runtime_id))
    }
    pub fn fingerprint(&self) -> Result<String> {
        if let Some(expected) = &self.expected_runtime_id {
            digest(&(self.generation, self.action, &self.spec, expected))
        } else {
            digest(&(self.generation, self.action, &self.spec))
        }
    }
}
pub fn timestamp() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| ExecutionError::Expired)
}
pub fn digest(value: &impl Serialize) -> Result<String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(value).map_err(|_| ExecutionError::Invalid("serialization"))?
        )
    ))
}
#[async_trait::async_trait]
pub trait Authorizer: Send + Sync + 'static {
    /// Verify the enrolled agent, durable operation, resource ownership, fencing generation
    /// and live lease. A bearer string or a future timestamp is not sufficient authority.
    async fn authorize(&self, _command: &Command) -> Result<()> {
        Err(ExecutionError::Denied)
    }
    async fn authorize_read(&self, _command: &ReadCommand) -> Result<()> {
        Err(ExecutionError::Denied)
    }
}
pub struct DenyAll;
#[async_trait::async_trait]
impl Authorizer for DenyAll {}
#[derive(Clone)]
pub struct LeaseGuard {
    command: Arc<Command>,
    authority: Arc<dyn Authorizer>,
}
impl LeaseGuard {
    pub fn new(command: Command, authority: Arc<dyn Authorizer>) -> Self {
        Self {
            command: Arc::new(command),
            authority,
        }
    }
    pub async fn check_for(&self, command: &Command) -> Result<()> {
        if command.operation_id != self.command.operation_id
            || command.fingerprint()? != self.command.fingerprint()?
        {
            return Err(ExecutionError::Denied);
        }
        self.check().await
    }
    pub async fn check(&self) -> Result<()> {
        if self.command.lease_until <= timestamp()? {
            return Err(ExecutionError::Expired);
        }
        tokio::time::timeout(
            Duration::from_secs(3),
            self.authority.authorize(&self.command),
        )
        .await
        .map_err(|_| ExecutionError::Denied)??;
        if self.command.lease_until <= timestamp()? {
            return Err(ExecutionError::Expired);
        }
        Ok(())
    }
}
#[async_trait::async_trait]
pub trait RuntimeAdapter: Send + Sync + 'static {
    fn capabilities(&self) -> Capabilities;
    async fn target_observation(&self) -> Result<Option<TargetObservation>> {
        Ok(None)
    }
    /// Reconcile actual owned state before each retry, never blindly repeat provisioning.
    async fn check_expected(&self, command: &Command) -> Result<()> {
        if let Some(expected) = &command.expected_runtime_id {
            let observed = self.inspect(&command.spec).await?;
            if !(observed.runtime_id.as_ref() == Some(expected)
                || (command.action == Action::Retire && observed.phase == Phase::Absent))
            {
                return Err(ExecutionError::Ownership);
            }
        }
        Ok(())
    }
    async fn reconcile(&self, command: &Command, guard: &LeaseGuard) -> Result<ObservedState>;
    /// Read-only proof of a previously completed mutation under a renewed lease.
    /// Deletion adapters must inspect retained storage or its durable purge evidence.
    async fn inspect_completed(
        &self,
        command: &Command,
        guard: &LeaseGuard,
    ) -> Result<ObservedState> {
        guard.check_for(command).await?;
        self.check_expected(command).await?;
        let observed = self.inspect(&command.spec).await?;
        guard.check_for(command).await?;
        Ok(observed)
    }
    async fn inspect(&self, spec: &RuntimeSpec) -> Result<ObservedState>;
    async fn logs(&self, spec: &RuntimeSpec, tail: u32) -> Result<String>;
    async fn metrics(&self, _spec: &RuntimeSpec) -> Result<ResourceMetrics> {
        Err(ExecutionError::Unsupported("metrics"))
    }
}
#[cfg(feature = "journal")]
pub struct Executor {
    journal: tokio::sync::Mutex<Journal>,
    authority: Arc<dyn Authorizer>,
    adapter: Arc<dyn RuntimeAdapter>,
}
#[cfg(feature = "journal")]
impl Executor {
    pub fn new(
        journal: Journal,
        authority: Arc<dyn Authorizer>,
        adapter: Arc<dyn RuntimeAdapter>,
    ) -> Self {
        Self {
            journal: tokio::sync::Mutex::new(journal),
            authority,
            adapter,
        }
    }
    pub async fn execute(&self, command: Command) -> Result<ObservedState> {
        command.validate()?;
        let guard = LeaseGuard::new(command.clone(), self.authority.clone());
        guard.check().await?;
        // The journal's process lock plus this mutex exclude concurrent executors and generations.
        let mut journal = self.journal.lock().await;
        guard.check().await?;
        let verification = match journal.claim(&command)? {
            Claim::Replay(result) => return Ok(result),
            Claim::Verify(result) => Some(result),
            Claim::Execute { .. } => None,
        };
        tracing::info!(operation_id=%command.operation_id,resource_id=%command.spec.resource_id,generation=command.generation,"runtime command accepted");
        let work = async {
            if let Some(previous) = &verification {
                let started = timestamp()?;
                let observed = self.adapter.inspect_completed(&command, &guard).await?;
                verify_reinspection(&command, previous, &observed, started)?;
                return Ok(observed);
            }
            if command.action == Action::Apply {
                command.verify_material()?;
            }
            self.adapter.check_expected(&command).await?;
            self.adapter.reconcile(&command, &guard).await
        };
        tokio::pin!(work);
        let deadline = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(deadline);
        let mut check = tokio::time::interval(Duration::from_millis(250));
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result=&mut work=>break result,
                _=&mut deadline=>break Err(ExecutionError::Expired),
                _=check.tick()=>if let Err(e)=guard.check().await {break Err(e);},
            }
        };
        let result = match result {
            Ok(result) => guard.check().await.map(|()| result),
            Err(e) => Err(e),
        };
        match result {
            Ok(result) => {
                if result.resource_id != command.spec.resource_id.as_str() {
                    journal.uncertain(&command)?;
                    return Err(ExecutionError::Ownership);
                }
                journal.complete(&command, &result)?;
                tracing::info!(operation_id=%command.operation_id,phase=?result.phase,"runtime command committed");
                Ok(result)
            }
            Err(error) => {
                // Keep the persisted verification requirement after a failed inspection.
                // A retry must not fall through to reapplying the old completed mutation.
                if verification.is_none() {
                    journal.uncertain(&command)?;
                }
                tracing::warn!(operation_id=%command.operation_id,"runtime command requires reconciliation");
                Err(error)
            }
        }
    }
}

#[cfg(feature = "journal")]
fn verify_reinspection(
    command: &Command,
    previous: &ObservedState,
    observed: &ObservedState,
    started: u64,
) -> Result<()> {
    if observed.resource_id != command.spec.resource_id.as_str()
        || observed.revision != command.spec.revision
        || observed.runtime_id != previous.runtime_id
        || command
            .expected_runtime_id
            .as_ref()
            .is_some_and(|expected| {
                observed.runtime_id.as_ref() != Some(expected)
                    && !(command.action == Action::Retire && observed.phase == Phase::Absent)
            })
    {
        return Err(ExecutionError::Ownership);
    }
    if observed.observed_at < started || observed.observed_at > timestamp()?.saturating_add(30) {
        return Err(ExecutionError::Runtime);
    }
    let converged = match command.action {
        Action::Retire | Action::PurgeStorage => {
            observed.phase == Phase::Absent && observed.ready && observed.runtime_id.is_none()
        }
        Action::Stop => observed.phase == Phase::Stopped,
        Action::Apply if command.spec.replicas == 0 => observed.phase == Phase::Stopped,
        Action::Apply => {
            observed.phase == Phase::Running && observed.ready && observed.runtime_id.is_some()
        }
    };
    if !converged {
        return Err(ExecutionError::Runtime);
    }
    if let Some(volume) = &command.spec.volume {
        let expected = if command.action == Action::PurgeStorage {
            Some(0)
        } else {
            volume.size_bytes
        };
        if observed.storage.as_ref().is_none_or(|storage| {
            storage.volume_bytes != expected
                || storage.retained != (command.action == Action::Retire)
        }) {
            return Err(ExecutionError::Runtime);
        }
    } else if observed.storage.as_ref().is_some_and(|storage| {
        storage.retained || storage.volume_bytes.is_some_and(|bytes| bytes > 0)
    }) {
        return Err(ExecutionError::Runtime);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadKind {
    Inspect,
    Logs,
    Metrics,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadCommand {
    pub operation_id: String,
    pub generation: u64,
    pub lease_until: u64,
    pub authorization: String,
    pub kind: ReadKind,
    pub tail: u32,
    pub spec: RuntimeSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_runtime_id: Option<String>,
}
impl ReadCommand {
    pub fn validate(&self) -> Result<()> {
        Command {
            operation_id: self.operation_id.clone(),
            generation: self.generation,
            lease_until: self.lease_until,
            authorization: self.authorization.clone(),
            action: Action::Apply,
            material: None,
            spec: self.spec.clone(),
            expected_runtime_id: self.expected_runtime_id.clone(),
        }
        .validate()?;
        if (self.kind == ReadKind::Logs && !(1..=1000).contains(&self.tail))
            || (self.kind != ReadKind::Logs && self.tail != 0)
        {
            return Err(ExecutionError::Invalid("read parameters"));
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String> {
        if let Some(expected) = &self.expected_runtime_id {
            digest(&(self.generation, self.kind, self.tail, &self.spec, expected))
        } else {
            digest(&(self.generation, self.kind, self.tail, &self.spec))
        }
    }
}
