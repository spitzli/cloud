use super::*;
use crate::mail::configuration::{ConfigurationAuthority, ConfigurationRuntime};
use wire::mail::MailOutcome;
use wire::mail_configuration::{
    ConfigurationCheckpoint, ConfigurationCheckpointPhase, ConfigurationClaim,
    ConfigurationCompletion, ConfigurationMode, ConfigurationResourceKind,
};

pub fn configuration_claim(
    raw: &wire::MailConfigurationCommand,
    generation: u64,
) -> Result<ConfigurationClaim> {
    if raw.spec_json.is_empty()
        || raw.spec_json.len() > 262144
        || raw.lease_json.is_empty()
        || raw.lease_json.len() > 2048
        || raw.material_json.is_empty()
        || raw.material_json.len() > 65536
        || raw.checkpoints_json.len() > 524288
    {
        return Err(AgentError::Configuration(
            "Invalid Mail configuration dispatch size",
        ));
    }
    let claim = ConfigurationClaim {
        spec: serde_json::from_slice(&raw.spec_json)
            .map_err(|_| AgentError::Configuration("Invalid Mail configuration specification"))?,
        material: serde_json::from_slice(&raw.material_json)
            .map_err(|_| AgentError::Configuration("Invalid Mail configuration material"))?,
        lease: serde_json::from_slice(&raw.lease_json)
            .map_err(|_| AgentError::Configuration("Invalid Mail configuration lease"))?,
        checkpoints: if raw.checkpoints_json.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&raw.checkpoints_json)
                .map_err(|_| AgentError::Configuration("Invalid Mail configuration checkpoints"))?
        },
        lease_until: i64::try_from(raw.lease_until)
            .ok()
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .ok_or(AgentError::Configuration(
                "Invalid Mail configuration expiration",
            ))?,
    };
    claim
        .validate(chrono::Utc::now())
        .map_err(|_| ExecutionError::Denied)?;
    if u64::try_from(claim.spec.agent_generation).ok() != Some(generation)
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
    async fn configuration_session(&self) -> Result<Session> {
        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::Transport)?;
        session.valid()?;
        Ok(session)
    }
    async fn flush_configuration_delivery(&self) -> Result<()> {
        let Some(delivery) = self.configuration_outbox.lock().await.pending()? else {
            return Ok(());
        };
        if self
            .configuration_retry_at
            .lock()
            .await
            .is_some_and(|deadline| deadline > tokio::time::Instant::now())
        {
            return Err(AgentError::ConfigurationPending);
        }
        let session = self.configuration_session().await?;
        let mut client = session.configuration_client.clone();
        let result = match delivery.callback.clone().ok_or(AgentError::State)? {
            wire::mail_configuration_delivery::Callback::Checkpoint(checkpoint) => {
                if checkpoint.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::ConfigurationPending);
                }
                client
                    .checkpoint_mail_configuration(request(checkpoint))
                    .await
            }
            wire::mail_configuration_delivery::Callback::Receipt(receipt) => {
                if receipt.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::ConfigurationPending);
                }
                client.complete_mail_configuration(request(receipt)).await
            }
        };
        match result {
            Ok(response) if response.get_ref().accepted => {}
            Err(status)
                if matches!(
                    status.code(),
                    tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
                ) =>
            {
                return Err(session.rpc_error(status));
            }
            failed => {
                if matches!(failed, Err(ref status) if status.code() == tonic::Code::Unimplemented)
                {
                    session
                        .configuration_available
                        .store(false, Ordering::SeqCst);
                }
                *self.configuration_retry_at.lock().await =
                    Some(tokio::time::Instant::now() + Duration::from_secs(5));
                return Err(AgentError::ConfigurationPending);
            }
        }
        *self.configuration_retry_at.lock().await = None;
        self.configuration_outbox
            .lock()
            .await
            .acknowledge(&delivery)
    }
}
#[async_trait::async_trait]
impl ConfigurationAuthority for RemoteAuthority {
    async fn guard(&self, claim: &ConfigurationClaim) -> webdock_supervisor::Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        let session = self
            .configuration_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let result = session
            .configuration_client
            .clone()
            .guard_mail_configuration(request(wire::MailConfigurationGuardRequest {
                identity: Some(session.identity.clone()),
                spec_json: serde_json::to_vec(&claim.spec).map_err(|_| ExecutionError::Runtime)?,
                lease_json: serde_json::to_vec(&claim.lease)
                    .map_err(|_| ExecutionError::Runtime)?,
                request_sha256: claim
                    .spec
                    .fingerprint()
                    .map_err(|_| ExecutionError::Denied)?,
            }))
            .await;
        let reply = match result {
            Ok(reply) => reply.into_inner(),
            Err(status)
                if matches!(
                    status.code(),
                    tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
                ) =>
            {
                session.rpc_error(status);
                return Err(ExecutionError::Denied);
            }
            Err(status)
                if matches!(
                    status.code(),
                    tonic::Code::FailedPrecondition | tonic::Code::NotFound
                ) =>
            {
                return Err(ExecutionError::Denied);
            }
            Err(status) => {
                if status.code() == tonic::Code::Unimplemented {
                    session
                        .configuration_available
                        .store(false, Ordering::SeqCst);
                }
                self.configuration_transient.store(true, Ordering::SeqCst);
                return Err(ExecutionError::Runtime);
            }
        };
        session.valid().map_err(|_| ExecutionError::Denied)?;
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        if !reply.allowed {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &ConfigurationClaim,
        checkpoint: ConfigurationCheckpoint,
    ) -> webdock_supervisor::Result<()> {
        ConfigurationAuthority::guard(self, claim).await?;
        checkpoint
            .validate()
            .map_err(|_| ExecutionError::Invalid("Mail configuration checkpoint"))?;
        if checkpoint.lease.lease_token != claim.lease.lease_token {
            return Err(ExecutionError::Denied);
        }
        if let ConfigurationCheckpointPhase::Handoff { resource, .. }
        | ConfigurationCheckpointPhase::Applied { resource, .. } = &checkpoint.phase
            && (!claim.spec.contains(resource)
                || claim.spec.mode == ConfigurationMode::Restrict
                    && resource.kind != ConfigurationResourceKind::Identity)
        {
            return Err(ExecutionError::Denied);
        }
        let session = self
            .configuration_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let delivery = wire::MailConfigurationDelivery {
            callback: Some(wire::mail_configuration_delivery::Callback::Checkpoint(
                wire::MailConfigurationCheckpointRequest {
                    identity: Some(session.identity),
                    spec_json: serde_json::to_vec(&claim.spec)
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
        self.configuration_outbox
            .lock()
            .await
            .put(&delivery)
            .map_err(|_| ExecutionError::Journal)?;
        self.flush_configuration_delivery()
            .await
            .map_err(|_| ExecutionError::Journal)
    }
}
impl Daemon {
    pub fn with_configuration_runtime(
        mut self,
        runtime: Arc<dyn ConfigurationRuntime>,
    ) -> Result<Self> {
        if !self.config.mail_configuration_enabled || self.mail_runtime.is_none() {
            return Err(AgentError::Configuration(
                "Mail configuration requires explicit opt-in and reviewed lifecycle runtime",
            ));
        }
        self.configuration_runtime = Some(runtime);
        Ok(self)
    }
    pub(super) async fn configuration_delivery_ready(&self) -> Result<bool> {
        match self.authority.flush_configuration_delivery().await {
            Ok(()) => Ok(true),
            Err(AgentError::ConfigurationPending) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub(super) async fn process_configuration(&self) -> Result<bool> {
        let Some(runtime) = &self.configuration_runtime else {
            return Ok(false);
        };
        if !self.config.mail_configuration_enabled {
            return Ok(false);
        }
        let session = self.session().await?;
        if !session.configuration_available.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let dispatch = match session
            .configuration_client
            .clone()
            .poll_mail_configuration(request(wire::PollRequest {
                identity: Some(session.identity.clone()),
            }))
            .await
        {
            Ok(dispatch) => dispatch.into_inner(),
            Err(status)
                if matches!(
                    status.code(),
                    tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
                ) =>
            {
                return Err(session.rpc_error(status));
            }
            Err(status) => {
                if status.code() == tonic::Code::Unimplemented {
                    session
                        .configuration_available
                        .store(false, Ordering::SeqCst);
                }
                return Ok(false);
            }
        };
        let Some(command) = dispatch.command else {
            return Ok(false);
        };
        let Ok(claim) = configuration_claim(&command, self.config.generation) else {
            tracing::warn!("Invalid optional Mail configuration dispatch refused");
            return Ok(false);
        };
        self.authority
            .configuration_transient
            .store(false, Ordering::SeqCst);
        let result = match ConfigurationAuthority::guard(self.authority.as_ref(), &claim).await {
            Ok(()) => runtime.configure(&claim, self.authority.as_ref()).await,
            Err(error) => Err(error),
        };
        self.session().await?;
        if self
            .authority
            .configuration_outbox
            .lock()
            .await
            .pending()?
            .is_some()
        {
            return Err(AgentError::ConfigurationPending);
        }
        if self
            .authority
            .configuration_transient
            .load(Ordering::SeqCst)
        {
            return Ok(false);
        }
        let (outcome, proof) = match result {
            Ok(proof) if proof.validate_for(&claim.spec).is_ok() => {
                (MailOutcome::Succeeded, Some(proof))
            }
            _ => (MailOutcome::NeedsReview, None),
        };
        let completion = ConfigurationCompletion {
            lease: claim.lease,
            outcome,
            proof,
        };
        completion
            .validate()
            .map_err(|_| ExecutionError::Invalid("Mail configuration completion"))?;
        let delivery = wire::MailConfigurationDelivery {
            callback: Some(wire::mail_configuration_delivery::Callback::Receipt(
                wire::MailConfigurationReceipt {
                    identity: Some(session.identity),
                    spec_json: command.spec_json,
                    request_sha256: command.request_sha256,
                    completion_json: serde_json::to_vec(&completion)
                        .map_err(|_| AgentError::State)?,
                },
            )),
        };
        self.authority
            .configuration_outbox
            .lock()
            .await
            .put(&delivery)?;
        self.authority.flush_configuration_delivery().await?;
        Ok(true)
    }
}
