//! Mail methods share ControlPlane's transport verification and Hosting agent transaction.
use super::*;
use wire::mail::{MailCheckpoint, MailCompletion, MailLease, MailSpec};
fn mail_error(error: webdock_mail::incoming::Error) -> Status {
    match error {
        webdock_mail::incoming::Error::Invalid => invalid(),
        webdock_mail::incoming::Error::Conflict => {
            Status::failed_precondition("Mail operation authority changed")
        }
        webdock_mail::incoming::Error::Unavailable => {
            Status::failed_precondition("Mail operation unavailable")
        }
    }
}
pub(super) fn parse<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
) -> Result<T, Status> {
    if bytes.is_empty() || bytes.len() > limit {
        return Err(invalid());
    }
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn spec(bytes: &[u8], hash: &str) -> Result<MailSpec, Status> {
    let spec: MailSpec = parse(bytes, 16384)?;
    if spec.fingerprint().map_err(|_| invalid())? != hash {
        return Err(invalid());
    }
    Ok(spec)
}
#[tonic::async_trait]
impl wire::mail_control_server::MailControl for ControlPlane {
    async fn poll_mail(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::MailDispatch>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let service = self
            .mail
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Mail execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let claim = service.claim(tx, &agent).await.map_err(mail_error)?;
        let command = claim
            .map(|claim| {
                claim.validate(chrono::Utc::now()).map_err(|_| invalid())?;
                let checkpoint_json = claim
                    .checkpoint
                    .as_ref()
                    .map(serde_json::to_vec)
                    .transpose()
                    .map_err(|_| invalid())?
                    .unwrap_or_default();
                Ok::<_, Status>(wire::MailCommand {
                    checkpoint_json,
                    request_sha256: claim.spec.fingerprint().map_err(|_| invalid())?,
                    spec_json: serde_json::to_vec(&claim.spec).map_err(|_| invalid())?,
                    lease_json: serde_json::to_vec(&claim.lease).map_err(|_| invalid())?,
                    lease_until: u64::try_from(claim.lease_until.timestamp())
                        .map_err(|_| invalid())?,
                    material_json: serde_json::to_vec(&claim.material).map_err(|_| invalid())?,
                })
            })
            .transpose()?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::MailDispatch { command }))
    }
    async fn guard_mail(
        &self,
        request: Request<wire::MailGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let lease: MailLease = parse(&input.lease_json, 2048)?;
        let service = self
            .mail
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Mail execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .guard(tx, &agent, &spec, &lease)
            .await
            .map_err(mail_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::GuardReply { allowed: true }))
    }
    async fn checkpoint_mail(
        &self,
        request: Request<wire::MailCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let checkpoint: MailCheckpoint = parse(&input.checkpoint_json, 4096)?;
        let service = self
            .mail
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Mail execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .checkpoint(tx, &agent, &spec, checkpoint)
            .await
            .map_err(mail_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_mail(
        &self,
        request: Request<wire::MailReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let completion: MailCompletion = parse(&input.completion_json, 8192)?;
        let service = self
            .mail
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Mail execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .complete(tx, &agent, &spec, completion)
            .await
            .map_err(mail_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
