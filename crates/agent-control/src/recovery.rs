//! Recovery shares native enrollment, while standing policies retain domain authority.
use super::*;
use wire::recovery::{RecoveryClaim, RecoveryCompletion};
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, Status> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(invalid());
    }
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn claim(bytes: &[u8]) -> Result<RecoveryClaim, Status> {
    let claim: RecoveryClaim = parse(bytes, 131072)?;
    claim.spec.validate().map_err(|_| invalid())?;
    claim.lease.validate_shape().map_err(|_| invalid())?;
    Ok(claim)
}
#[tonic::async_trait]
impl wire::recovery_control_server::RecoveryControl for ControlPlane {
    async fn poll_recovery(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::RecoveryDispatch>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let claim = self
            .hosting
            .claim_recovery(&agent)
            .await
            .map_err(hosting_error)?;
        if let Some(claim) = &claim {
            let identity = input.identity.as_ref().ok_or_else(invalid)?;
            self.hosting
                .bind_recovery_transport(&agent, claim, &identity.installation, &identity.agent_id)
                .await
                .map_err(hosting_error)?;
        }
        self.authorize(&request, input.identity.as_ref()).await?;
        let claim_json = claim
            .map(|c| serde_json::to_vec(&c).map_err(|_| invalid()))
            .transpose()?
            .unwrap_or_default();
        if claim_json.len() > 131072 {
            return Err(Status::resource_exhausted("Recovery dispatch limit"));
        }
        Ok(Response::new(wire::RecoveryDispatch { claim_json }))
    }
    async fn guard_recovery(
        &self,
        request: Request<wire::RecoveryGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let claim = claim(&input.claim_json)?;
        self.hosting
            .guard_recovery(&agent, &claim)
            .await
            .map_err(hosting_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::GuardReply { allowed: true }))
    }
    async fn checkpoint_recovery(
        &self,
        request: Request<wire::RecoveryCheckpointRequest>,
    ) -> Result<Response<wire::RecoveryCheckpointReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let claim = claim(&input.claim_json)?;
        let checkpoint: Value = parse(&input.checkpoint_json, 65536)?;
        if ![
            "checkpoint",
            "fencing",
            "fenced",
            "resume",
            "source-admit",
            "source-drained",
        ]
        .contains(&input.phase.as_str())
        {
            return Err(invalid());
        }
        let plan = self
            .hosting
            .apply_recovery_checkpoint(&agent, &claim, &input.phase, checkpoint)
            .await
            .map_err(hosting_error)?;
        let epoch = claim.spec.epoch.clone();
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::RecoveryCheckpointReply {
            accepted: true,
            epoch,
            plan_json: serde_json::to_vec(&plan).map_err(|_| invalid())?,
        }))
    }
    async fn complete_recovery(
        &self,
        request: Request<wire::RecoveryReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let claim = claim(&input.claim_json)?;
        let completion: RecoveryCompletion = parse(&input.completion_json, 65536)?;
        self.hosting
            .complete_recovery(&agent, &claim, &completion)
            .await
            .map_err(hosting_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
impl ControlPlane {
    pub fn recovery_service(self) -> wire::recovery_control_server::RecoveryControlServer<Self> {
        wire::recovery_control_server::RecoveryControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
}
