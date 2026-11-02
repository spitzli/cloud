//! Native operator installation HTTP boundary and enrolled-agent adapter.
use super::*;
use webdock_agent_protocol::plugin::{
    PluginRuntimeCheckpoint, PluginRuntimeClaim, PluginRuntimeCompletion, PluginRuntimeLease,
    PluginRuntimeSpec,
};
pub mod worker;

pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/console/plugins/catalog", get(catalog))
        .route("/api/console/plugins/install", post(install))
        .route("/api/console/plugins/{plugin}/releases", get(history))
        .route("/api/console/plugins/{plugin}/update", post(update))
        .route("/api/console/plugins/{plugin}/rollback", post(rollback))
        .route("/api/console/plugins/operations/{id}/retry", post(retry))
}
async fn history(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(plugin): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let value = endpoints(&state)?
            .host
            .release_history_with_authority(&actor.principal, &actor.authority, &plugin)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn update(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(plugin): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let input: webdock_plugin_host::UpdateRequest =
            serde_json::from_value(json_body(&body, &headers)?).map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid update request.",
                )
            })?;
        let value = endpoints(&state)?
            .host
            .request_update_with_authority(&actor.principal, &actor.authority, &plugin, input)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn rollback(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(plugin): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let input: webdock_plugin_host::RollbackRequest =
            serde_json::from_value(json_body(&body, &headers)?).map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid rollback request.",
                )
            })?;
        let value = endpoints(&state)?
            .host
            .request_rollback_with_authority(&actor.principal, &actor.authority, &plugin, input)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn catalog(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let rows = endpoints(&state)?
            .host
            .catalog_with_authority(&actor.principal, &actor.authority)
            .await
            .map_err(failure)?;
        Ok(Json(rows).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn install(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let request: webdock_plugin_host::InstallRequest =
            serde_json::from_value(json_body(&body, &headers)?).map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid installation request.",
                )
            })?;
        let receipt = endpoints(&state)?
            .host
            .request_install_with_authority(&actor.principal, &actor.authority, request)
            .await
            .map_err(failure)?;
        Ok(Json(receipt).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryRequest {
    idempotency_key: String,
}
async fn retry(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let request: RetryRequest =
            serde_json::from_value(json_body(&body, &headers)?).map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid installation retry.",
                )
            })?;
        let receipt = endpoints(&state)?
            .host
            .retry_install_with_authority(
                &actor.principal,
                &actor.authority,
                &id,
                &request.idempotency_key,
            )
            .await
            .map_err(failure)?;
        Ok(Json(receipt).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

pub struct AgentBackend {
    pub host: Arc<HostService>,
}
fn status(error: PluginError) -> tonic::Status {
    match error.code {
        // Operation revocation must not revoke the healthy shared agent transport.
        ErrorCode::PermissionDenied => {
            tonic::Status::failed_precondition("Installation authority unavailable")
        }
        ErrorCode::InvalidInput => {
            tonic::Status::invalid_argument("Invalid installation execution contract")
        }
        ErrorCode::Conflict => tonic::Status::aborted("Installation execution changed"),
        _ => tonic::Status::unavailable("Installation execution unavailable"),
    }
}
#[tonic::async_trait]
impl webdock_agent_control::PluginRuntimeBackend for AgentBackend {
    fn supported_spec_version(&self) -> u32 {
        2
    }
    async fn poll(
        &self,
        agent: &webdock_hosting::AgentIdentity,
        max_spec_version: u32,
    ) -> Result<Option<PluginRuntimeClaim>, tonic::Status> {
        self.host
            .poll_plugin_runtime(agent, max_spec_version)
            .await
            .map_err(status)
    }
    async fn guard(
        &self,
        agent: &webdock_hosting::AgentIdentity,
        spec: &PluginRuntimeSpec,
        lease: &PluginRuntimeLease,
    ) -> Result<bool, tonic::Status> {
        self.host
            .guard_plugin_runtime(agent, spec, lease)
            .await
            .map_err(status)
    }
    async fn checkpoint(
        &self,
        agent: &webdock_hosting::AgentIdentity,
        spec: &PluginRuntimeSpec,
        lease: &PluginRuntimeLease,
        checkpoint: &PluginRuntimeCheckpoint,
    ) -> Result<bool, tonic::Status> {
        self.host
            .checkpoint_plugin_runtime(agent, spec, lease, checkpoint)
            .await
            .map_err(status)
    }
    async fn complete(
        &self,
        agent: &webdock_hosting::AgentIdentity,
        spec: &PluginRuntimeSpec,
        lease: &PluginRuntimeLease,
        completion: &PluginRuntimeCompletion,
    ) -> Result<bool, tonic::Status> {
        self.host
            .complete_plugin_runtime(agent, spec, lease, completion)
            .await
            .map_err(status)
    }
}
