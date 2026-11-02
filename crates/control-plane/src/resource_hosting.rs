use crate::resource_auth::ResourceFailure;
use crate::{
    http::ServiceState,
    resource_auth::{self, Resource},
    resource_registry::{public_error, read_json, unavailable},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use webdock_core::features::OperationKind;
use webdock_oauth::McpAuthentication;

/// Retained names for the currently implemented native domain commands.
pub(crate) const TOOLS: &[(&str, &str)] = &[
    ("apps.list", "list_hosting_apps"),
    ("apps.get", "get_hosting_app"),
    ("apps.prepare", "prepare_hosting_app"),
    ("apps.deploy", "deploy_prepared_hosting_app"),
    ("apps.create", "create_hosting_app"),
    ("apps.update", "update_hosting_app"),
    ("apps.scale", "scale_hosting_app"),
    ("apps.start", "start_hosting_app"),
    ("apps.stop", "stop_hosting_app"),
    ("apps.restart", "restart_hosting_app"),
    ("apps.rollback", "rollback_hosting_app"),
    ("apps.logs", "get_hosting_logs"),
    ("apps.deletion", "preview_hosting_app_deletion"),
    ("apps.delete", "delete_hosting_app"),
    ("apps.storageDeletion", "preview_hosting_storage_deletion"),
    ("apps.purgeStorage", "delete_hosting_storage"),
    ("apps.reconcile", "reconcile_hosting_app"),
    ("clusters.list", "list_hosting_clusters"),
    ("clusters.get", "get_hosting_cluster"),
    ("clusters.register", "register_hosting_cluster"),
    ("clusters.activate", "activate_hosting_cluster"),
    ("clusters.revoke", "revoke_hosting_agent"),
    ("limits.get", "get_hosting_limits"),
    ("limits.set", "set_hosting_limits"),
    ("usage.get", "get_hosting_usage"),
    ("projects.get", "get_hosting_project"),
    ("projects.list", "list_hosting_projects"),
    ("projects.create", "create_hosting_project"),
    ("projects.update", "update_hosting_project"),
    ("operations.get", "get_hosting_operation"),
    ("operations.overview", "get_hosting_overview"),
];
pub(crate) fn is_read(action: &str) -> bool {
    action.ends_with(".get")
        || action.ends_with(".list")
        || [
            "apps.logs",
            "apps.deletion",
            "apps.storageDeletion",
            "operations.overview",
        ]
        .contains(&action)
}
pub(crate) async fn execute(
    state: &ServiceState,
    actor: &McpAuthentication,
    input: Value,
) -> Result<Value, ResourceFailure> {
    if crate::resource_databases::is_database(&input) {
        return crate::resource_databases::execute(state, actor, input).await;
    }
    let action = input
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| public_error(StatusCode::BAD_REQUEST, "Action is required."))?;
    state
        .config
        .features
        .require(
            "hosting",
            if is_read(action) {
                OperationKind::Read
            } else {
                OperationKind::Provision
            },
        )
        .map_err(|_| unavailable())?;
    state
        .hosting
        .as_ref()
        .ok_or_else(unavailable)?
        .execute_with_mcp(state.oauth.as_ref().ok_or_else(unavailable)?, actor, input)
        .await
        .map_err(|error| {
            public_error(
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                &error.message,
            )
        })
}
pub fn routes() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/hosting", any(handle))
        .route("/api/hosting/{*path}", any(handle))
}
fn query(raw: Option<&str>) -> Result<serde_json::Map<String, Value>, ResourceFailure> {
    let mut seen = HashSet::new();
    let mut output = serde_json::Map::new();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or("").as_bytes()) {
        if !["page", "limit", "search", "sort"].contains(&key.as_ref())
            || !seen.insert(key.to_string())
        {
            return Err(public_error(StatusCode::BAD_REQUEST, "Invalid query."));
        }
        let value =
            match key.as_ref() {
                "page" | "limit" => {
                    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(public_error(StatusCode::BAD_REQUEST, "Invalid pagination."));
                    }
                    json!(value.parse::<u64>().map_err(|_| public_error(
                        StatusCode::BAD_REQUEST,
                        "Invalid pagination."
                    ))?)
                }
                _ => json!(value),
            };
        output.insert(key.into_owned(), value);
    }
    Ok(output)
}
fn read_target<'a>(path: &[&'a str]) -> Option<(&'static str, Option<(&'static str, &'a str)>)> {
    match path {
        ["apps", id] => Some(("apps.get", Some(("appID", id)))),
        ["apps", id, "storage-deletion"] => Some(("apps.storageDeletion", Some(("appID", id)))),
        ["apps", id, "deletion"] => Some(("apps.deletion", Some(("appID", id)))),
        ["projects", id, "apps"] => Some(("apps.list", Some(("projectID", id)))),
        ["clusters"] => Some(("clusters.list", None)),
        ["clusters", id] => Some(("clusters.get", Some(("clusterID", id)))),
        ["customers", id, "limits"] => Some(("limits.get", Some(("customerID", id)))),
        ["customers", id, "usage"] => Some(("usage.get", Some(("customerID", id)))),
        ["customers", id, "projects"] => Some(("projects.list", Some(("customerID", id)))),
        ["operations", id] => Some(("operations.get", Some(("operationID", id)))),
        _ => None,
    }
}
fn write_target<'a>(
    method: &Method,
    path: &[&'a str],
) -> Option<(&'static str, Option<(&'static str, &'a str)>)> {
    match (method, path) {
        (&Method::POST, ["projects", id, "apps"]) => Some(("apps.create", Some(("projectID", id)))),
        (&Method::PATCH, ["apps", id]) => Some(("apps.update", Some(("appID", id)))),
        (&Method::DELETE, ["apps", id, "storage"]) => {
            Some(("apps.purgeStorage", Some(("appID", id))))
        }
        (&Method::DELETE, ["apps", id]) => Some(("apps.delete", Some(("appID", id)))),
        (&Method::POST, ["apps", id, child]) => {
            let action = match *child {
                "start" => "apps.start",
                "stop" => "apps.stop",
                "scale" => "apps.scale",
                "restart" => "apps.restart",
                "rollback" => "apps.rollback",
                "logs" => "apps.logs",
                "reconcile" => "apps.reconcile",
                _ => return None,
            };
            Some((action, Some(("appID", id))))
        }
        (&Method::POST, ["clusters"]) => Some(("clusters.register", None)),
        (&Method::POST, ["clusters", id, child]) => {
            let action = match *child {
                "enrollment" => "clusters.enrollment",
                "revoke" => "clusters.revoke",
                "activate" => "clusters.activate",
                _ => return None,
            };
            Some((action, Some(("clusterID", id))))
        }
        (&Method::PUT, ["customers", id, "limits"]) => {
            Some(("limits.set", Some(("customerID", id))))
        }
        (&Method::POST, ["projects", id]) => Some(("projects.create", Some(("projectID", id)))),
        (&Method::PATCH, ["projects", id]) => Some(("projects.update", Some(("projectID", id)))),
        _ => None,
    }
}
fn command(
    method: &Method,
    path: &[&str],
    raw_query: Option<&str>,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Result<Value, ResourceFailure> {
    if path.first() == Some(&"databases") {
        if !bytes.is_empty() {
            return Err(public_error(
                StatusCode::BAD_REQUEST,
                "Database GET requests have no body.",
            ));
        }
        return crate::resource_databases::command(method, &path[1..], raw_query);
    }
    if method == Method::POST && path == ["commands"] {
        let input = read_json(headers, bytes)?;
        if !input
            .get("action")
            .and_then(Value::as_str)
            .is_some_and(|a| {
                ["byok.", "git.", "databases."]
                    .iter()
                    .any(|p| a.starts_with(p))
            })
        {
            return Err(public_error(
                StatusCode::BAD_REQUEST,
                "Use the resource endpoint for this action.",
            ));
        }
        return Ok(input);
    }
    let (target, mut fields) = if method == Method::GET {
        (read_target(path), query(raw_query)?)
    } else {
        let input = read_json(headers, bytes)?;
        let fields = input
            .as_object()
            .ok_or_else(|| public_error(StatusCode::BAD_REQUEST, "Invalid hosting input."))?
            .clone();
        for key in fields.keys() {
            let path_target =
                ["action", "customerID", "clusterID", "projectID", "appID"].contains(&key.as_str());
            let project_cluster =
                matches!(path, ["projects", ..]) && method == Method::POST && key == "clusterID";
            let project_limit = matches!(path, ["customers", _, "limits"]) && key == "projectID";
            if path_target && !project_cluster && !project_limit {
                return Err(public_error(
                    StatusCode::BAD_REQUEST,
                    "Target must come from the request path.",
                ));
            }
        }
        (write_target(method, path), fields)
    };
    let (action, target) = target
        .ok_or_else(|| public_error(StatusCode::NOT_FOUND, "Hosting endpoint is unavailable."))?;
    fields.insert("action".into(), json!(action));
    if let Some((key, value)) = target {
        fields.insert(key.into(), json!(value));
    }
    Ok(Value::Object(fields))
}
async fn handle(
    State(state): State<Arc<ServiceState>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let result: Result<Response, ResourceFailure> = async {
        let actor = resource_auth::authenticate(&state, &headers, Resource::Hosting, false).await?;
        let path = uri
            .path()
            .strip_prefix("/api/hosting/")
            .unwrap_or("")
            .split('/')
            .collect::<Vec<_>>();
        let input = command(&method, &path, uri.query(), &headers, &bytes)?;
        let data = execute(&state, &actor, input).await?;
        let status = if data.get("status").and_then(Value::as_str) == Some("queued") {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        };
        Ok((status, Json(data)).into_response())
    }
    .await;
    result.unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
mod database_read_tests {
    use super::*;
    #[test]
    fn database_rest_paths_use_native_database_read_contract_before_hosting() {
        let read = command(
            &Method::GET,
            &["databases", "projects", "101", "databases"],
            Some("limit=2&cursor=102"),
            &HeaderMap::new(),
            &[],
        );
        assert_eq!(
            read.unwrap_or_else(|_| panic!("database route rejected")),
            json!({"action":"databases.list","projectID":"101","limit":2,"cursor":"102"})
        );
        for raw in [
            "limit=101",
            "cursor=-1",
            "limit=2&limit=3",
            "authority=x",
            "action=databases.backup",
        ] {
            assert!(
                command(
                    &Method::GET,
                    &["databases", "projects", "101", "databases"],
                    Some(raw),
                    &HeaderMap::new(),
                    &[]
                )
                .is_err()
            );
        }
        assert!(
            command(
                &Method::POST,
                &["databases", "101", "restore"],
                None,
                &HeaderMap::new(),
                &[]
            )
            .is_err()
        );
    }
}
