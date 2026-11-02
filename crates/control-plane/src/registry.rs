use crate::{
    auth,
    http::{ApiFailure, ServiceState, reject as error},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use webdock_identity::Principal;
use webdock_registry::{RegistryError, RegistryService};

pub fn routes() -> Router<Arc<ServiceState>> {
    let mut router = Router::new();
    for prefix in ["/v1/operator/registry", "/api/console/registry"] {
        router = router
            .route(&format!("{prefix}/{{collection}}"), get(list).post(create))
            .route(
                &format!("{prefix}/{{collection}}/{{id}}"),
                get(record).put(save).patch(archive).delete(remove),
            );
    }
    router
}
async fn authorized<'a>(
    state: &'a ServiceState,
    headers: &HeaderMap,
) -> Result<(&'a RegistryService, Principal), ApiFailure> {
    let authenticated = auth::resolve_actor(state, headers).await?.ok_or_else(|| {
        error(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Sign in to continue.",
        )
    })?;
    let principal = authenticated.principal;
    if !principal.is_operator() {
        return Err(error(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Current operator access is required.",
        ));
    }
    let registry = state.registry.as_ref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Registry is unavailable.",
        )
    })?;
    Ok((registry, principal))
}
fn failure(failure: RegistryError) -> ApiFailure {
    let status =
        StatusCode::from_u16(failure.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let message = if status.is_server_error() {
        "Registry request failed.".into()
    } else {
        failure.to_string()
    };
    error(status, "registry_error", &message)
}
pub fn list_query(collection: &str, query: Option<&str>) -> Result<Value, ApiFailure> {
    let mut value =
        serde_json::Map::from_iter([("collection".into(), Value::String(collection.into()))]);
    let mut keys = HashSet::new();
    for (key, item) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        if !keys.insert(key.to_string()) || key == "collection" {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Duplicate or reserved query field.",
            ));
        }
        let field = if matches!(key.as_ref(), "page" | "limit") {
            if item.is_empty() || !item.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Invalid pagination.",
                ));
            }
            json!(item.parse::<u64>().map_err(|_| error(
                StatusCode::BAD_REQUEST,
                "invalid_input",
                "Invalid pagination."
            ))?)
        } else {
            Value::String(item.into_owned())
        };
        value.insert(key.into_owned(), field);
    }
    Ok(Value::Object(value))
}
fn body(headers: &HeaderMap, bytes: &[u8]) -> Result<Value, ApiFailure> {
    if bytes.len() > 65536 {
        return Err(error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_input",
            "Request is too large.",
        ));
    }
    if !headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().unwrap_or("").trim() == "application/json")
    {
        return Err(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_input",
            "JSON is required.",
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_input", "Invalid JSON."))
}
async fn list(
    State(state): State<Arc<ServiceState>>,
    Path(collection): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let (registry, actor) = authorized(&state, &headers).await?;
        registry
            .list(&actor, list_query(&collection, query.as_deref())?)
            .await
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(failure)
    }
    .await;
    result.unwrap_or_else(|response| response.into_response())
}
async fn record(
    State(state): State<Arc<ServiceState>>,
    Path((collection, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let (registry, actor) = authorized(&state, &headers).await?;
        registry
            .get(&actor, &collection, &id)
            .await
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(failure)
    }
    .await;
    result.unwrap_or_else(|response| response.into_response())
}
async fn write(
    state: &ServiceState,
    headers: &HeaderMap,
    collection: &str,
    id: Option<&str>,
    bytes: &[u8],
) -> Result<Response, ApiFailure> {
    let (registry, actor) = authorized(state, headers).await?;
    let input = body(headers, bytes)?;
    let record = match collection {
        "customers" => registry.save_customer(&actor, id, input).await,
        "projects" => registry.save_project(&actor, id, input).await,
        "audit-events" => {
            return Err(error(
                StatusCode::METHOD_NOT_ALLOWED,
                "read_only",
                "Audit events are read-only.",
            ));
        }
        _ => {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Writable collection unavailable.",
            ));
        }
    }
    .map_err(failure)?;
    Ok((StatusCode::OK, Json(record)).into_response())
}
async fn create(
    State(state): State<Arc<ServiceState>>,
    Path(collection): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    write(&state, &headers, &collection, None, &bytes)
        .await
        .unwrap_or_else(|response| response.into_response())
}
async fn save(
    State(state): State<Arc<ServiceState>>,
    Path((collection, id)): Path<(String, String)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    write(&state, &headers, &collection, Some(&id), &bytes)
        .await
        .unwrap_or_else(|response| response.into_response())
}
async fn archive(
    State(state): State<Arc<ServiceState>>,
    Path((collection, id)): Path<(String, String)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let result = async {
        let (registry, actor) = authorized(&state, &headers).await?;
        let input = body(&headers, &bytes)?;
        let archived = input
            .as_object()
            .filter(|object| object.len() == 1)
            .and_then(|object| object.get("archived"))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                error(
                    StatusCode::BAD_REQUEST,
                    "invalid_input",
                    "Expected archived boolean.",
                )
            })?;
        registry
            .archive(&actor, &collection, &id, archived)
            .await
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(failure)
    }
    .await;
    result.unwrap_or_else(|response| response.into_response())
}
async fn remove(
    State(state): State<Arc<ServiceState>>,
    Path((collection, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let (registry, actor) = authorized(&state, &headers).await?;
        if collection != "customers" {
            return Err(error(
                StatusCode::METHOD_NOT_ALLOWED,
                "invalid_input",
                "Only customers can be deleted here.",
            ));
        }
        registry
            .delete_customer(&actor, &id)
            .await
            .map(Json)
            .map(IntoResponse::into_response)
            .map_err(failure)
    }
    .await;
    result.unwrap_or_else(|response| response.into_response())
}
