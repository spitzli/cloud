//! Installation domain on the existing enrolled channel. Secrets never enter the callback outbox.
use super::*;
use crate::plugin_runtime::{PluginRuntimeAuthority, PluginRuntimeExecutor};
use wire::plugin::{
    MAX_MATERIAL_BYTES, PluginRuntimeCheckpoint, PluginRuntimeClaim, PluginRuntimeCompletion,
    PluginRuntimeOutcome,
};

pub(super) const DOMAIN_UNKNOWN: u8 = 0;
pub(super) const DOMAIN_AVAILABLE: u8 = 1;
pub(super) const DOMAIN_UNAVAILABLE: u8 = 2;

pub fn plugin_claim(
    raw: &wire::PluginRuntimeCommand,
    generation: u64,
) -> Result<PluginRuntimeClaim> {
    if raw.spec_json.is_empty()
        || raw.spec_json.len() > 32768
        || raw.lease_json.is_empty()
        || raw.lease_json.len() > 2048
        || raw.material_json.is_empty()
        || raw.material_json.len() > MAX_MATERIAL_BYTES
        || raw.checkpoint_json.len() > 4096
    {
        return Err(AgentError::Configuration(
            "Invalid plugin runtime dispatch size",
        ));
    }
    let claim = PluginRuntimeClaim {
        spec: serde_json::from_slice(&raw.spec_json)
            .map_err(|_| AgentError::Configuration("Invalid plugin runtime specification"))?,
        lease: serde_json::from_slice(&raw.lease_json)
            .map_err(|_| AgentError::Configuration("Invalid plugin runtime lease"))?,
        material: serde_json::from_slice(&raw.material_json)
            .map_err(|_| AgentError::Configuration("Invalid plugin runtime material"))?,
        checkpoint: if raw.checkpoint_json.is_empty() {
            None
        } else {
            Some(
                serde_json::from_slice(&raw.checkpoint_json)
                    .map_err(|_| AgentError::Configuration("Invalid plugin runtime checkpoint"))?,
            )
        },
    };
    claim
        .validate(timestamp()?)
        .map_err(|_| ExecutionError::Denied)?;
    if claim.lease.agent_generation != generation
        || claim
            .spec
            .fingerprint()
            .map_err(|_| ExecutionError::Denied)?
            != raw.request_sha256
    {
        return Err(ExecutionError::Denied.into());
    }
    Ok(claim)
}
impl RemoteAuthority {
    async fn plugin_session(&self) -> Result<Session> {
        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::Transport)?;
        session.valid()?;
        Ok(session)
    }
    pub(super) async fn flush_plugin_delivery(&self) -> Result<()> {
        let Some(delivery) = self.plugin_outbox.lock().await.pending()? else {
            return Ok(());
        };
        if self
            .plugin_retry_at
            .lock()
            .await
            .is_some_and(|deadline| deadline > tokio::time::Instant::now())
        {
            return Err(AgentError::PluginPending);
        }
        let session = self.plugin_session().await?;
        let mut client = session.plugin_client.clone();
        let result = match delivery.callback.clone().ok_or(AgentError::State)? {
            wire::plugin_runtime_delivery::Callback::Checkpoint(checkpoint) => {
                if checkpoint.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::PluginPending);
                }
                client.checkpoint_plugin_runtime(request(checkpoint)).await
            }
            wire::plugin_runtime_delivery::Callback::Receipt(receipt) => {
                if receipt.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::PluginPending);
                }
                client.complete_plugin_runtime(request(receipt)).await
            }
        };
        match result {
            Ok(response) if response.get_ref().accepted => {}
            Err(status)
                if matches!(
                    status.code(),
                    tonic::Code::Unauthenticated | tonic::Code::PermissionDenied
                ) =>
            {
                return Err(session.rpc_error(status));
            }
            _ => {
                *self.plugin_retry_at.lock().await =
                    Some(tokio::time::Instant::now() + Duration::from_secs(5));
                return Err(AgentError::PluginPending);
            }
        }
        session.valid()?;
        *self.plugin_retry_at.lock().await = None;
        self.plugin_outbox.lock().await.acknowledge(&delivery)
    }
}
#[async_trait::async_trait]
impl PluginRuntimeAuthority for RemoteAuthority {
    async fn guard(&self, claim: &PluginRuntimeClaim) -> webdock_supervisor::Result<()> {
        claim
            .validate(timestamp()?)
            .map_err(|_| ExecutionError::Expired)?;
        let session = self
            .plugin_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        if claim.spec.platform_id != session.identity.installation
            || claim.lease.agent_generation != session.identity.generation
        {
            return Err(ExecutionError::Denied);
        }
        let allowed = session
            .plugin_client
            .clone()
            .guard_plugin_runtime(request(wire::PluginRuntimeGuardRequest {
                identity: Some(session.identity.clone()),
                spec_json: serde_json::to_vec(&claim.spec).map_err(|_| ExecutionError::Runtime)?,
                lease_json: serde_json::to_vec(&claim.lease)
                    .map_err(|_| ExecutionError::Runtime)?,
                request_sha256: claim
                    .spec
                    .fingerprint()
                    .map_err(|_| ExecutionError::Denied)?,
            }))
            .await
            .map_err(|status| {
                session.rpc_error(status);
                ExecutionError::Denied
            })?
            .into_inner()
            .allowed;
        session.valid().map_err(|_| ExecutionError::Denied)?;
        claim
            .validate(timestamp()?)
            .map_err(|_| ExecutionError::Expired)?;
        if !allowed {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &PluginRuntimeClaim,
        checkpoint: PluginRuntimeCheckpoint,
    ) -> webdock_supervisor::Result<()> {
        PluginRuntimeAuthority::guard(self, claim).await?;
        checkpoint
            .validate()
            .map_err(|_| ExecutionError::Ownership)?;
        let session = self
            .plugin_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let delivery = wire::PluginRuntimeDelivery {
            callback: Some(wire::plugin_runtime_delivery::Callback::Checkpoint(
                wire::PluginRuntimeCheckpointRequest {
                    identity: Some(session.identity),
                    spec_json: serde_json::to_vec(&claim.spec)
                        .map_err(|_| ExecutionError::Runtime)?,
                    lease_json: serde_json::to_vec(&claim.lease)
                        .map_err(|_| ExecutionError::Runtime)?,
                    request_sha256: claim
                        .spec
                        .fingerprint()
                        .map_err(|_| ExecutionError::Denied)?,
                    checkpoint_json: serde_json::to_vec(&checkpoint)
                        .map_err(|_| ExecutionError::Runtime)?,
                },
            )),
        };
        self.plugin_outbox
            .lock()
            .await
            .put(&delivery)
            .map_err(|_| ExecutionError::Journal)?;
        self.flush_plugin_delivery()
            .await
            .map_err(|_| ExecutionError::Journal)?;
        *self.plugin_checkpoint.lock().await = Some(checkpoint);
        PluginRuntimeAuthority::guard(self, claim).await
    }
}
impl Daemon {
    pub fn with_plugin_runtime(mut self, runtime: Arc<dyn PluginRuntimeExecutor>) -> Self {
        self.plugin_runtime = Some(runtime);
        self
    }
    pub(super) async fn plugin_delivery_ready(&self) -> Result<bool> {
        match self.authority.flush_plugin_delivery().await {
            Ok(()) => Ok(true),
            Err(AgentError::PluginPending) => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub(super) async fn process_plugin(&self) -> Result<bool> {
        let Some(runtime) = &self.plugin_runtime else {
            return Ok(false);
        };
        if !self.config.plugin_runtime_enabled {
            return Ok(false);
        }
        let session = self.session().await?;
        if session.plugin_domain.load(Ordering::SeqCst) == DOMAIN_UNAVAILABLE {
            return Ok(false);
        }
        let response = session
            .plugin_client
            .clone()
            .poll_plugin_runtime(request(wire::PluginRuntimePollRequest {
                identity: Some(session.identity.clone()),
                max_spec_version: 2,
            }))
            .await;
        let dispatch = match response {
            Ok(response) => {
                let dispatch = response.into_inner();
                let version = wire::plugin::normalize_spec_version(dispatch.supported_spec_version)
                    .map_err(|_| {
                        AgentError::Configuration("Unsupported plugin runtime specification")
                    })?;
                session
                    .plugin_spec_version
                    .store(version as u8, Ordering::SeqCst);
                session
                    .plugin_domain
                    .store(DOMAIN_AVAILABLE, Ordering::SeqCst);
                dispatch
            }
            Err(status) if status.code() == tonic::Code::Unimplemented => {
                session
                    .plugin_domain
                    .store(DOMAIN_UNAVAILABLE, Ordering::SeqCst);
                return Ok(false);
            }
            Err(status) => return Err(session.rpc_error(status)),
        };
        let Some(command) = dispatch.command else {
            return Ok(false);
        };
        let claim = plugin_claim(&command, self.config.generation)?;
        if claim.spec.platform_id != self.config.installation
            || claim.spec.version > session.plugin_spec_version.load(Ordering::SeqCst)
        {
            return Err(ExecutionError::Ownership.into());
        }
        PluginRuntimeAuthority::guard(self.authority.as_ref(), &claim).await?;
        *self.authority.plugin_checkpoint.lock().await = claim.checkpoint.clone();
        let completion = match runtime.execute(&claim, self.authority.as_ref()).await {
            Ok(completion) => {
                PluginRuntimeAuthority::guard(self.authority.as_ref(), &claim).await?;
                completion
            }
            Err(_) => {
                if self
                    .authority
                    .plugin_outbox
                    .lock()
                    .await
                    .pending()?
                    .is_some()
                {
                    return Err(AgentError::PluginPending);
                }
                PluginRuntimeCompletion {
                    outcome: PluginRuntimeOutcome::NeedsReconciliation,
                    checkpoint: self
                        .authority
                        .plugin_checkpoint
                        .lock()
                        .await
                        .clone()
                        .unwrap_or_default(),
                    converged: false,
                }
            }
        };
        completion
            .validate()
            .map_err(|_| ExecutionError::Ownership)?;
        let delivery = wire::PluginRuntimeDelivery {
            callback: Some(wire::plugin_runtime_delivery::Callback::Receipt(
                wire::PluginRuntimeReceipt {
                    identity: Some(session.identity),
                    spec_json: command.spec_json,
                    lease_json: command.lease_json,
                    request_sha256: command.request_sha256,
                    completion_json: serde_json::to_vec(&completion)
                        .map_err(|_| AgentError::State)?,
                },
            )),
        };
        self.authority.plugin_outbox.lock().await.put(&delivery)?;
        self.authority.flush_plugin_delivery().await?;
        Ok(true)
    }
}
