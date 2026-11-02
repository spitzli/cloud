//! Private authenticated transport for the existing persisted HostService capabilities.
use crate::*;
use serde_json::Value;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use webdock_plugin_protocol::{InvocationClaims, Result};
use webdock_plugin_runtime::{InvocationAuthority, authenticated_peer, wire};
pub struct HostRpc {
    host: Arc<HostService>,
    permits: tokio::sync::Semaphore,
}
impl HostService {
    pub fn rpc(self: Arc<Self>) -> wire::plugin_host_server::PluginHostServer<HostRpc> {
        wire::plugin_host_server::PluginHostServer::new(HostRpc {
            host: self,
            permits: tokio::sync::Semaphore::new(64),
        })
        .max_decoding_message_size(1024 * 1024 + 32768)
        .max_encoding_message_size(1024 * 1024 + 1024)
    }
}
fn failure(error: PluginError) -> Status {
    let code = match error.code {
        ErrorCode::InvalidInput => tonic::Code::InvalidArgument,
        ErrorCode::PermissionDenied => tonic::Code::PermissionDenied,
        ErrorCode::Conflict => tonic::Code::Aborted,
        ErrorCode::NotFound => tonic::Code::NotFound,
        ErrorCode::CapabilityUnavailable | ErrorCode::NotSupported => tonic::Code::Unimplemented,
        ErrorCode::DeadlineExceeded => tonic::Code::DeadlineExceeded,
        ErrorCode::Unavailable => tonic::Code::Unavailable,
    };
    let message = webdock_plugin_protocol::public_message(error.code, &error.message)
        .unwrap_or_else(|| "Plugin host request denied".into());
    Status::new(code, message)
}
fn claims(bytes: &[u8]) -> Result<InvocationClaims> {
    if bytes.len() > 16384 {
        return Err(invalid("Invalid invocation"));
    }
    serde_json::from_slice(bytes).map_err(|_| invalid("Invalid invocation"))
}
fn json(bytes: &[u8], limit: usize) -> Result<Value> {
    if bytes.len() > limit {
        return Err(invalid("Host request limit"));
    }
    serde_json::from_slice(bytes).map_err(|_| invalid("Invalid host request"))
}
#[tonic::async_trait]
impl wire::plugin_host_server::PluginHost for HostRpc {
    async fn authorize_peer(
        &self,
        request: Request<wire::HostPeerRequest>,
    ) -> std::result::Result<Response<wire::HostAuthorityResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Plugin host is busy"))?;
        let peer = authenticated_peer(&request)?;
        let input = request.into_inner();
        self.host
            .runtime_authority(input.plugin_id, input.generation)
            .map_err(failure)?
            .authorize_peer(&peer)
            .await
            .map_err(failure)?;
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn authorize_invocation(
        &self,
        request: Request<wire::HostInvocationRequest>,
    ) -> std::result::Result<Response<wire::HostAuthorityResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Plugin host is busy"))?;
        let peer = authenticated_peer(&request)?;
        let claims = claims(&request.into_inner().claims_json).map_err(failure)?;
        self.host
            .runtime_authority(claims.plugin_id.clone(), claims.generation)
            .map_err(failure)?
            .authorize_invocation(&peer, &claims)
            .await
            .map_err(failure)?;
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn authorize_request(
        &self,
        request: Request<wire::HostPayloadRequest>,
    ) -> std::result::Result<Response<wire::HostAuthorityResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Plugin host is busy"))?;
        let peer = authenticated_peer(&request)?;
        let request = request.into_inner();
        let claims = claims(&request.claims_json).map_err(failure)?;
        if request.input_json.len()
            > crate::authority::invocation_input_limit(
                &claims.plugin_id,
                &claims.action,
                claims.job.is_some(),
            )
            || request.configuration_json.len() > 65536
        {
            return Err(Status::resource_exhausted("Invocation payload limit"));
        }
        self.host
            .runtime_authority(claims.plugin_id.clone(), claims.generation)
            .map_err(failure)?
            .authorize_request(
                &peer,
                &claims,
                &request.input_json,
                &request.configuration_json,
            )
            .await
            .map_err(failure)?;
        Ok(Response::new(wire::HostAuthorityResponse { active: true }))
    }
    async fn workloads(
        &self,
        request: Request<wire::HostWorkloadRequest>,
    ) -> std::result::Result<Response<wire::HostCallResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Plugin host is busy"))?;
        let peer = authenticated_peer(&request)?;
        let request = request.into_inner();
        let input = json(&request.input_json, 262144 + 4096).map_err(failure)?;
        let value = self
            .host
            .workloads(
                &peer,
                &request.plugin_id,
                request.generation,
                &request.method,
                input,
            )
            .await
            .map_err(failure)?;
        let bytes =
            serde_json::to_vec(&value).map_err(|_| Status::unavailable("Invalid host result"))?;
        Ok(Response::new(wire::HostCallResponse { output_json: bytes }))
    }
    async fn call(
        &self,
        request: Request<wire::HostCallRequest>,
    ) -> std::result::Result<Response<wire::HostCallResponse>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Plugin host is busy"))?;
        let peer = authenticated_peer(&request)?;
        let request = request.into_inner();
        let limit = match (request.service.as_str(), request.method.as_str()) {
            ("data", "read") | ("apps", "request") => 32768,
            ("jobs", "enqueue") => 36864,
            ("jobs", "get" | "cancel") => 256,
            ("data", "write") => 131072,
            ("http", "request") => 1048576,
            _ => return Err(Status::unimplemented("Host capability unavailable")),
        };
        let input = json(&request.input_json, limit).map_err(failure)?;
        let handle = self
            .host
            .authenticate(peer, &request.invocation_token)
            .await
            .map_err(failure)?;
        let value = match (request.service.as_str(), request.method.as_str()) {
            ("data", "read") => {
                if input.as_object().is_none_or(|v| v.len() != 1) {
                    return Err(Status::invalid_argument("Invalid document read"));
                }
                let key = input["key"]
                    .as_str()
                    .ok_or_else(|| Status::invalid_argument("Invalid document key"))?;
                self.host.data_read(&handle, key).await
            }
            ("data", "write") => self.host.data_write(&handle, input).await,
            ("http", "request") => self
                .host
                .http_request(&handle, input)
                .await
                .and_then(|value| serde_json::to_value(value).map_err(|_| unavailable())),
            ("apps", "request") => self.host.apps_request(&handle, input).await,
            ("jobs", "enqueue") => self.host.jobs_enqueue(&handle, input).await,
            ("jobs", "get") => self.host.jobs_get(&handle, input).await,
            ("jobs", "cancel") => self.host.jobs_cancel(&handle, input).await,
            _ => unreachable!(),
        }
        .map_err(failure)?;
        let bytes =
            serde_json::to_vec(&value).map_err(|_| Status::unavailable("Invalid host result"))?;
        if bytes.len() > 1048576 {
            return Err(Status::resource_exhausted("Host result limit"));
        }
        Ok(Response::new(wire::HostCallResponse { output_json: bytes }))
    }
}
