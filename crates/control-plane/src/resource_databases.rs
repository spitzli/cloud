//! Bearer-only database metadata uses native ManagedCommand validation, never Hosting parsing.
use crate::{
    http::ServiceState,
    resource_auth::ResourceFailure,
    resource_registry::{public_error, unavailable},
};
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::HashSet;
use webdock_core::features::OperationKind;
use webdock_oauth::McpAuthentication;

pub(crate) fn is_database(input: &Value) -> bool {
    input
        .get("action")
        .and_then(Value::as_str)
        .is_some_and(|a| a.starts_with("databases."))
}
pub(crate) async fn execute(
    state: &ServiceState,
    actor: &McpAuthentication,
    input: Value,
) -> Result<Value, ResourceFailure> {
    // Reject all non-allowlisted actions, authority injection and secrets before any domain dispatch.
    webdock_databases::DatabaseReadCommand::parse(input.clone()).map_err(|_| invalid())?;
    state
        .config
        .features
        .require("databases", OperationKind::Read)
        .map_err(|_| unavailable())?;
    state
        .databases
        .as_ref()
        .ok_or_else(unavailable)?
        .service
        .execute_mcp_read(state.oauth.as_ref().ok_or_else(unavailable)?, actor, input)
        .await
        .map_err(|e| {
            public_error(
                StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                e.message,
            )
        })
}
fn invalid() -> ResourceFailure {
    public_error(
        StatusCode::BAD_REQUEST,
        "Invalid database metadata request.",
    )
}

pub(crate) fn command(
    method: &Method,
    path: &[&str],
    raw_query: Option<&str>,
) -> Result<Value, ResourceFailure> {
    if *method != Method::GET {
        return Err(public_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "Database bearer mutations are unavailable.",
        ));
    }
    let (action, target) = match path {
        ["customers", id, "projects"] => ("databases.projects.list", Some(("customerID", *id))),
        ["projects", id, "databases"] => ("databases.list", Some(("projectID", *id))),
        ["projects", id, "permissions"] => ("databases.permissions", Some(("projectID", *id))),
        ["projects", id, "plans"] => ("databases.plans.list", Some(("projectID", *id))),
        ["plans"] => ("databases.plans.list", None),
        [id, "backups"] => ("databases.backupPolicies.get", Some(("databaseID", *id))),
        [id, "operations", _] => ("databases.operations.get", Some(("databaseID", *id))),
        [id] => ("databases.get", Some(("databaseID", *id))),
        _ => {
            return Err(public_error(
                StatusCode::NOT_FOUND,
                "Database metadata endpoint is unavailable.",
            ));
        }
    };
    let mut input = json!({"action":action});
    if let Some((key, id)) = target {
        input[key] = json!(id);
    }
    if let [_, "operations", id] = path {
        input["operationID"] = json!(id);
    }
    let mut seen = HashSet::new();
    for (key, value) in url::form_urlencoded::parse(raw_query.unwrap_or("").as_bytes()) {
        if !seen.insert(key.to_string()) {
            return Err(invalid());
        }
        match key.as_ref() {
            "limit" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                input["limit"] = json!(value.parse::<u64>().map_err(|_| invalid())?)
            }
            "cursor" => input["cursor"] = json!(value),
            _ => return Err(invalid()),
        }
    }
    webdock_databases::DatabaseReadCommand::parse(input.clone()).map_err(|_| invalid())?;
    Ok(input)
}

pub(crate) fn tool(name: &str, args: Value) -> Result<Option<Value>, ResourceFailure> {
    let Some((action, _)) = webdock_databases::DATABASE_READ_TOOLS
        .iter()
        .find(|(_, tool)| *tool == name)
    else {
        return Ok(None);
    };
    let mut input = args.as_object().ok_or_else(invalid)?.clone();
    if input.contains_key("action") {
        return Err(invalid());
    }
    input.insert("action".into(), json!(action));
    let input = Value::Object(input);
    webdock_databases::DatabaseReadCommand::parse(input.clone()).map_err(|_| invalid())?;
    Ok(Some(input))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_tool_mapping_rejects_action_or_secret_injection() {
        assert!(
            tool("get_managed_database", json!({"databaseID":"101"}))
                .unwrap_or_else(|_| panic!("read rejected"))
                .is_some()
        );
        for field in [
            "action",
            "authority",
            "nativeAuthority",
            "writeCredentials",
            "token",
        ] {
            let mut value = json!({"databaseID":"101"});
            value[field] = json!("injected");
            assert!(tool("get_managed_database", value).is_err());
        }
        assert!(
            tool("restore_database", json!({}))
                .unwrap_or_else(|_| panic!("unknown tool handling"))
                .is_none()
        );
    }
}
