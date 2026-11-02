//! Database RPCs delegate to the retained database authority within the same authenticated transaction.
use super::*;
use wire::database::{DatabaseCheckpoint, DatabaseCompletion, DatabaseJobSpec, DatabaseLease};
fn domain_error(error: webdock_databases::Error) -> Status {
    match error.status {
        401 => denied(),
        403 | 409 => Status::failed_precondition("Database operation authority changed"),
        400 => invalid(),
        404 => Status::not_found("Database operation unavailable"),
        429 => Status::resource_exhausted("Database operation limit"),
        _ => Status::unavailable("Database operation unavailable"),
    }
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T, Status> {
    if bytes.is_empty() || bytes.len() > max {
        return Err(invalid());
    }
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn spec(bytes: &[u8], hash: &str) -> Result<DatabaseJobSpec, Status> {
    let spec: DatabaseJobSpec = parse(bytes, 65536)?;
    spec.validate().map_err(|_| invalid())?;
    let actual = spec.fingerprint().map_err(|_| invalid())?;
    if !bool::from(actual.as_bytes().ct_eq(hash.as_bytes())) {
        return Err(invalid());
    }
    Ok(spec)
}
#[tonic::async_trait]
impl wire::database_control_server::DatabaseControl for ControlPlane {
    async fn database_observation_config(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::DatabaseObservationPlan>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let service = self
            .databases
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Database execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let value = service
            .database_observation_config(tx, &agent)
            .await
            .map_err(domain_error)?;
        let configuration_json = serde_json::to_vec(&value).map_err(|_| invalid())?;
        if configuration_json.len() > 320 * 1024 {
            return Err(Status::resource_exhausted(
                "Database observation inventory limit",
            ));
        }
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::DatabaseObservationPlan {
            configuration_json,
        }))
    }
    async fn report_database_observations(
        &self,
        request: Request<wire::DatabaseObservationRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let value: Value = parse(&input.observation_json, 320 * 1024)?;
        let service = self
            .databases
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Database execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let value = service
            .report_database_observations(tx, &agent, value)
            .await
            .map_err(domain_error)?;
        if value["accepted"] != true {
            return Err(Status::failed_precondition("Database observation rejected"));
        }
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn poll_database(
        &self,
        request: Request<wire::PollRequest>,
    ) -> Result<Response<wire::DatabaseDispatch>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let service = self
            .databases
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Database execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let claim = service
            .claim_database_operation(tx, &agent)
            .await
            .map_err(domain_error)?;
        let command = if let Some(claim) = claim {
            claim
                .validate(chrono::Utc::now())
                .map_err(|_| Status::failed_precondition("Database lease expired"))?;
            let spec_json = serde_json::to_vec(&claim.spec).map_err(|_| invalid())?;
            let lease_json = serde_json::to_vec(&claim.lease).map_err(|_| invalid())?;
            let material_json = serde_json::to_vec(&claim.material).map_err(|_| invalid())?;
            if spec_json.len() > 65536 || lease_json.len() > 2048 || material_json.len() > 65536 {
                return Err(invalid());
            }
            Some(wire::DatabaseCommand {
                request_sha256: claim.spec.fingerprint().map_err(|_| invalid())?,
                spec_json,
                lease_json,
                lease_until: u64::try_from(claim.lease_until.timestamp()).map_err(|_| invalid())?,
                material_json,
            })
        } else {
            None
        };
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::DatabaseDispatch { command }))
    }
    async fn guard_database(
        &self,
        request: Request<wire::DatabaseGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let lease: DatabaseLease = parse(&input.lease_json, 2048)?;
        lease.validate().map_err(|_| invalid())?;
        let service = self
            .databases
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Database execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .validate_database_callback_in(
                &tx,
                &agent,
                &spec,
                &lease,
                webdock_databases::DatabaseCallback::Guard,
            )
            .await
            .map_err(domain_error)?;
        service
            .guard_database_operation(tx, &agent, &lease)
            .await
            .map_err(domain_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::GuardReply { allowed: true }))
    }
    async fn checkpoint_database(
        &self,
        request: Request<wire::DatabaseCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let checkpoint: DatabaseCheckpoint = parse(&input.checkpoint_json, 320 * 1024)?;
        checkpoint.validate().map_err(|_| invalid())?;
        let service = self
            .databases
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Database execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        service
            .receive_database_checkpoint(tx, &agent, &spec, checkpoint)
            .await
            .map_err(domain_error)?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
    async fn complete_database(
        &self,
        request: Request<wire::DatabaseReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let spec = spec(&input.spec_json, &input.request_sha256)?;
        let receipt: DatabaseCompletion = parse(&input.completion_json, 65536)?;
        receipt.validate().map_err(|_| invalid())?;
        let service = self
            .databases
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("Database execution unavailable"))?;
        let tx = self
            .hosting
            .begin_authenticated_agent(&agent)
            .await
            .map_err(hosting_error)?;
        let result = service
            .receive_database_completion(tx, &agent, &spec, receipt)
            .await
            .map_err(domain_error)?;
        if result["accepted"] != true {
            return Err(Status::failed_precondition(
                "Database receipt was not accepted",
            ));
        }
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted: true }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn database_payloads_are_bounded_and_never_accept_unknown_lease_fields() {
        assert!(parse::<DatabaseLease>(&vec![b' '; 2049], 2048).is_err());
        assert!(
            parse::<DatabaseLease>(
                br#"{"operationID":"123","generation":1,"leaseToken":"a","sql":"select 1"}"#,
                2048
            )
            .is_err()
        );
        assert!(spec(b"{}", &"a".repeat(64)).is_err());
    }
}
