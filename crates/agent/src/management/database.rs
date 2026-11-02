use super::*;
use crate::database::{DatabaseAuthority, DatabaseRuntime, DatabaseStep};
use wire::database::{
    DatabaseCheckpoint, DatabaseClaim, DatabaseCompletion, DatabaseCompletionOutcome,
};

/// Validate the bounded, authenticated dispatch before passing secrets to a runtime.
pub fn database_claim(raw: &wire::DatabaseCommand, generation: u64) -> Result<DatabaseClaim> {
    if raw.spec_json.is_empty()
        || raw.spec_json.len() > 65536
        || raw.lease_json.is_empty()
        || raw.lease_json.len() > 2048
        || raw.material_json.is_empty()
        || raw.material_json.len() > 65536
    {
        return Err(AgentError::Configuration("Invalid database dispatch size"));
    }
    let claim = DatabaseClaim {
        spec: serde_json::from_slice(&raw.spec_json)
            .map_err(|_| AgentError::Configuration("Invalid database specification"))?,
        lease: serde_json::from_slice(&raw.lease_json)
            .map_err(|_| AgentError::Configuration("Invalid database lease"))?,
        lease_until: i64::try_from(raw.lease_until)
            .ok()
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .ok_or(AgentError::Configuration("Invalid database expiration"))?,
        material: serde_json::from_slice(&raw.material_json)
            .map_err(|_| AgentError::Configuration("Invalid database material"))?,
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
    async fn database_session(&self) -> Result<Session> {
        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::Transport)?;
        session.valid()?;
        Ok(session)
    }

    pub(super) async fn flush_database_delivery(&self) -> Result<()> {
        let pending = self.database_outbox.lock().await.pending()?;
        let Some(delivery) = pending else {
            return Ok(());
        };
        if self
            .database_retry_at
            .lock()
            .await
            .is_some_and(|deadline| deadline > tokio::time::Instant::now())
        {
            return Err(AgentError::DatabasePending);
        }
        let session = self.database_session().await?;
        let mut client = session.database_client.clone();
        let result = match delivery.callback.clone().ok_or(AgentError::State)? {
            wire::database_delivery::Callback::Checkpoint(checkpoint) => {
                if checkpoint.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::DatabasePending);
                }
                client.checkpoint_database(request(checkpoint)).await
            }
            wire::database_delivery::Callback::Receipt(receipt) => {
                if receipt.identity.as_ref() != Some(&session.identity) {
                    return Err(AgentError::DatabasePending);
                }
                client.complete_database(request(receipt)).await
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
                *self.database_retry_at.lock().await =
                    Some(tokio::time::Instant::now() + Duration::from_secs(5));
                return Err(AgentError::DatabasePending);
            }
        }
        *self.database_retry_at.lock().await = None;
        self.database_outbox.lock().await.acknowledge(&delivery)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DatabaseAuthority for RemoteAuthority {
    async fn guard(&self, claim: &DatabaseClaim) -> webdock_supervisor::Result<()> {
        claim
            .validate(chrono::Utc::now())
            .map_err(|_| ExecutionError::Expired)?;
        let session = self
            .database_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let result = session
            .database_client
            .clone()
            .guard_database(request(wire::DatabaseGuardRequest {
                identity: Some(session.identity.clone()),
                spec_json: serde_json::to_vec(&claim.spec).map_err(|_| ExecutionError::Runtime)?,
                lease_json: serde_json::to_vec(&claim.lease)
                    .map_err(|_| ExecutionError::Runtime)?,
                request_sha256: claim
                    .spec
                    .fingerprint()
                    .map_err(|_| ExecutionError::Invalid("database fingerprint"))?,
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
        if !result.allowed {
            return Err(ExecutionError::Denied);
        }
        Ok(())
    }

    async fn checkpoint(
        &self,
        claim: &DatabaseClaim,
        checkpoint: DatabaseCheckpoint,
    ) -> webdock_supervisor::Result<()> {
        DatabaseAuthority::guard(self, claim).await?;
        checkpoint
            .validate()
            .map_err(|_| ExecutionError::Invalid("database checkpoint"))?;
        let session = self
            .database_session()
            .await
            .map_err(|_| ExecutionError::Denied)?;
        let delivery = wire::DatabaseDelivery {
            callback: Some(wire::database_delivery::Callback::Checkpoint(
                wire::DatabaseCheckpointRequest {
                    identity: Some(session.identity),
                    spec_json: serde_json::to_vec(&claim.spec)
                        .map_err(|_| ExecutionError::Runtime)?,
                    request_sha256: claim
                        .spec
                        .fingerprint()
                        .map_err(|_| ExecutionError::Invalid("database fingerprint"))?,
                    checkpoint_json: serde_json::to_vec(&checkpoint)
                        .map_err(|_| ExecutionError::Runtime)?,
                },
            )),
        };
        self.database_outbox
            .lock()
            .await
            .put(&delivery)
            .map_err(|_| ExecutionError::Journal)?;
        self.flush_database_delivery()
            .await
            .map_err(|_| ExecutionError::Journal)
    }
}

impl Daemon {
    pub(super) async fn database_delivery_ready(&self) -> Result<bool> {
        match self.authority.flush_database_delivery().await {
            Ok(()) => Ok(true),
            Err(AgentError::DatabasePending) => Ok(false),
            Err(error) => Err(error),
        }
    }
    /// Alternate database executors still receive only the authenticated Core authority.
    pub fn with_database_runtime(mut self, runtime: Arc<dyn DatabaseRuntime>) -> Self {
        self.database_runtime = Some(runtime);
        self
    }

    pub(super) async fn process_database(&self) -> Result<bool> {
        let Some(runtime) = &self.database_runtime else {
            return Ok(false);
        };
        let session = self.session().await?;
        let dispatch = session
            .database_client
            .clone()
            .poll_database(request(wire::PollRequest {
                identity: Some(session.identity.clone()),
            }))
            .await
            .map_err(|error| session.rpc_error(error))?
            .into_inner();
        let Some(command) = dispatch.command else {
            return Ok(false);
        };
        let claim = database_claim(&command, self.config.generation)?;
        DatabaseAuthority::guard(self.authority.as_ref(), &claim).await?;
        let outcome = runtime.execute(&claim, self.authority.as_ref()).await;
        let (outcome, proof) = match outcome {
            Ok(DatabaseStep::Pending) => return Ok(true),
            Ok(DatabaseStep::Complete(proof)) => {
                (DatabaseCompletionOutcome::Succeeded, Some(proof))
            }
            Err(error) => {
                // A checkpoint may have committed remotely; replay its exact envelope before polling again.
                if self
                    .authority
                    .database_outbox
                    .lock()
                    .await
                    .pending()?
                    .is_some()
                {
                    self.session().await?;
                    return Err(AgentError::DatabasePending);
                }
                tracing::warn!(
                    operation_id = %claim.spec.operation_id,
                    action = ?claim.spec.action,
                    phase = ?claim.spec.phase,
                    error_code = super::error_code(&error),
                    "Database runtime step requires reconciliation"
                );
                (DatabaseCompletionOutcome::NeedsReconciliation, None)
            }
        };
        let completion = DatabaseCompletion {
            lease: claim.lease,
            outcome,
            proof,
        };
        completion
            .validate()
            .map_err(|_| ExecutionError::Invalid("database completion"))?;
        let receipt = wire::DatabaseDelivery {
            callback: Some(wire::database_delivery::Callback::Receipt(
                wire::DatabaseReceipt {
                    identity: Some(session.identity),
                    spec_json: command.spec_json,
                    request_sha256: command.request_sha256,
                    completion_json: serde_json::to_vec(&completion)
                        .map_err(|_| AgentError::State)?,
                },
            )),
        };
        self.authority.database_outbox.lock().await.put(&receipt)?;
        self.authority.flush_database_delivery().await?;
        Ok(true)
    }
}
