//! Native plugin browser adapters; discovery metadata never grants execution authority.
use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject},
    plugin_config::RuntimeConnections,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, RawQuery, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spitzli_plugin_host::{AdmissionInput, HostService};
use spitzli_plugin_protocol::{ErrorCode, Manifest, PluginError};
use std::sync::Arc;
#[path = "plugin_builtins.rs"]
mod builtins;
#[path = "plugin_jobs.rs"]
mod jobs;
#[path = "plugin_storage.rs"]
mod storage;
#[derive(Clone, Copy)]
enum InvokeBoundary {
    View,
    Route,
    Admit,
    Replay,
    Connect,
    Authenticate,
    Health,
    Describe,
    Invoke,
    Decode,
    Reload,
    Revalidate,
    Resource,
    Result,
    Revoke,
    Complete,
}
impl InvokeBoundary {
    fn token(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Route => "route",
            Self::Admit => "admit",
            Self::Replay => "replay",
            Self::Connect => "connect",
            Self::Authenticate => "authenticate",
            Self::Health => "health",
            Self::Describe => "describe",
            Self::Invoke => "invoke",
            Self::Decode => "decode",
            Self::Reload => "reload",
            Self::Revalidate => "revalidate",
            Self::Resource => "resource",
            Self::Result => "result",
            Self::Revoke => "revoke",
            Self::Complete => "complete",
        }
    }
}
fn invoke_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidInput => "invalid_input",
        ErrorCode::NotSupported => "not_supported",
        ErrorCode::PermissionDenied => "permission_denied",
        ErrorCode::CapabilityUnavailable => "capability_unavailable",
        ErrorCode::Conflict => "conflict",
        ErrorCode::Unavailable => "unavailable",
        ErrorCode::DeadlineExceeded => "deadline_exceeded",
        ErrorCode::NotFound => "not_found",
    }
}
struct InvokeTrace {
    action: Option<&'static str>,
}
impl InvokeTrace {
    fn new(plugin: &str, action: &str) -> Self {
        Self {
            action: if plugin == "mail" {
                match action {
                    "domains.create" => Some("domains.create"),
                    "mail.send" => Some("mail.send"),
                    "incoming.activate" => Some("incoming.activate"),
                    "mailboxes.create" => Some("mailboxes.create"),
                    "mailboxes.view" => Some("mailboxes.view"),
                    "mailboxes.credentials" => Some("mailboxes.credentials"),
                    _ => None,
                }
            } else {
                None
            },
        }
    }
    fn enabled(value: Option<&std::ffi::OsStr>) -> bool {
        value == Some(std::ffi::OsStr::new("1"))
    }
    fn line(
        &self,
        stage: InvokeBoundary,
        event: &'static str,
        code: Option<ErrorCode>,
        tonic: Option<i32>,
        errno: Option<i32>,
    ) -> Option<String> {
        let action = self.action?;
        let number = |v: Option<i32>| v.map_or_else(|| "null".to_owned(), |v| v.to_string());
        Some(format!(
            "plugin_invoke action={action} stage={} event={event} publicCode={} tonicCode={} ioErrno={}",
            stage.token(),
            code.map_or("none", invoke_code),
            number(tonic),
            number(errno)
        ))
    }
    fn emit(
        &self,
        stage: InvokeBoundary,
        event: &'static str,
        code: Option<ErrorCode>,
        tonic: Option<i32>,
        errno: Option<i32>,
    ) {
        if Self::enabled(std::env::var_os("SPITZLI_RECOVERY_COMPONENT_TEST").as_deref())
            && let Some(line) = self.line(stage, event, code, tonic, errno)
        {
            use std::io::Write;
            let _ = writeln!(std::io::stderr().lock(), "{line}");
        }
    }
    fn enter(&self, stage: InvokeBoundary) {
        self.emit(stage, "enter", None, None, None)
    }
    fn host(&self, stage: InvokeBoundary, error: &PluginError) {
        self.emit(stage, "error", Some(error.code), None, None)
    }
    fn rpc(&self, stage: InvokeBoundary, error: &tonic::Status) {
        self.emit(stage, "error", None, Some(error.code() as i32), None)
    }
    fn transport_numbers(error: &(dyn std::error::Error + 'static)) -> (Option<i32>, Option<i32>) {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (mut code, mut errno) = (None, None);
            let mut current = Some(error);
            for _ in 0..8 {
                let Some(error) = current else { break };
                if code.is_none() {
                    code = error
                        .downcast_ref::<tonic::Status>()
                        .map(|e| e.code() as i32);
                }
                if errno.is_none() {
                    errno = error
                        .downcast_ref::<std::io::Error>()
                        .and_then(std::io::Error::raw_os_error);
                }
                current = error
                    .downcast_ref::<std::io::Error>()
                    .and_then(|e| e.get_ref().map(|e| e as &(dyn std::error::Error + 'static)))
                    .or_else(|| error.source());
            }
            (code, errno)
        }))
        .unwrap_or((None, None))
    }
    fn transport(&self, stage: InvokeBoundary, error: &(dyn std::error::Error + 'static)) {
        let (code, errno) = Self::transport_numbers(error);
        self.emit(stage, "error", None, code, errno)
    }

    fn outcome<T>(&self, stage: InvokeBoundary, result: &Result<T, PluginError>) {
        match result {
            Ok(_) => self.emit(stage, "ok", None, None, None),
            Err(error) => self.host(stage, error),
        }
    }
}
pub struct PluginEndpoints {
    pub exports: Option<Arc<dyn spitzli_plugin_host::ExportSources>>,
    pub host: Arc<HostService>,
    pub connections: RuntimeConnections,
    pub repository: Option<Arc<spitzli_build::RepositoryAdmission>>,
    pub mcp_client: Option<crate::plugin_config::McpClientConfig>,
}
impl PluginEndpoints {
    pub fn new(
        host: Arc<HostService>,
        connections: RuntimeConnections,
        repository: Option<Arc<spitzli_build::RepositoryAdmission>>,
    ) -> Self {
        Self {
            exports: None,
            host,
            connections,
            repository,
            mcp_client: None,
        }
    }
    pub async fn invoke(
        &self,
        actor: &auth::AuthenticatedActor,
        input: AdmissionInput,
    ) -> Result<Value, PluginError> {
        self.invoke_as(actor, input, !actor.read_only()).await
    }
    /// As `invoke`, with writes allowed only when `can_write` (an API token may only read).
    pub async fn invoke_as(
        &self,
        actor: &auth::AuthenticatedActor,
        input: AdmissionInput,
        can_write: bool,
    ) -> Result<Value, PluginError> {
        self.invoke_as_storage(actor, input, can_write, None).await
    }
    pub async fn invoke_api(
        &self,
        actor: &auth::AuthenticatedActor,
        input: AdmissionInput,
        can_write: bool,
        api_hash: String,
    ) -> Result<Value, PluginError> {
        // API token revocation is not part of Browser StoredAuthority. Fail closed for this adapter.
        if input.plugin_id == "neon" {
            return Err(PluginError::denied());
        }
        self.invoke_as_storage(actor, input, can_write, Some(api_hash))
            .await
    }
    async fn invoke_as_storage(
        &self,
        actor: &auth::AuthenticatedActor,
        input: AdmissionInput,
        can_write: bool,
        api_hash: Option<String>,
    ) -> Result<Value, PluginError> {
        let trace = InvokeTrace::new(&input.plugin_id, &input.action);
        trace.enter(InvokeBoundary::View);
        let view = self
            .host
            .project_plugins_with_authority(&actor.principal, &input.project_id, &actor.authority)
            .await
            .inspect_err(|e| trace.host(InvokeBoundary::View, e))?
            .into_iter()
            .find(|v| v.manifest["name"] == input.plugin_id)
            .ok_or_else(|| PluginError::new(ErrorCode::NotFound, "Plugin is unavailable"))?;
        trace.enter(InvokeBoundary::Route);
        let connection = self
            .connections
            .load_generation(&input.plugin_id, view.generation)
            .await
            .map_err(|e| {
                trace.transport(InvokeBoundary::Route, e.as_ref());
                unavailable_plugin()
            })?;
        if connection.generation != view.generation {
            return Err(PluginError::denied());
        }
        let plugin = input.plugin_id.clone();
        trace.enter(InvokeBoundary::Admit);
        let admitted = self
            .host
            .admit_with_authority(&actor.principal, input, can_write, &actor.authority)
            .await
            .inspect_err(|e| trace.host(InvokeBoundary::Admit, e))?;
        self.invoke_admitted(
            plugin,
            view,
            connection,
            admitted,
            if api_hash.is_some() {
                "/v1/api/storage"
            } else {
                "/api/console/plugins/storage"
            },
            api_hash,
        )
        .await
    }
    pub async fn invoke_mcp(
        &self,
        actor: &spitzli_oauth::McpAuthentication,
        input: AdmissionInput,
    ) -> Result<Value, PluginError> {
        let view = self
            .host
            .project_plugins_with_mcp(&actor.principal, &input.project_id, &actor.authority)
            .await?
            .into_iter()
            .find(|v| v.manifest["name"] == input.plugin_id)
            .ok_or_else(PluginError::denied)?;
        let plugin = input.plugin_id.clone();
        let connection = self
            .connections
            .load_generation(&plugin, view.generation)
            .await
            .map_err(|_| unavailable_plugin())?;
        if connection.generation != view.generation {
            return Err(PluginError::denied());
        }
        let admitted = self
            .host
            .admit_with_mcp_authority(
                &actor.principal,
                input,
                actor.authority.can_write(),
                &actor.authority,
            )
            .await?;
        self.invoke_admitted(
            plugin,
            view,
            connection,
            admitted,
            "/v1/plugins/storage",
            None,
        )
        .await
    }
    async fn invoke_admitted(
        &self,
        plugin: String,
        view: spitzli_plugin_host::PluginView,
        connection: crate::plugin_config::RuntimeConnection,
        admitted: spitzli_plugin_host::AdmittedInvocation,
        storage_path: &str,
        source_api_hash: Option<String>,
    ) -> Result<Value, PluginError> {
        let trace = InvokeTrace::new(&plugin, &admitted.claims.action);
        let result = async {
            trace.enter(InvokeBoundary::Replay);
            if let Some(result) = &admitted.replay_result {
                return Ok(result.clone());
            }
            let payload = serde_json::to_vec(&admitted.input).map_err(|_| PluginError::denied())?;
            trace.enter(InvokeBoundary::Connect);
            let (mut client, peer) = connection.connect_authenticated().await.map_err(|e| {
                trace.transport(InvokeBoundary::Connect, e.as_ref());
                unavailable_plugin()
            })?;
            trace.enter(InvokeBoundary::Authenticate);
            let handle = self
                .host
                .authenticate(peer, &admitted.token)
                .await
                .inspect_err(|e| trace.host(InvokeBoundary::Authenticate, e))?;
            trace.enter(InvokeBoundary::Health);
            let health = client
                .health(spitzli_plugin_runtime::wire::HealthRequest {})
                .await
                .map_err(|e| {
                    trace.rpc(InvokeBoundary::Health, &e);
                    unavailable_plugin()
                })?
                .into_inner();
            if health.plugin_id != plugin
                || health.version != view.manifest["version"].as_str().unwrap_or("")
                || health.generation != admitted.claims.generation
                || health.protocol_version != 1
            {
                return Err(PluginError::denied());
            }
            trace.enter(InvokeBoundary::Describe);
            let described = client
                .describe(spitzli_plugin_runtime::wire::DescribeRequest {})
                .await
                .map_err(|e| {
                    trace.rpc(InvokeBoundary::Describe, &e);
                    unavailable_plugin()
                })?
                .into_inner();
            if hex::encode(Sha256::digest(&described.manifest_json)) != view.manifest_sha256 {
                return Err(PluginError::denied());
            }
            let manifest = Manifest::parse(&described.manifest_json)?;
            if plugin == "mail"
                && admitted.claims.plugin_id == "mail"
                && admitted.claims.action == "mail.send"
                && admitted.claims.job.is_none()
            {
                client = client.max_encoding_message_size(224 * 1024);
            }
            trace.enter(InvokeBoundary::Invoke);
            let reply = client
                .invoke(spitzli_plugin_runtime::wire::InvokeRequest {
                    invocation_token: admitted.token.clone(),
                    action: admitted.claims.action.clone(),
                    input_json: payload,
                    configuration_json: serde_json::to_vec(&admitted.configuration)
                        .map_err(|_| unavailable_plugin())?,
                })
                .await
                .map_err(|status| {
                    trace.rpc(InvokeBoundary::Invoke, &status);
                    invocation_failure(status)
                })?
                .into_inner();
            trace.enter(InvokeBoundary::Decode);
            if reply.output_json.len() > 65536 {
                return Err(unavailable_plugin());
            }
            let result =
                serde_json::from_slice(&reply.output_json).map_err(|_| unavailable_plugin())?;
            let result = if manifest
                .action(&admitted.claims.action)
                .is_some_and(|a| a.get("storage").is_some())
            {
                let plan: spitzli_plugin_protocol::storage::StoragePlan =
                    serde_json::from_value(result).map_err(|_| PluginError::denied())?;
                let mut descriptor = self
                    .host
                    .retain_storage_plan_with_api(
                        &handle,
                        plan,
                        storage::transport_hash(&connection)?,
                        source_api_hash.clone(),
                    )
                    .await?;
                descriptor.path = format!("{storage_path}/{}", descriptor.id);
                serde_json::to_value(descriptor).map_err(|_| unavailable_plugin())?
            } else {
                result
            };
            spitzli_plugin_protocol::validate_value(
                manifest
                    .action(&admitted.claims.action)
                    .and_then(|a| a.get("output")),
                &result,
            )?;
            trace.enter(InvokeBoundary::Reload);
            if self
                .connections
                .load_generation(&plugin, view.generation)
                .await
                .map_err(|e| {
                    trace.transport(InvokeBoundary::Reload, e.as_ref());
                    unavailable_plugin()
                })?
                != connection
            {
                return Err(PluginError::denied());
            }
            trace.enter(InvokeBoundary::Revalidate);
            self.host
                .revalidate(&handle)
                .await
                .inspect_err(|e| trace.host(InvokeBoundary::Revalidate, e))?;
            // Create/delete actions update the Core resource index before the token is revoked.
            if manifest
                .action(&admitted.claims.action)
                .is_some_and(|a| a.get("resource").is_some_and(|r| r != "use"))
            {
                trace.enter(InvokeBoundary::Resource);
                self.host
                    .record_resource(&handle, &result)
                    .await
                    .inspect_err(|e| trace.host(InvokeBoundary::Resource, e))?;
            }
            Ok(result)
        }
        .await;
        trace.outcome(InvokeBoundary::Result, &result);
        trace.enter(InvokeBoundary::Revoke);
        let revoked = self.host.revoke_issued(&admitted.token).await;
        trace.outcome(InvokeBoundary::Revoke, &revoked);
        revoked?;
        trace.enter(InvokeBoundary::Complete);
        result
    }
}
fn invocation_failure(status: tonic::Status) -> PluginError {
    match status.code() {
        tonic::Code::PermissionDenied | tonic::Code::Unauthenticated => PluginError::denied(),
        tonic::Code::InvalidArgument => PluginError::new(
            ErrorCode::InvalidInput,
            spitzli_plugin_protocol::public_message(ErrorCode::InvalidInput, status.message())
                .unwrap_or_else(|| "Plugin request is invalid".into()),
        ),
        tonic::Code::Aborted => PluginError::new(
            ErrorCode::Conflict,
            spitzli_plugin_protocol::public_message(ErrorCode::Conflict, status.message())
                .unwrap_or_else(|| "Plugin operation conflict".into()),
        ),
        tonic::Code::Unimplemented => PluginError::new(
            ErrorCode::CapabilityUnavailable,
            "Plugin capability unavailable",
        ),
        _ => unavailable_plugin(),
    }
}

fn unavailable_plugin() -> PluginError {
    PluginError::new(ErrorCode::Unavailable, "Plugin runtime is unavailable")
}
fn failure(error: PluginError) -> ApiFailure {
    let status = match error.code {
        ErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
        ErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let code = serde_json::to_value(error.code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unavailable".into());
    reject(status, &code, &error.message)
}
fn endpoints(state: &ServiceState) -> Result<&Arc<PluginEndpoints>, ApiFailure> {
    state
        .plugins
        .as_ref()
        .ok_or_else(|| failure(unavailable_plugin()))
}
async fn actor(
    state: &ServiceState,
    headers: &HeaderMap,
) -> Result<auth::AuthenticatedActor, ApiFailure> {
    auth::resolve_actor(state, headers).await?.ok_or_else(|| {
        reject(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Sign in to continue.",
        )
    })
}
const MAIL_ENVELOPE_BYTES: usize = 160 * 1024;
fn json_body(body: &[u8], headers: &HeaderMap) -> Result<Value, ApiFailure> {
    json_body_limit(body, headers, 65536)
}
fn json_body_limit(body: &[u8], headers: &HeaderMap, limit: usize) -> Result<Value, ApiFailure> {
    if body.len() > limit {
        return Err(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Plugin request exceeds limit.",
        ));
    }
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return Err(reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_input",
            "JSON is required.",
        ));
    }
    serde_json::from_slice(body).map_err(|_| {
        reject(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Invalid plugin request.",
        )
    })
}
// The caller authenticates before buffering this single Console invocation envelope.
fn invoke_input(body: &[u8], headers: &HeaderMap) -> Result<AdmissionInput, ApiFailure> {
    let input: AdmissionInput =
        serde_json::from_value(json_body_limit(body, headers, MAIL_ENVELOPE_BYTES)?).map_err(
            |_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid plugin command.",
                )
            },
        )?;
    let mail = input.plugin_id == "mail" && input.action == "mail.send";
    if body.len() > 65536 && !mail
        || mail
            && serde_json::to_vec(&input.input)
                .map_err(|_| {
                    reject(
                        StatusCode::BAD_REQUEST,
                        "invalid_input",
                        "Invalid plugin command.",
                    )
                })?
                .len()
                > spitzli_plugin_host::MAIL_ACTION_INPUT_BYTES
    {
        return Err(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Plugin request exceeds limit.",
        ));
    }
    Ok(input)
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .merge(api::routes())
        .merge(storage::routes())
        .merge(exposure::routes())
        .merge(operations::routes())
        .merge(installation::routes())
        .merge(builtins::routes())
        .route("/api/console/plugins", get(list))
        .route("/api/console/plugins/invoke", post(invoke))
        .route("/api/console/plugins/installed", get(installed).post(admin))
        .route("/api/console/plugins/{id}/self-service", get(self_service))
        .route("/api/console/plugins/{id}/resources", get(plugin_resources))
        .route(
            "/api/console/plugins/{id}/self-service/features/{feature}",
            put(set_feature_mode),
        )
        .route(
            "/api/console/plugins/{id}/self-service/features/{feature}/customers/{customer}",
            put(set_feature_customer),
        )
        .route(
            "/api/console/customers/{id}/self-service",
            get(customer_self_service),
        )
        .route("/api/console/plans", get(plans).post(create_plan))
        .route(
            "/api/console/plans/{id}",
            put(save_plan).delete(delete_plan),
        )
        .route("/api/console/customers/{id}/plan", put(customer_plan))
        .route("/api/console/customers/{id}/usage", get(usage))
        .route(
            "/api/console/customers/{id}/notifications",
            get(notifications),
        )
        .route(
            "/api/console/projects/{project}/plugins/{plugin}/resources/{resource}/logs",
            get(logs),
        )
        .route(
            "/api/console/projects/{project}/placement",
            get(placement).put(place),
        )
        .route("/api/console/clusters", get(clusters))
        .route("/api/console/clusters/{id}", put(offer_cluster))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModeCommand {
    mode: String,
    expected_revision: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CustomerCommand {
    allowed: bool,
    expected_revision: String,
}
fn writer(actor: &auth::AuthenticatedActor) -> Result<(), ApiFailure> {
    if actor.read_only() {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Write authority is required.",
        ));
    }
    Ok(())
}
fn command<T: serde::de::DeserializeOwned>(
    body: &[u8],
    headers: &HeaderMap,
) -> Result<T, ApiFailure> {
    serde_json::from_value(json_body(body, headers)?).map_err(|_| {
        reject(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Check the request and try again.",
        )
    })
}
async fn plugin_resources(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let host = &endpoints(&state)?.host;
        let value = host
            .plugin_resources(&actor.principal, Some(&actor.authority), &id)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn self_service(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let host = &endpoints(&state)?.host;
        let value = host
            .self_service(&actor.principal, Some(&actor.authority), &id)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn set_feature_mode(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((id, feature)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let input: ModeCommand = command(&body, &headers)?;
        let host = &endpoints(&state)?.host;
        let value = host
            .set_feature_mode(
                &actor.principal,
                Some(&actor.authority),
                &id,
                &feature,
                &input.mode,
                &input.expected_revision,
            )
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn set_feature_customer(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((id, feature, customer)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let input: CustomerCommand = command(&body, &headers)?;
        let host = &endpoints(&state)?.host;
        let value = host
            .set_feature_customer(
                &actor.principal,
                Some(&actor.authority),
                &id,
                &feature,
                &customer,
                input.allowed,
                &input.expected_revision,
            )
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn customer_self_service(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let host = &endpoints(&state)?.host;
        let value = host
            .customer_self_service(&actor.principal, Some(&actor.authority), &id)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn list(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let parameters = url::form_urlencoded::parse(query.as_deref().unwrap_or("").as_bytes())
            .collect::<Vec<_>>();
        if parameters.len() != 1 || parameters[0].0 != "projectID" {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "A project is required.",
            ));
        }
        let service = endpoints(&state)?;
        let views = service
            .host
            .project_plugins_with_authority(&actor.principal, &parameters[0].1, &actor.authority)
            .await
            .map_err(failure)?;
        let mut output = Vec::new();
        for view in views {
            let plugin = view.manifest["name"]
                .as_str()
                .ok_or_else(|| failure(unavailable_plugin()))?;
            let mut value =
                serde_json::to_value(&view).map_err(|_| failure(unavailable_plugin()))?;
            if let Ok(binding) = service
                .connections
                .load_generation(plugin, view.generation)
                .await
                && binding.generation == view.generation
            {
                value["uiOrigin"] = json!(binding.ui_origin);
            }
            output.push(value);
        }
        Ok(Json(output).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn invoke(State(state): State<Arc<ServiceState>>, request: Request) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let headers = request.headers().clone();
        let actor = actor(&state, &headers).await?;
        let body = axum::body::to_bytes(request.into_body(), MAIL_ENVELOPE_BYTES)
            .await
            .map_err(|_| {
                reject(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "invalid_input",
                    "Plugin request exceeds limit.",
                )
            })?;
        let input = invoke_input(&body, &headers)?;
        Ok(Json(
            endpoints(&state)?
                .invoke(&actor, input)
                .await
                .map_err(failure)?,
        )
        .into_response())
    }
    .await;
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
async fn installed(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        Ok(Json(
            endpoints(&state)?
                .host
                .installed_with_authority(&actor.principal, &actor.authority)
                .await
                .map_err(failure)?,
        )
        .into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum AdminCommand {
    State {
        #[serde(rename = "pluginID")]
        plugin: String,
        desired: String,
        expected_revision: String,
        idempotency_key: String,
    },
}
async fn admin(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        if actor.read_only() {
            return Err(reject(
                StatusCode::FORBIDDEN,
                "permission_denied",
                "Write authority is required.",
            ));
        }
        let command: AdminCommand =
            serde_json::from_value(json_body(&body, &headers)?).map_err(|_| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid plugin state command.",
                )
            })?;
        let host = &endpoints(&state)?.host;
        let value = match command {
            AdminCommand::State {
                plugin,
                desired,
                expected_revision,
                idempotency_key,
            } => {
                host.request_state_with_authority(
                    &actor.principal,
                    &actor.authority,
                    &plugin,
                    &desired,
                    &expected_revision,
                    &idempotency_key,
                )
                .await
            }
        }
        .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

pub struct PluginDependencies {
    pub business: sea_orm::DatabaseConnection,
    pub worker: sea_orm::DatabaseConnection,
    pub auth: sea_orm::DatabaseConnection,
    pub identity: Arc<spitzli_identity::IdentityService>,
    pub browser: Option<Arc<spitzli_browser_session::BrowserSessions>>,
    pub oauth: Arc<spitzli_oauth::OAuthService>,
    pub ids: Arc<spitzli_persistence::IdAllocator>,
    pub rls_secret: String,
    pub database_new_orders_enabled: bool,
    pub neon_connections: Option<Arc<spitzli_provider_connections::Connections>>,
}
impl PluginEndpoints {
    pub async fn initialize(
        dependencies: PluginDependencies,
        config: &crate::plugin_config::PluginConfig,
    ) -> Result<Self, crate::plugin_config::ConfigurationError> {
        let fallback: Arc<dyn spitzli_plugin_host::RuntimeBindings> = Arc::new(
            spitzli_plugin_host::FileBindings::new(config.host_bindings.clone()),
        );
        let managed = match &config.installation_catalog {
            Some(_) => Some(Arc::new(
                spitzli_plugin_host::ManagedBindings::new(
                    crate::db::independent_pool(&dependencies.worker, "webdock_worker_runtime")
                        .await?,
                    dependencies.rls_secret.clone(),
                    fallback.clone(),
                )
                .await?
                .with_encryption_secret(&crate::plugin_config::installation_secret()?)?,
            )),
            None => None,
        };
        let bindings: Arc<dyn spitzli_plugin_host::RuntimeBindings> = match &managed {
            Some(value) => value.clone(),
            None => fallback,
        };
        let export_database = if config.export_sources {
            let encryption = std::env::var("SPITZLI_HOSTING_ENCRYPTION_KEY")
                .or_else(|_| std::env::var("BETTER_AUTH_SECRET"))
                .or_else(|_| std::env::var("SPITZLI_AUTH_SECRET"))?;
            let gateway = std::env::var("SPITZLI_DATABASE_GATEWAY_ORIGIN")?;
            Some(Arc::new(
                spitzli_databases::DatabaseService::new(
                    dependencies.business.clone(),
                    dependencies.auth.clone(),
                    dependencies.worker.clone(),
                    dependencies.identity.clone(),
                    dependencies.browser.clone(),
                    dependencies.ids.clone(),
                    dependencies.rls_secret.clone(),
                    &encryption,
                    &gateway,
                )
                .await?,
            ))
        } else {
            None
        };
        let mut host = HostService::new(
            dependencies.business,
            dependencies.worker.clone(),
            dependencies.rls_secret.clone(),
            dependencies.ids,
            bindings,
            crate::plugin_config::signing_key(&config.invocation_signing_key)?,
        )
        .await?
        .with_new_database_orders_enabled(dependencies.database_new_orders_enabled)
        .with_session_authority(
            dependencies.auth.clone(),
            dependencies.identity.clone(),
            dependencies.browser.clone(),
        )
        .await?
        .with_mcp_authority(dependencies.oauth)?
        .with_native_mail_control(crate::mail::native_control_configured(
            config.mail.as_ref(),
            &std::env::vars().collect(),
        ));
        if let Some(neon) = dependencies.neon_connections {
            host = host.with_named_http(neon);
        }
        if let Some(storage) = &config.storage {
            host = host.with_storage_gateway(Arc::new(
                spitzli_plugin_host::storage::StorageGateway::new(
                    storage.staging_directory.clone(),
                )?,
            ));
        }
        if let Some(catalog) = &config.installation_catalog {
            host = host.with_installation(
                Arc::new(spitzli_plugin_host::Catalog::load(catalog)?),
                &crate::plugin_config::installation_secret()?,
            )?;
        }
        let host = Arc::new(host);
        let repository = match &config.repository {
            Some(repository) => Some(Arc::new(
                spitzli_build::RepositoryAdmission::new(
                    dependencies.worker,
                    dependencies.rls_secret,
                    host.clone(),
                    crate::plugin_config::signing_key(&repository.signing_key)?,
                    Arc::new(spitzli_build::FileRepositoryBindings::new(
                        repository.bindings.clone(),
                    )),
                )
                .await?
                .with_session_authority(
                    dependencies.auth,
                    dependencies.identity,
                    dependencies.browser,
                )
                .await?,
            )),
            None => None,
        };
        let mut connections = RuntimeConnections::new(config.connections.clone());
        if let Some(managed) = managed {
            connections = connections.with_managed(managed);
        }
        let exports = export_database.map(|databases| {
            Arc::new(crate::plugin_exports::Sources {
                databases,
                repository: repository.clone(),
            }) as Arc<dyn spitzli_plugin_host::ExportSources>
        });
        Ok(Self {
            host,
            connections,
            repository,
            exports,
            mcp_client: config.mcp_client.clone(),
        })
    }
}

#[path = "plugin_installation/mod.rs"]
pub mod installation;

#[path = "api.rs"]
pub(crate) mod api;
#[path = "plugin_exposure.rs"]
mod exposure;
#[path = "plugin_operations.rs"]
mod operations;

pub(crate) fn credential_endpoint(path: &str) -> bool {
    exposure::credential_endpoint(path)
}

async fn plans(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let host = &endpoints(&state)?.host;
        let value = host
            .plans(&actor.principal, Some(&actor.authority))
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn write_plan(
    state: &ServiceState,
    headers: &HeaderMap,
    id: Option<&str>,
    body: &[u8],
) -> Result<Response, ApiFailure> {
    let actor = actor(state, headers).await?;
    writer(&actor)?;
    let input: spitzli_plugin_host::PlanInput = command(body, headers)?;
    let host = &endpoints(state)?.host;
    let value = host
        .save_plan(&actor.principal, Some(&actor.authority), id, input)
        .await
        .map_err(failure)?;
    Ok(Json(value).into_response())
}
async fn create_plan(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    write_plan(&state, &headers, None, &body)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}
async fn save_plan(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    write_plan(&state, &headers, Some(&id), &body)
        .await
        .unwrap_or_else(IntoResponse::into_response)
}
async fn delete_plan(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let host = &endpoints(&state)?.host;
        let value = host
            .delete_plan(&actor.principal, Some(&actor.authority), &id)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn customer_plan(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let input: spitzli_plugin_host::CustomerPlanInput = command(&body, &headers)?;
        let host = &endpoints(&state)?.host;
        let value = host
            .set_customer_plan(&actor.principal, Some(&actor.authority), &id, input)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn usage(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let host = &endpoints(&state)?.host;
        let value = host.usage(&actor.principal, &id).await.map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn notifications(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let host = &endpoints(&state)?.host;
        let value = host
            .notifications(&actor.principal, &id)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn logs(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((project, plugin, resource)): Path<(String, String, String)>,
    RawQuery(query): RawQuery,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let mut search = String::new();
        let mut before = None;
        for (key, value) in url::form_urlencoded::parse(query.as_deref().unwrap_or("").as_bytes()) {
            match key.as_ref() {
                "search" => search = value.into_owned(),
                "before" => before = Some(value.into_owned()),
                _ => {
                    return Err(reject(
                        StatusCode::BAD_REQUEST,
                        "invalid_input",
                        "Search with search and before only.",
                    ));
                }
            }
        }
        let value = endpoints(&state)?
            .host
            .logs(
                &actor.principal,
                &project,
                &plugin,
                &resource,
                &search,
                before.as_deref(),
            )
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn placement(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let value = endpoints(&state)?
            .host
            .placement(&actor.principal, &project)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Place {
    cluster_id: Option<String>,
}
async fn place(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(project): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let input: Place = command(&body, &headers)?;
        let value = endpoints(&state)?
            .host
            .place(&actor.principal, &project, input.cluster_id.as_deref())
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn clusters(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let value = endpoints(&state)?
            .host
            .clusters(&actor.principal, Some(&actor.authority))
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Offer {
    offered: bool,
}
async fn offer_cluster(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        writer(&actor)?;
        let input: Offer = command(&body, &headers)?;
        let value = endpoints(&state)?
            .host
            .offer_cluster(&actor.principal, Some(&actor.authority), &id, input.offered)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod mail_input_tests {
    use super::*;
    fn headers() -> HeaderMap {
        HeaderMap::from_iter([(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        )])
    }
    #[test]
    fn authenticated_console_parser_only_widens_mail_send() {
        for count in [20_000, 30_000] {
            let mut value = json!({"pluginID":"mail","projectID":"123","action":"mail.send","idempotencyKey":"same","input":{"resource_id":"owned","text":"😀".repeat(count)}});
            let body = serde_json::to_vec(&value).unwrap();
            assert!(body.len() > 65536);
            assert!(invoke_input(&body, &headers()).is_ok());
            assert!(json_body(&body, &headers()).is_err());
            value["pluginID"] = json!("notes");
            assert!(invoke_input(&serde_json::to_vec(&value).unwrap(), &headers()).is_err());
            value["pluginID"] = json!("mail");
            value["action"] = json!("mail.status");
            assert!(invoke_input(&serde_json::to_vec(&value).unwrap(), &headers()).is_err());
        }
        let body=serde_json::to_vec(&json!({"pluginID":"mail","projectID":"123","action":"mail.send","idempotencyKey":"same","input":{"text":"x".repeat(131073)}})).unwrap();
        assert!(invoke_input(&body, &headers()).is_err());
    }
}

#[cfg(test)]
mod invoke_diagnostic_tests {
    use super::*;
    #[test]
    fn real_tonic_and_http_mapping_remains_unchanged_and_keeps_numeric_distinction() {
        for code in [
            tonic::Code::PermissionDenied,
            tonic::Code::Unauthenticated,
            tonic::Code::InvalidArgument,
            tonic::Code::Aborted,
            tonic::Code::Unimplemented,
            tonic::Code::NotFound,
            tonic::Code::Unavailable,
            tonic::Code::DeadlineExceeded,
            tonic::Code::Internal,
        ] {
            let status = tonic::Status::new(code, "private-token-poison");
            let error = invocation_failure(status);
            let expected = match code {
                tonic::Code::PermissionDenied | tonic::Code::Unauthenticated => 403,
                tonic::Code::InvalidArgument => 400,
                tonic::Code::Aborted => 409,
                _ => 503,
            };
            assert_eq!(failure(error).into_response().status().as_u16(), expected);
            let line = InvokeTrace::new("mail", "domains.create")
                .line(
                    InvokeBoundary::Invoke,
                    "error",
                    None,
                    Some(code as i32),
                    None,
                )
                .unwrap();
            assert!(line.contains(&format!("tonicCode={}", code as i32)));
            assert!(!line.contains("poison"));
        }
        for code in [
            ErrorCode::NotSupported,
            ErrorCode::CapabilityUnavailable,
            ErrorCode::Unavailable,
            ErrorCode::DeadlineExceeded,
        ] {
            assert_eq!(
                failure(PluginError::new(code, "private-poison"))
                    .into_response()
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }
    }
    #[test]
    fn typed_transport_walk_retains_nested_tonic_and_errno_without_formatting() {
        struct Poison;
        impl std::fmt::Debug for Poison {
            fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                panic!("must not debug private error")
            }
        }
        impl std::fmt::Display for Poison {
            fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                panic!("must not display private error")
            }
        }
        impl std::error::Error for Poison {}
        assert_eq!(InvokeTrace::transport_numbers(&Poison), (None, None));
        let wrapped = std::io::Error::other(tonic::Status::unavailable("private-poison"));
        assert_eq!(InvokeTrace::transport_numbers(&wrapped), (Some(14), None));
        assert_eq!(
            InvokeTrace::transport_numbers(&std::io::Error::from_raw_os_error(13)),
            (None, Some(13))
        );
    }
    #[test]
    fn exact_flag_and_fixed_actions_never_format_private_errors() {
        for value in [None, Some("0"), Some("true"), Some("01"), Some("1")] {
            assert_eq!(
                InvokeTrace::enabled(value.map(std::ffi::OsStr::new)),
                value == Some("1")
            );
        }
        assert!(
            InvokeTrace::new("mail", "private-poison")
                .line(InvokeBoundary::View, "enter", None, None, None)
                .is_none()
        );
        assert!(
            InvokeTrace::new("other", "mail.send")
                .line(InvokeBoundary::View, "enter", None, None, None)
                .is_none()
        );
        let primary = PluginError::new(ErrorCode::NotFound, "private-primary-poison");
        let revoke = PluginError::new(ErrorCode::Unavailable, "private-revoke-poison");
        let trace = InvokeTrace::new("mail", "mail.send");
        let a = trace
            .line(
                InvokeBoundary::Result,
                "error",
                Some(primary.code),
                None,
                None,
            )
            .unwrap();
        let b = trace
            .line(
                InvokeBoundary::Revoke,
                "error",
                Some(revoke.code),
                None,
                None,
            )
            .unwrap();
        assert!(a.contains("publicCode=not_found") && b.contains("publicCode=unavailable"));
        assert!(!format!("{a}{b}").contains("poison"));
    }
}
