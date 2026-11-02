//! Optional mailbox configuration shares the established enrollment and signed agent scope.
use super::*;
use wire::mail::MailLease;
use wire::mail_configuration::{
    ConfigurationCheckpoint, ConfigurationCompletion, ConfigurationSpec,
};
fn error(error: webdock_mail::incoming::Error) -> Status {
    match error {
        webdock_mail::incoming::Error::Invalid => invalid(),
        webdock_mail::incoming::Error::Conflict => {
            Status::failed_precondition("Mail configuration authority changed")
        }
        webdock_mail::incoming::Error::Unavailable => {
            Status::unavailable("Mail configuration unavailable")
        }
    }
}
fn spec(bytes: &[u8], hash: &str) -> Result<ConfigurationSpec, Status> {
    let value: ConfigurationSpec = mail::parse(bytes, 262144)?;
    if value.fingerprint().map_err(|_| invalid())? != hash {
        return Err(invalid());
    }
    Ok(value)
}
impl ControlPlane {
    pub fn mail_configuration_service(
        self,
    ) -> wire::mail_configuration_control_server::MailConfigurationControlServer<Self> {
        wire::mail_configuration_control_server::MailConfigurationControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
}
#[tonic::async_trait]
impl wire::mail_configuration_control_server::MailConfigurationControl for ControlPlane {
    async fn poll_mail_configuration(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::MailConfigurationDispatch>, Status> {
        let identity = request.get_ref().identity.as_ref();
        let agent = self.authorize(&request, identity).await?;
        let Some(service) = &self.mail else {
            return Ok(Response::new(wire::MailConfigurationDispatch {
                command: None,
            }));
        };
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let command = service
            .claim_configuration(tx, &agent)
            .await
            .map_err(error)?
            .map(|claim| {
                claim.validate(chrono::Utc::now()).map_err(|_| invalid())?;
                Ok::<_, Status>(wire::MailConfigurationCommand {
                    request_sha256: claim.spec.fingerprint().map_err(|_| invalid())?,
                    spec_json: serde_json::to_vec(&claim.spec).map_err(|_| invalid())?,
                    lease_json: serde_json::to_vec(&claim.lease).map_err(|_| invalid())?,
                    lease_until: u64::try_from(claim.lease_until.timestamp())
                        .map_err(|_| invalid())?,
                    material_json: serde_json::to_vec(&claim.material).map_err(|_| invalid())?,
                    checkpoints_json: serde_json::to_vec(&claim.checkpoints)
                        .map_err(|_| invalid())?,
                })
            })
            .transpose()?;
        self.authorize(&request, identity).await?;
        Ok(Response::new(wire::MailConfigurationDispatch { command }))
    }
    async fn guard_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let lease: MailLease = mail::parse(&input.lease_json, 2048)?;
        let Some(service) = &self.mail else {
            return Ok(Response::new(wire::GuardReply { allowed: false }));
        };
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let allowed = match service.guard_configuration(tx, &agent, &spec, &lease).await {
            Ok(()) => true,
            Err(webdock_mail::incoming::Error::Conflict) => false,
            Err(failure) => return Err(error(failure)),
        };
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::GuardReply { allowed }))
    }
    async fn checkpoint_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let checkpoint: ConfigurationCheckpoint = mail::parse(&input.checkpoint_json, 4096)?;
        let service = self
            .mail
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Mail configuration unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .checkpoint_configuration(tx, &agent, &spec, checkpoint)
            .await
            .map_err(error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_mail_configuration(
        &self,
        request: Request<wire::MailConfigurationReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let completion: ConfigurationCompletion = mail::parse(&input.completion_json, 262144)?;
        let service = self
            .mail
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Mail configuration unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .complete_configuration(tx, &agent, &spec, completion)
            .await
            .map_err(error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
