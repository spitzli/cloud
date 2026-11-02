use super::*;
use crate::resource_auth::{self, Resource};
use axum::{
    extract::{Path, Request},
    http::Method,
    routing::any,
};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use tower::ServiceExt;
use webdock_oauth::McpAuthentication;

pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route(
            "/v1/plugins/{plugin}/projects/{project}/api/{*path}",
            any(api),
        )
        .route("/api/mcp", any(mcp))
}
pub(super) fn credential_endpoint(path: &str) -> bool {
    path == "/api/mcp" || path.starts_with("/v1/plugins/")
}
fn route_action<'a>(manifest: &'a Value, method: &str, path: &str) -> Option<&'a Value> {
    let route = manifest["api"]
        .as_array()?
        .iter()
        .find(|r| r["method"] == method && r["path"] == path)?;
    manifest["actions"]
        .as_array()?
        .iter()
        .find(|a| a["name"] == route["action"])
}
fn api_input_limit(plugin: &str, action: &Value, read: bool) -> usize {
    if !read && plugin == "mail" && action["name"] == "mail.send" && action["intent"] == "write" {
        webdock_plugin_host::MAIL_ACTION_INPUT_BYTES
    } else {
        32768
    }
}
async fn api(
    State(state): State<Arc<ServiceState>>,
    Path((plugin, project, path)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    let result: Result<Response, Response> = async {
        let read = request.method() == Method::GET;
        let actor =
            resource_auth::authenticate(&state, request.headers(), Resource::Registry, false)
                .await
                .map_err(IntoResponse::into_response)?;
        // Retain the existing plugin API's 401 insufficient-write response.
        if !read && !actor.authority.can_write() {
            return Err(reject(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Bearer authorization is no longer valid.",
            )
            .into_response());
        }
        let endpoint = endpoints(&state).map_err(IntoResponse::into_response)?;
        let view = endpoint
            .host
            .project_plugins_with_mcp(&actor.principal, &project, &actor.authority)
            .await
            .map_err(|e| failure(e).into_response())?
            .into_iter()
            .find(|v| v.manifest["name"] == plugin)
            .ok_or_else(|| {
                reject(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "Plugin route unavailable.",
                )
                .into_response()
            })?;
        let action = route_action(
            &view.manifest,
            request.method().as_str(),
            &format!("/{path}"),
        )
        .ok_or_else(|| {
            reject(
                StatusCode::NOT_FOUND,
                "not_found",
                "Plugin route unavailable.",
            )
            .into_response()
        })?;
        let key = request
            .headers()
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .or_else(|| (action["intent"] == "read").then(|| uuid::Uuid::new_v4().to_string()))
            .ok_or_else(|| {
                reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Idempotency-Key required.",
                )
                .into_response()
            })?;
        let input_limit = api_input_limit(&plugin, action, read);
        let action = action["name"]
            .as_str()
            .ok_or_else(|| failure(unavailable_plugin()).into_response())?
            .to_owned();
        let input = if read {
            let parameters =
                url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                    .collect::<Vec<_>>();
            if parameters.len() > 1
                || parameters
                    .first()
                    .is_some_and(|p| p.0 != "input" || p.1.len() > 32768)
            {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Use one input JSON query parameter.",
                )
                .into_response());
            }
            match parameters.first() {
                Some(p) if !p.1.is_empty() => serde_json::from_str(&p.1).map_err(|_| {
                    reject(
                        StatusCode::BAD_REQUEST,
                        "invalid_input",
                        "Invalid plugin input.",
                    )
                    .into_response()
                })?,
                _ => json!({}),
            }
        } else {
            let headers = request.headers().clone();
            let body = axum::body::to_bytes(request.into_body(), input_limit)
                .await
                .map_err(|_| {
                    reject(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "invalid_input",
                        "Plugin input exceeds limit.",
                    )
                    .into_response()
                })?;
            json_body_limit(&body, &headers, input_limit).map_err(IntoResponse::into_response)?
        };
        let value = endpoint
            .invoke_mcp(
                &actor,
                AdmissionInput {
                    plugin_id: plugin,
                    project_id: project,
                    action,
                    input,
                    idempotency_key: key,
                },
            )
            .await
            .map_err(|e| failure(e).into_response())?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
struct ExposedTool {
    tool: Tool,
    plugin: String,
    action: String,
    write: bool,
}
fn tools(
    views: &[webdock_plugin_host::PluginView],
    can_write: bool,
) -> Result<Vec<ExposedTool>, PluginError> {
    let mut output = Vec::new();
    let mut names = std::collections::HashSet::new();
    for view in views {
        if view.state != "enabled" || view.runtime != "ready" {
            continue;
        }
        let m = &view.manifest;
        let Some(exposed) = m["mcp"].as_array() else {
            continue;
        };
        for entry in exposed {
            let Some(action) = m["actions"]
                .as_array()
                .and_then(|a| a.iter().find(|a| a["name"] == entry["action"]))
            else {
                continue;
            };
            let write = action["intent"] == "write";
            if write && !can_write {
                continue;
            }
            let plugin = m["name"].as_str().ok_or_else(unavailable_plugin)?;
            let name = format!(
                "plugin__{}__{}",
                plugin.replace('-', "_"),
                entry["name"].as_str().ok_or_else(unavailable_plugin)?
            );
            if !names.insert(name.clone()) {
                return Err(PluginError::new(
                    ErrorCode::Conflict,
                    "Plugin MCP tool collision",
                ));
            }
            let input = action.get("input").cloned().unwrap_or_else(
                || json!({"type":"object","properties":{},"additionalProperties":false}),
            );
            let mut required = vec!["projectID", "input"];
            if write {
                required.push("idempotencyKey");
            }
            let tool:Tool=serde_json::from_value(json!({"name":name,"description":entry["description"],"inputSchema":{"type":"object","properties":{"projectID":{"type":"string","pattern":"^[0-9]{1,20}$"},"input":input,"idempotencyKey":{"type":"string","minLength":1,"maxLength":128}},"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":!write,"destructiveHint":action["destructive"].as_bool().unwrap_or(false)}})).map_err(|_|unavailable_plugin())?;
            output.push(ExposedTool {
                tool,
                plugin: plugin.into(),
                action: action["name"]
                    .as_str()
                    .ok_or_else(unavailable_plugin)?
                    .into(),
                write,
            });
        }
    }
    Ok(output)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolArguments {
    #[serde(rename = "projectID")]
    project_id: String,
    input: Value,
    idempotency_key: Option<String>,
}
#[derive(Clone)]
struct McpHandler {
    state: Arc<ServiceState>,
    actor: Arc<McpAuthentication>,
    resource: Resource,
}
impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            rmcp::model::Implementation::new(
                if self.resource == Resource::Registry {
                    "webdock-studio"
                } else {
                    "webdock-hosting"
                },
                "1.0.0",
            ),
        )
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        if request.as_ref().is_some_and(|p| p.cursor.is_some()) {
            return Err(rmcp::ErrorData::invalid_params("Cursor unavailable", None));
        }
        let oauth =
            self.state.oauth.as_ref().ok_or_else(|| {
                rmcp::ErrorData::internal_error("Authorization unavailable", None)
            })?;
        let guard = oauth
            .begin_mcp_authority(&self.actor.authority)
            .await
            .map_err(|_| {
                rmcp::ErrorData::invalid_request("Authorization is no longer valid", None)
            })?;
        let can_write = guard.can_write();
        // Domain discovery below acquires and verifies its own guard. Release this
        // preliminary check first so a single-connection auth pool remains usable.
        drop(guard);
        let mut definitions = crate::first_party_mcp::tools(&self.state, self.resource, can_write)?;
        if self.resource == Resource::Registry {
            if self.state.registry.is_some() {
                // Public registry discovery requires the same current mapped operator as a read.
                crate::resource_registry::execute(
                    &self.state,
                    &self.actor,
                    crate::resource_registry::Operation::List(
                        json!({"collection":"customers","limit":1}),
                    ),
                )
                .await
                .map_err(|_| {
                    rmcp::ErrorData::invalid_request("Registry access unavailable", None)
                })?;
            }
            if let Some(endpoint) = &self.state.plugins {
                let views = endpoint
                    .host
                    .mcp_plugins(&self.actor.principal, &self.actor.authority)
                    .await
                    .map_err(mcp_error)?;
                definitions.extend(
                    tools(&views, can_write)
                        .map_err(mcp_error)?
                        .into_iter()
                        .map(|t| t.tool),
                );
            }
        }
        Ok(ListToolsResult {
            tools: definitions,
            ..ListToolsResult::default()
        })
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        if !request.name.starts_with("plugin__") {
            if request.input_responses.is_some() || request.request_state.is_some() {
                return Err(rmcp::ErrorData::invalid_params(
                    "Continuation unavailable",
                    None,
                ));
            }
            let result = crate::first_party_mcp::execute(
                &self.state,
                &self.actor,
                self.resource,
                &request.name,
                Value::Object(request.arguments.unwrap_or_default()),
            )
            .await;
            let result = match result {
                Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
                Err(response) => {
                    let body = axum::body::to_bytes(response.into_response().into_body(), 65536)
                        .await
                        .unwrap_or_default();
                    CallToolResult::error(vec![ContentBlock::text(String::from_utf8_lossy(&body))])
                }
            };
            return Ok(result.into());
        }
        let result: Result<Value, PluginError> = async {
            if request.input_responses.is_some() || request.request_state.is_some() {
                return Err(PluginError::new(
                    ErrorCode::InvalidInput,
                    "Continuation unavailable",
                ));
            }
            if self.resource != Resource::Registry {
                return Err(PluginError::denied());
            }
            let endpoint = self.state.plugins.as_ref().ok_or_else(unavailable_plugin)?;
            let views = endpoint
                .host
                .mcp_plugins(&self.actor.principal, &self.actor.authority)
                .await?;
            let tool = tools(&views, self.actor.authority.can_write())?
                .into_iter()
                .find(|t| t.tool.name == request.name)
                .ok_or_else(|| PluginError::new(ErrorCode::NotFound, "Plugin tool unavailable"))?;
            let input: ToolArguments =
                serde_json::from_value(Value::Object(request.arguments.unwrap_or_default()))
                    .map_err(|_| {
                        PluginError::new(ErrorCode::InvalidInput, "Invalid tool arguments")
                    })?;
            let project = input.project_id;
            let key = input
                .idempotency_key
                .or_else(|| (!tool.write).then(|| uuid::Uuid::new_v4().to_string()))
                .ok_or_else(PluginError::denied)?;
            endpoint
                .invoke_mcp(
                    &self.actor,
                    AdmissionInput {
                        plugin_id: tool.plugin,
                        project_id: project,
                        action: tool.action,
                        input: input.input,
                        idempotency_key: key,
                    },
                )
                .await
        }
        .await;
        Ok(match result {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            Err(error) => {
                CallToolResult::error(vec![ContentBlock::text(json!({"error":error}).to_string())])
            }
        }
        .into())
    }
}
fn mcp_error(_: PluginError) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error("Plugin discovery unavailable", None)
}
async fn registry_write_ceiling(
    state: &ServiceState,
    request: Request,
) -> Result<Request, resource_auth::ResourceFailure> {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 65536).await.map_err(|_| {
        resource_auth::ResourceFailure::from(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Request is too large.",
        ))
    })?;
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && value["method"] == "tools/call"
        && value["params"]["name"]
            .as_str()
            .is_some_and(|name| ["save_customer", "save_project", "set_archived"].contains(&name))
    {
        return Err(resource_auth::challenge(
            state,
            "webdock:read webdock:write",
            StatusCode::FORBIDDEN,
        ));
    }
    Ok(Request::from_parts(parts, axum::body::Body::from(bytes)))
}
fn large_mcp_name(value: &Value, bytes: usize) -> Option<&str> {
    let object = value.as_object()?;
    if bytes > MAIL_ENVELOPE_BYTES
        || object.len() != 4
        || !["jsonrpc", "id", "method", "params"]
            .iter()
            .all(|k| object.contains_key(*k))
        || value["jsonrpc"] != "2.0"
        || value["method"] != "tools/call"
    {
        return None;
    }
    if !(value["id"]
        .as_str()
        .is_some_and(|id| id.encode_utf16().count() <= 128)
        || value["id"].as_i64().is_some())
    {
        return None;
    }
    let params = value["params"].as_object()?;
    if params.len() != 2 || !params.contains_key("name") || !params.contains_key("arguments") {
        return None;
    }
    let args: ToolArguments = serde_json::from_value(params["arguments"].clone()).ok()?;
    let input_bytes = serde_json::to_vec(&args.input).ok()?.len();
    if !args.input.is_object()
        || !(1..=20).contains(&args.project_id.len())
        || !args.project_id.bytes().all(|b| b.is_ascii_digit())
        || input_bytes > webdock_plugin_host::MAIL_ACTION_INPUT_BYTES
        || bytes.saturating_sub(input_bytes) > 16 * 1024
        || !args
            .idempotency_key
            .as_deref()
            .is_some_and(|k| !k.is_empty() && k.encode_utf16().count() <= 128)
    {
        return None;
    }
    params["name"].as_str()
}
fn declared_mail_tool(definitions: &[ExposedTool], name: &str) -> bool {
    definitions.iter().any(|tool| {
        tool.tool.name == name && tool.plugin == "mail" && tool.action == "mail.send" && tool.write
    })
}
async fn mcp_request_budget(
    state: &ServiceState,
    actor: &McpAuthentication,
    resource: Resource,
    request: Request,
) -> Result<(Request, usize), resource_auth::ResourceFailure> {
    if request.method() != Method::POST {
        return Ok((request, 65536));
    }
    let (parts, body) = request.into_parts();
    let too_large = || {
        resource_auth::ResourceFailure::from(reject(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Request is too large.",
        ))
    };
    let bytes = axum::body::to_bytes(body, MAIL_ENVELOPE_BYTES)
        .await
        .map_err(|_| too_large())?;
    let limit = if bytes.len() <= 65536 {
        65536
    } else {
        if resource != Resource::Registry || !actor.authority.can_write() {
            return Err(too_large());
        }
        let value =
            webdock_plugin_protocol::release::strict_json(&bytes).map_err(|_| too_large())?;
        let name = large_mcp_name(&value, bytes.len()).ok_or_else(too_large)?;
        let endpoint = state
            .plugins
            .as_ref()
            .ok_or_else(|| resource_auth::ResourceFailure::from(failure(unavailable_plugin())))?;
        let views = endpoint
            .host
            .mcp_plugins(&actor.principal, &actor.authority)
            .await
            .map_err(|e| resource_auth::ResourceFailure::from(failure(e)))?;
        let definitions =
            tools(&views, true).map_err(|e| resource_auth::ResourceFailure::from(failure(e)))?;
        if !declared_mail_tool(&definitions, name) {
            return Err(too_large());
        }
        MAIL_ENVELOPE_BYTES
    };
    Ok((
        Request::from_parts(parts, axum::body::Body::from(bytes)),
        limit,
    ))
}
async fn mcp(State(state): State<Arc<ServiceState>>, request: Request) -> Response {
    let (actor, resource) =
        match resource_auth::authenticate(&state, request.headers(), Resource::Registry, false)
            .await
        {
            Ok(actor) => (actor, Resource::Registry),
            Err(registry_error) => {
                if registry_error.status() != StatusCode::UNAUTHORIZED {
                    return registry_error.into_response();
                }
                match resource_auth::authenticate(
                    &state,
                    request.headers(),
                    Resource::Hosting,
                    false,
                )
                .await
                {
                    Ok(actor) => (actor, Resource::Hosting),
                    Err(error) if error.status() == StatusCode::UNAUTHORIZED => {
                        return registry_error.into_response();
                    }
                    Err(error) => return error.into_response(),
                }
            }
        };
    let (request, body_limit) = match mcp_request_budget(&state, &actor, resource, request).await {
        Ok(result) => result,
        Err(error) => return error.into_response(),
    };
    let request = if resource == Resource::Registry && !actor.authority.can_write() {
        match registry_write_ceiling(&state, request).await {
            Ok(request) => request,
            Err(error) => return error.into_response(),
        }
    } else {
        request
    };
    let handler = McpHandler {
        state: state.clone(),
        actor,
        resource,
    };
    let studio = url::Url::parse(&state.config.studio_origin).expect("validated origin");
    let authority = studio[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
    let config = rmcp::transport::streamable_http_server::StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts([authority])
        .with_allowed_origins([format!(
            "{}://{}:{}",
            studio.scheme(),
            studio.host_str().expect("validated host"),
            studio.port_or_known_default().expect("validated port")
        )])
        .with_max_request_body_bytes(body_limit);
    let service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        config,
    );
    match service.oneshot(request).await {
        Ok(response) => response.map(axum::body::Body::new),
        Err(error) => match error {},
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn view() -> webdock_plugin_host::PluginView {
        webdock_plugin_host::PluginView {
            state: "enabled".into(),
            runtime: "ready".into(),
            generation: 1,
            manifest_sha256: "a".repeat(64),
            launch_links: vec![],
            features: vec![],
            blocked_create_actions: vec![],
            resources: vec![],
            manifest: json!({"name":"notes-fixture","actions":[{"name":"notes.read","intent":"read"},{"name":"notes.write","intent":"write","destructive":true}],"api":[{"method":"GET","path":"/notes","action":"notes.read"}],"mcp":[{"name":"read","action":"notes.read","description":"Read notes"},{"name":"write","action":"notes.write","description":"Write notes"}]}),
        }
    }
    #[test]
    fn readonly_tools_and_exact_api_routes() {
        let v = view();
        let listed = tools(std::slice::from_ref(&v), false).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].tool.name, "plugin__notes_fixture__read");
        assert_eq!(tools(std::slice::from_ref(&v), true).unwrap().len(), 2);
        assert!(tools(&[view(), view()], true).is_err());
        assert_eq!(
            route_action(&v.manifest, "GET", "/notes").unwrap()["name"],
            "notes.read"
        );
        assert!(route_action(&v.manifest, "POST", "/notes").is_none());
        assert!(route_action(&v.manifest, "GET", "/notes/extra").is_none());
    }
    #[test]
    fn large_body_limits_follow_the_declared_mail_action_only() {
        let mut v = view();
        v.manifest["name"] = json!("mail");
        v.manifest["actions"] =
            json!([{"name":"mail.send","intent":"write"},{"name":"mail.status","intent":"read"}]);
        v.manifest["api"] = json!([{"method":"POST","path":"/send","action":"mail.send"}]);
        v.manifest["mcp"] =
            json!([{"name":"send_email","action":"mail.send","description":"Send"}]);
        let route = route_action(&v.manifest, "POST", "/send").unwrap();
        let body =
            serde_json::to_vec(&json!({"resource_id":"owned","text":"😀".repeat(30_000)})).unwrap();
        let headers = HeaderMap::from_iter([(
            header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        )]);
        assert!(json_body_limit(&body, &headers, api_input_limit("mail", route, false)).is_ok());
        assert!(json_body_limit(&body, &headers, api_input_limit("other", route, false)).is_err());
        assert_eq!(api_input_limit("mail", route, false), 131072);
        assert_eq!(api_input_limit("other", route, false), 32768);
        assert_eq!(api_input_limit("mail", route, true), 32768);
        let defs = tools(std::slice::from_ref(&v), true).unwrap();
        let value = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"plugin__mail__send_email","arguments":{"projectID":"123","idempotencyKey":"same","input":{"resource_id":"owned","text":"😀".repeat(30_000)}}}});
        let bytes = serde_json::to_vec(&value).unwrap();
        let name = large_mcp_name(&value, bytes.len()).unwrap();
        assert!(declared_mail_tool(&defs, name));
        v.manifest["mcp"][0]["action"] = json!("mail.status");
        assert!(!declared_mail_tool(&tools(&[v], true).unwrap(), name));
        for (field, replacement) in [
            ("id", json!(null)),
            ("id", json!([])),
            ("id", json!({})),
            ("id", json!(1.5)),
            ("id", json!("x".repeat(129))),
            ("method", json!("tools/list")),
        ] {
            let mut bad = value.clone();
            bad[field] = replacement;
            assert!(large_mcp_name(&bad, bytes.len()).is_none());
        }
        for id in [json!("bounded"), json!(i64::MAX)] {
            let mut good = value.clone();
            good["id"] = id;
            assert!(large_mcp_name(&good, serde_json::to_vec(&good).unwrap().len()).is_some());
        }
        let mut meta = value.clone();
        meta["params"]["_meta"] = json!({"padding":"x".repeat(70_000)});
        assert!(large_mcp_name(&meta, serde_json::to_vec(&meta).unwrap().len()).is_none());
        let mut extra = value.clone();
        extra["params"]["arguments"]["padding"] = json!("x");
        assert!(large_mcp_name(&extra, bytes.len()).is_none());
        assert!(large_mcp_name(&json!([value.clone()]), bytes.len()).is_none());
        assert!(large_mcp_name(&value, bytes.len() + 40_000).is_none());
        let mut padded = value.clone();
        padded["padding"] = json!("x");
        assert!(large_mcp_name(&padded, bytes.len()).is_none());
        let mut bad_project = value.clone();
        bad_project["params"]["arguments"]["projectID"] = json!("caller-project");
        assert!(large_mcp_name(&bad_project, bytes.len()).is_none());
        let mut small = value;
        small["params"]["arguments"]["input"]["text"] = json!("small");
        assert!(large_mcp_name(&small, 80_000).is_none());
    }
}
