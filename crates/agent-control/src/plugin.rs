//! Installation-runtime authority is supplied by Core; transport enrollment remains owned here.
use super::*;
use wire::plugin::{
    MAX_MATERIAL_BYTES, PluginRuntimeCheckpoint, PluginRuntimeClaim, PluginRuntimeCompletion,
    PluginRuntimeLease, PluginRuntimeSpec,
};

/// Core implements this adapter without introducing an agent-control -> plugin-host dependency.
/// Implementations retain operation/spec/actor fences and exact historical receipt deduplication.
#[tonic::async_trait]
pub trait PluginRuntimeBackend: Send + Sync {
    fn supported_spec_version(&self) -> u32 {
        1
    }
    async fn poll(
        &self,
        agent: &AgentIdentity,
        max_spec_version: u32,
    ) -> Result<Option<PluginRuntimeClaim>, Status>;
    async fn guard(
        &self,
        agent: &AgentIdentity,
        spec: &PluginRuntimeSpec,
        lease: &PluginRuntimeLease,
    ) -> Result<bool, Status>;
    async fn checkpoint(
        &self,
        agent: &AgentIdentity,
        spec: &PluginRuntimeSpec,
        lease: &PluginRuntimeLease,
        checkpoint: &PluginRuntimeCheckpoint,
    ) -> Result<bool, Status>;
    async fn complete(
        &self,
        agent: &AgentIdentity,
        spec: &PluginRuntimeSpec,
        lease: &PluginRuntimeLease,
        completion: &PluginRuntimeCompletion,
    ) -> Result<bool, Status>;
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T, Status> {
    if bytes.is_empty() || bytes.len() > max {
        return Err(invalid());
    }
    serde_json::from_slice(bytes).map_err(|_| invalid())
}
fn contract(
    spec_json: &[u8],
    lease_json: &[u8],
    hash: &str,
    identity: &wire::AgentIdentity,
) -> Result<(PluginRuntimeSpec, PluginRuntimeLease), Status> {
    let spec: PluginRuntimeSpec = parse(spec_json, 32768)?;
    let lease: PluginRuntimeLease = parse(lease_json, 2048)?;
    spec.validate().map_err(|_| invalid())?;
    lease.validate().map_err(|_| invalid())?;
    let actual = spec.fingerprint().map_err(|_| invalid())?;
    if actual.len() != hash.len()
        || !bool::from(actual.as_bytes().ct_eq(hash.as_bytes()))
        || spec.platform_id != identity.installation
        || lease.agent_generation != identity.generation
    {
        return Err(invalid());
    }
    Ok((spec, lease))
}
impl ControlPlane {
    pub fn with_plugin_runtime(mut self, backend: Arc<dyn PluginRuntimeBackend>) -> Self {
        self.plugin_runtime = Some(backend);
        self
    }
    pub fn plugin_runtime_service(
        self,
    ) -> wire::plugin_runtime_control_server::PluginRuntimeControlServer<Self> {
        wire::plugin_runtime_control_server::PluginRuntimeControlServer::new(self)
            .max_decoding_message_size(wire::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(wire::MAX_MESSAGE_BYTES)
    }
    fn plugin_backend(&self) -> Result<&Arc<dyn PluginRuntimeBackend>, Status> {
        self.plugin_runtime
            .as_ref()
            .ok_or_else(|| Status::unimplemented("Plugin runtime deployment unavailable"))
    }
}
#[tonic::async_trait]
impl wire::plugin_runtime_control_server::PluginRuntimeControl for ControlPlane {
    async fn poll_plugin_runtime(
        &self,
        request: Request<wire::PluginRuntimePollRequest>,
    ) -> Result<Response<wire::PluginRuntimeDispatch>, Status> {
        let identity = request.get_ref().identity.as_ref();
        let agent = self.authorize(&request, identity).await?;
        let backend = self.plugin_backend()?;
        let supported_spec_version =
            wire::plugin::normalize_spec_version(backend.supported_spec_version())
                .map_err(|_| invalid())?;
        let max_spec_version =
            wire::plugin::normalize_spec_version(request.get_ref().max_spec_version)
                .map_err(|_| invalid())?;
        let claim = backend
            .poll(&agent, max_spec_version.min(supported_spec_version))
            .await?;
        let command = claim
            .map(|claim| {
                claim.validate(now()).map_err(|_| invalid())?;
                if claim.spec.platform_id != identity.ok_or_else(invalid)?.installation
                    || claim.lease.agent_generation != identity.ok_or_else(invalid)?.generation
                    || claim.spec.cluster_id != agent.cluster_id()
                    || u32::from(claim.spec.version) > max_spec_version.min(supported_spec_version)
                {
                    return Err(invalid());
                }
                let command = wire::PluginRuntimeCommand {
                    spec_json: serde_json::to_vec(&claim.spec).map_err(|_| invalid())?,
                    lease_json: serde_json::to_vec(&claim.lease).map_err(|_| invalid())?,
                    request_sha256: claim.spec.fingerprint().map_err(|_| invalid())?,
                    material_json: serde_json::to_vec(&claim.material).map_err(|_| invalid())?,
                    checkpoint_json: claim
                        .checkpoint
                        .as_ref()
                        .map(serde_json::to_vec)
                        .transpose()
                        .map_err(|_| invalid())?
                        .unwrap_or_default(),
                };
                if command.spec_json.len() > 32768
                    || command.material_json.len() > MAX_MATERIAL_BYTES
                    || command.checkpoint_json.len() > 4096
                {
                    return Err(invalid());
                }
                Ok(command)
            })
            .transpose()?;
        self.authorize(&request, identity).await?;
        Ok(Response::new(wire::PluginRuntimeDispatch {
            command,
            supported_spec_version,
        }))
    }
    async fn guard_plugin_runtime(
        &self,
        request: Request<wire::PluginRuntimeGuardRequest>,
    ) -> Result<Response<wire::GuardReply>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let (spec, lease) = contract(
            &input.spec_json,
            &input.lease_json,
            &input.request_sha256,
            input.identity.as_ref().ok_or_else(invalid)?,
        )?;
        if spec.cluster_id != agent.cluster_id() {
            return Err(denied());
        }
        lease
            .current(now())
            .map_err(|_| Status::failed_precondition("Plugin runtime lease expired"))?;
        let allowed = self.plugin_backend()?.guard(&agent, &spec, &lease).await?;
        self.authorize(&request, input.identity.as_ref()).await?;
        lease
            .current(now())
            .map_err(|_| Status::failed_precondition("Plugin runtime lease expired"))?;
        Ok(Response::new(wire::GuardReply { allowed }))
    }
    async fn checkpoint_plugin_runtime(
        &self,
        request: Request<wire::PluginRuntimeCheckpointRequest>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let (spec, lease) = contract(
            &input.spec_json,
            &input.lease_json,
            &input.request_sha256,
            input.identity.as_ref().ok_or_else(invalid)?,
        )?;
        if spec.cluster_id != agent.cluster_id() {
            return Err(denied());
        }
        let checkpoint: PluginRuntimeCheckpoint = parse(&input.checkpoint_json, 4096)?;
        checkpoint.validate().map_err(|_| invalid())?;
        // Historical custody can be acknowledged after expiry, but must never renew execution.
        let accepted = self
            .plugin_backend()?
            .checkpoint(&agent, &spec, &lease, &checkpoint)
            .await?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted }))
    }
    async fn complete_plugin_runtime(
        &self,
        request: Request<wire::PluginRuntimeReceipt>,
    ) -> Result<Response<wire::Ack>, Status> {
        let input = request.get_ref();
        let agent = self.authorize(&request, input.identity.as_ref()).await?;
        let (spec, lease) = contract(
            &input.spec_json,
            &input.lease_json,
            &input.request_sha256,
            input.identity.as_ref().ok_or_else(invalid)?,
        )?;
        if spec.cluster_id != agent.cluster_id() {
            return Err(denied());
        }
        let completion: PluginRuntimeCompletion = parse(&input.completion_json, 8192)?;
        completion.validate().map_err(|_| invalid())?;
        let accepted = self
            .plugin_backend()?
            .complete(&agent, &spec, &lease, &completion)
            .await?;
        self.authorize(&request, input.identity.as_ref()).await?;
        Ok(Response::new(wire::Ack { accepted }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_or_secret_bearing_frames_fail_before_backend_dispatch() {
        assert!(parse::<PluginRuntimeLease>(br#"{"secret":"private"}"#, 2048).is_err());
        assert!(parse::<PluginRuntimeCheckpoint>(&vec![b' '; 4097], 4096).is_err());
        assert!(
            parse::<PluginRuntimeCompletion>(
                br#"{"outcome":"succeeded","checkpoint":{},"converged":true,"files":{}}"#,
                8192
            )
            .is_err()
        );
    }
}
