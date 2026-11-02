//! The workspace API: `Authorization: Bearer wdk_…` from a token made in the console. It reaches the
//! token's workspace only, with the role of the member who made it, and writes only if allowed to.
use super::*;
use axum::routing::delete;

pub(super) fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/v1/api/projects", get(projects))
        .route("/v1/api/projects/{project}/plugins", get(plugins))
        .route("/v1/api/projects/{project}/actions", post(action))
        .route("/v1/api/usage", get(usage))
        .route(
            "/api/console/tenant/{id}/api-tokens",
            get(tokens).post(create_token),
        )
        .route(
            "/api/console/tenant/{id}/api-tokens/{token}",
            delete(revoke_token),
        )
}
pub(crate) fn credential_endpoint(path: &str) -> bool {
    path.starts_with("/v1/api/")
}
fn access(state: &ServiceState) -> Result<&webdock_access::AccessService, ApiFailure> {
    state.access.as_deref().ok_or_else(|| {
        reject(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Access service is unavailable.",
        )
    })
}
fn access_failure(error: webdock_access::Error) -> ApiFailure {
    crate::access::failure(error)
}
/// The token's member as an actor, its workspace, and whether it may write.
pub(super) struct Bearer {
    pub(super) actor: auth::AuthenticatedActor,
    pub(super) customer: String,
    pub(super) can_write: bool,
}
pub(super) async fn bearer(
    state: &ServiceState,
    headers: &HeaderMap,
) -> Result<Bearer, ApiFailure> {
    let unauthorized = || {
        reject(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Send a workspace API token as a Bearer token.",
        )
    };
    if headers.contains_key(header::ORIGIN) || headers.contains_key(header::COOKIE) {
        return Err(reject(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "The workspace API is not for browsers.",
        ));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)?;
    let found = access(state)?
        .api_actor(token)
        .await
        .map_err(|_| unauthorized())?;
    Ok(Bearer {
        actor: auth::AuthenticatedActor {
            principal: found.principal,
            authority: auth::ActorAuthority::LocalSession {},
        },
        customer: found.customer,
        can_write: found.can_write,
    })
}
/// The project, if it belongs to the token's workspace.
async fn project_of(
    state: &ServiceState,
    bearer: &Bearer,
    project: &str,
) -> Result<(), ApiFailure> {
    let found = access(state)?
        .get_project_as(&bearer.actor.principal, project)
        .await
        .map_err(access_failure)?;
    if found["customer"]["id"] != bearer.customer.as_str() {
        return Err(reject(
            StatusCode::NOT_FOUND,
            "not_found",
            "Record unavailable.",
        ));
    }
    Ok(())
}
async fn projects(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let bearer = bearer(&state, &headers).await?;
        let list = access(&state)?
            .list_projects_as(&bearer.actor.principal, &bearer.customer)
            .await
            .map_err(access_failure)?;
        Ok(Json(list).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn plugins(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let bearer = bearer(&state, &headers).await?;
        project_of(&state, &bearer, &project).await?;
        let views = endpoints(&state)?
            .host
            .project_plugins_with_authority(&bearer.actor.principal, &project, &bearer.actor.authority)
            .await
            .map_err(failure)?;
        let list: Vec<Value> = views
            .iter()
            .map(|v| {
                json!({"plugin": v.manifest["name"], "actions": v.manifest["actions"], "resources": v.resources, "features": v.features})
            })
            .collect();
        Ok(Json(list).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Action {
    plugin: String,
    action: String,
    #[serde(default = "empty")]
    input: Value,
    idempotency_key: Option<String>,
}
fn empty() -> Value {
    json!({})
}
async fn action(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(project): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let bearer = bearer(&state, &headers).await?;
        let command: Action = serde_json::from_value(json_body(&body, &headers)?).map_err(|_| {
            reject(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Send plugin, action and input.",
            )
        })?;
        project_of(&state, &bearer, &project).await?;
        let input: AdmissionInput = serde_json::from_value(json!({
            "pluginID": command.plugin, "projectID": project, "action": command.action, "input": command.input,
            "idempotencyKey": command.idempotency_key.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        }))
        .map_err(|_| reject(StatusCode::BAD_REQUEST, "invalid_input", "Send plugin, action and input."))?;
        let output = endpoints(&state)?
            .invoke_api(&bearer.actor, input, bearer.can_write, hex::encode(Sha256::digest(headers.get(header::AUTHORIZATION).and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).ok_or_else(|| failure(PluginError::denied()))?.as_bytes())))
            .await
            .map_err(failure)?;
        Ok(Json(output).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn usage(State(state): State<Arc<ServiceState>>, headers: HeaderMap) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let bearer = bearer(&state, &headers).await?;
        let value = endpoints(&state)?
            .host
            .usage(&bearer.actor.principal, &bearer.customer)
            .await
            .map_err(failure)?;
        Ok(Json(value).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

async fn console_actor(
    state: &ServiceState,
    headers: &HeaderMap,
) -> Result<auth::AuthenticatedActor, ApiFailure> {
    let actor = actor(state, headers).await?;
    writer(&actor)?;
    Ok(actor)
}
async fn tokens(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = actor(&state, &headers).await?;
        let list = access(&state)?
            .list_api_tokens_as(&actor.principal, &id)
            .await
            .map_err(access_failure)?;
        Ok(Json(list).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewToken {
    name: String,
    pub(super) can_write: bool,
    days: i64,
}
async fn create_token(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = console_actor(&state, &headers).await?;
        let input: NewToken = command(&body, &headers)?;
        let created = access(&state)?
            .create_api_token_as(
                &actor.principal,
                &id,
                &input.name,
                input.can_write,
                input.days,
            )
            .await
            .map_err(access_failure)?;
        Ok(Json(created).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
async fn revoke_token(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((id, token)): Path<(String, String)>,
) -> Response {
    let result: Result<Response, ApiFailure> = async {
        let actor = console_actor(&state, &headers).await?;
        access(&state)?
            .revoke_api_token_as(&actor.principal, &id, &token)
            .await
            .map_err(access_failure)?;
        Ok(Json(json!({})).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}
