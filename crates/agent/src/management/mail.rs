use super::*;
use crate::mail::{MailAuthority, MailRuntime};
use wire::mail::{MailCheckpoint, MailClaim, MailCompletion, MailOutcome};

pub fn mail_claim(raw: &wire::MailCommand, generation: u64) -> Result<MailClaim> {
    if raw.spec_json.is_empty()
        || raw.spec_json.len() > 16384
        || raw.lease_json.is_empty()
        || raw.lease_json.len() > 2048
        || raw.material_json.is_empty()
        || raw.material_json.len() > 8192
        || raw.checkpoint_json.len() > 4096
    {
        return Err(AgentError::Configuration("Invalid Mail dispatch size"));
    }
    let claim = MailClaim {
        checkpoint: if raw.checkpoint_json.is_empty() {
            None
        } else {
            Some(
                serde_json::from_slice(&raw.checkpoint_json)
                    .map_err(|_| AgentError::Configuration("Invalid Mail checkpoint"))?,
            )
        },
        spec: serde_json::from_slice(&raw.spec_json)
            .map_err(|_| AgentError::Configuration("Invalid Mail specification"))?,
        lease: serde_json::from_slice(&raw.lease_json)
            .map_err(|_| AgentError::Configuration("Invalid Mail lease"))?,
        material: serde_json::from_slice(&raw.material_json)
            .map_err(|_| AgentError::Configuration("Invalid Mail material"))?,
        lease_until: i64::try_from(raw.lease_until)
            .ok()
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .ok_or(AgentError::Configuration("Invalid Mail expiration"))?,
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
    async fn mail_session(&self) -> Result<Session> {
        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::Transport)?;
        session.valid()?;
        Ok(session)
    }
    async fn flush_mail_delivery(&self) -> Result<()> {
        let Some(delivery) = self.mail_outbox.lock().await.pending()? else {
            return Ok(());
        };
        if self
            .mail_retry_at
            .lock()
            .await
            .is_some_and(|deadline| deadline > tokio::time::Instant::now())
        {
            return Err(AgentError::MailPending);
        }
        let session = self.mail_session().await?;
        let mut client = session.mail_client.clone();
        let result = match delivery.callback.clone().ok_or(AgentError::State)? {
            wire::mail_delivery::Callback::Checkpoint(checkpoint) => {
                if checkpoint.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::MailPending);
                }
                client.checkpoint_mail(request(checkpoint)).await
            }
            wire::mail_delivery::Callback::Receipt(receipt) => {
                if receipt.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::MailPending);
                }
                client.complete_mail(request(receipt)).await
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
            _ => {
                *self.mail_retry_at.lock().await =
                    Some(tokio::time::Instant::now() + Duration::from_secs(5));
                return Err(AgentError::MailPending);
            }
        }
        *self.mail_retry_at.lock().await = None;
        self.mail_outbox.lock().await.acknowledge(&delivery)?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl MailAuthority for RemoteAuthority {
    async fn guard(&self, claim: &MailClaim) -> webdock_supervisor::Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        let session = self
            .mail_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let response = session
            .mail_client
            .clone()
            .guard_mail(request(wire::MailGuardRequest {
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
            .map_err(|error| {
                session.rpc_error(error);
                ExecutionError::Denied
            })?
            .into_inner();
        session.valid().map_err(|_| ExecutionError::Denied)?;
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        if !response.allowed {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }
    async fn checkpoint(
        &self,
        claim: &MailClaim,
        checkpoint: MailCheckpoint,
    ) -> webdock_supervisor::Result<()> {
        MailAuthority::guard(self, claim).await?;
        checkpoint
            .validate()
            .map_err(|_| ExecutionError::Invalid("Mail checkpoint"))?;
        let session = self
            .mail_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let delivery = wire::MailDelivery {
            callback: Some(wire::mail_delivery::Callback::Checkpoint(
                wire::MailCheckpointRequest {
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
        self.mail_outbox
            .lock()
            .await
            .put(&delivery)
            .map_err(|_| ExecutionError::Journal)?;
        self.flush_mail_delivery()
            .await
            .map_err(|_| ExecutionError::Journal)
    }
}
impl Daemon {
    pub fn with_mail_runtime(mut self, runtime: Arc<dyn MailRuntime>) -> Self {
        self.mail_runtime = Some(runtime);
        self
    }
    pub(super) async fn mail_delivery_ready(&self) -> Result<bool> {
        match self.authority.flush_mail_delivery().await {
            Ok(()) => Ok(true),
            Err(AgentError::MailPending) => Ok(false),
            Err(error) => Err(error),
        }
    }
    pub(super) async fn process_mail(&self) -> Result<bool> {
        let Some(runtime) = &self.mail_runtime else {
            return Ok(false);
        };
        let session = self.session().await?;
        let dispatch = session
            .mail_client
            .clone()
            .poll_mail(request(wire::PollRequest {
                identity: Some(session.identity.clone()),
            }))
            .await
            .map_err(|error| session.rpc_error(error))?
            .into_inner();
        let Some(command) = dispatch.command else {
            return Ok(false);
        };
        let claim = mail_claim(&command, self.config.generation)?;
        MailAuthority::guard(self.authority.as_ref(), &claim).await?;
        let result = runtime.execute(&claim, self.authority.as_ref()).await;
        let (outcome, proof) = match result {
            Ok(proof) => (MailOutcome::Succeeded, Some(proof)),
            Err(error) => {
                if self.authority.mail_outbox.lock().await.pending()?.is_some() {
                    self.session().await?;
                    return Err(AgentError::MailPending);
                }
                tracing::warn!(operation_id=%claim.spec.operation_id,action=?claim.spec.action,error_code=super::error_code(&error),"Mail runtime requires reconciliation");
                (MailOutcome::NeedsReview, None)
            }
        };
        let completion = MailCompletion {
            lease: claim.lease,
            outcome,
            proof,
        };
        completion
            .validate()
            .map_err(|_| ExecutionError::Invalid("Mail completion"))?;
        let receipt = wire::MailDelivery {
            callback: Some(wire::mail_delivery::Callback::Receipt(wire::MailReceipt {
                identity: Some(session.identity),
                spec_json: command.spec_json,
                request_sha256: command.request_sha256,
                completion_json: serde_json::to_vec(&completion).map_err(|_| AgentError::State)?,
            })),
        };
        self.authority.mail_outbox.lock().await.put(&receipt)?;
        self.authority.flush_mail_delivery().await?;
        Ok(true)
    }
}
