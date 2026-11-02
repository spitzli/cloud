//! Saved-connection RPC policy. PostgreSQL privileges, not SQL text parsing, authorize queries.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Read,
    Write,
    Schema,
}
const COMMANDS: &str = "get_connections get_connections_with_groups get_connection_by_id get_connection_groups get_registered_drivers get_driver_manifest get_connection_metadata get_active_connections register_active_connection disconnect_connection test_connection get_available_databases set_selected_databases get_schemas get_tables get_columns get_foreign_keys get_indexes get_views get_view_columns get_table_query_template get_data_types get_materialized_views get_materialized_view_columns get_materialized_view_definition get_routines get_triggers get_schema_snapshot get_selected_schemas set_selected_schemas get_schema_preference set_schema_preference get_view_definition get_routine_parameters get_routine_definition build_routine_call_sql get_routine_create_template get_routine_edit_script get_trigger_definition execute_query execute_query_batch count_query explain_query_plan cancel_query release_query_session ping_connection get_server_now get_saved_queries save_query update_saved_query delete_saved_query get_query_history add_query_history_entry delete_query_history_entry clear_query_history load_editor_preferences save_editor_preferences delete_editor_preferences get_last_active_connection set_last_active_connection get_last_open_connections set_last_open_connections get_keybindings get_ui_state set_ui_state is_debug_mode list_connection_tags get_connection_tags list_notebooks create_notebook load_notebook save_notebook rename_notebook delete_notebook delete_ui_state insert_record update_record delete_record create_view alter_view drop_view refresh_materialized_view create_trigger drop_trigger drop_routine get_create_table_sql get_add_column_sql get_alter_column_sql get_create_index_sql get_create_foreign_key_sql drop_foreign_key_action drop_index_action";
const WRITES: &[&str] = &[
    "insert_record",
    "update_record",
    "delete_record",
    "refresh_materialized_view",
];
const DDL: &[&str] = &[
    "create_view",
    "alter_view",
    "drop_view",
    "create_trigger",
    "drop_trigger",
    "drop_routine",
    "drop_foreign_key_action",
    "drop_index_action",
];
fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or(Error::new(400, "Invalid database request."))
}
fn connection(value: &Value, scope: &str, internal: &str) -> Result<Value> {
    if value.as_str() != Some(scope) {
        return Err(Error::new(403, "Invalid database connection."));
    }
    Ok(json!(internal))
}
pub fn authorize_rpc(
    command: &str,
    input: Value,
    scope: &str,
    internal: &str,
    profile: Profile,
) -> Result<Value> {
    if !COMMANDS
        .split_ascii_whitespace()
        .any(|allowed| allowed == command)
        || (WRITES.contains(&command) && profile == Profile::Read)
        || (DDL.contains(&command) && profile != Profile::Schema)
    {
        return Err(Error::new(403, "This database operation is not permitted."));
    }
    let mut args = if input.is_null() {
        None
    } else {
        Some(object(&input)?.clone())
    };
    if let Some(args) = &mut args {
        for field in ["connectionId", "connection_id"] {
            if let Some(value) = args.get(field).filter(|v| !v.is_null()) {
                let value = connection(value, scope, internal)?;
                args.insert(field.into(), value);
            }
        }
        if let Some(value) = args.get("connectionIds") {
            let values = value
                .as_array()
                .ok_or(Error::new(400, "Invalid connections."))?
                .iter()
                .map(|v| connection(v, scope, internal))
                .collect::<Result<Vec<_>>>()?;
            args.insert("connectionIds".into(), json!(values));
        }
        if command == "get_connection_by_id" {
            args.insert(
                "id".into(),
                connection(args.get("id").unwrap_or(&Value::Null), scope, internal)?,
            );
        }
        if command == "save_editor_preferences" {
            let mut preferences = object(args.get("preferences").unwrap_or(&Value::Null))?.clone();
            let tabs = preferences
                .get("tabs")
                .and_then(Value::as_array)
                .ok_or(Error::new(400, "Invalid editor preferences."))?;
            let tabs = tabs
                .iter()
                .map(|value| {
                    let mut tab = object(value)?.clone();
                    let id = connection(
                        tab.get("connectionId").unwrap_or(&Value::Null),
                        scope,
                        internal,
                    )?;
                    tab.insert("connectionId".into(), id);
                    Ok(Value::Object(tab))
                })
                .collect::<Result<Vec<_>>>()?;
            preferences.insert("tabs".into(), json!(tabs));
            args.insert("preferences".into(), Value::Object(preferences));
        }
    }
    if command == "test_connection" {
        let args = args
            .as_ref()
            .ok_or(Error::new(400, "Invalid database request."))?;
        let request = object(args.get("request").unwrap_or(&Value::Null))?;
        connection(
            request.get("connection_id").unwrap_or(&Value::Null),
            scope,
            internal,
        )?;
        return Ok(json!({"request":{"connection_id":internal}}));
    }
    Ok(args.map(Value::Object).unwrap_or(Value::Null))
}
fn capabilities(value: Value, profile: Profile) -> Result<Value> {
    let mut result = object(&value)?.clone();
    if let Some(value) = result
        .get("capabilities")
        .filter(|v| !v.is_null() && **v != false)
    {
        let mut caps = object(value)?.clone();
        caps.insert(
            "readonly".into(),
            json!(profile == Profile::Read || caps.get("readonly") == Some(&Value::Bool(true))),
        );
        caps.insert(
            "manage_tables".into(),
            json!(
                profile == Profile::Schema
                    && caps.get("manage_tables") != Some(&Value::Bool(false))
            ),
        );
        for name in ["alter_column", "create_foreign_keys", "routine_management"] {
            caps.insert(
                name.into(),
                json!(profile == Profile::Schema && caps.get(name) == Some(&Value::Bool(true))),
            );
        }
        caps.insert("user_management".into(), json!(false));
        result.insert("capabilities".into(), Value::Object(caps));
    }
    Ok(Value::Object(result))
}
pub fn public_result(
    command: &str,
    value: Value,
    scope: &str,
    internal: &str,
    profile: Profile,
) -> Result<Value> {
    if command == "load_editor_preferences" && !value.is_null() && value != false {
        let mut preferences = object(&value)?.clone();
        let tabs = preferences
            .get("tabs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .map(|value| {
                let mut tab = object(value)?.clone();
                tab.insert("connectionId".into(), json!(scope));
                Ok(Value::Object(tab))
            })
            .collect::<Result<Vec<_>>>()?;
        preferences.insert("tabs".into(), json!(tabs));
        return Ok(Value::Object(preferences));
    }
    let public_connection = |value: &Value| -> Result<Value> {
        let row = object(value)?;
        let params = object(row.get("params").unwrap_or(&Value::Null))?;
        if row.get("id").and_then(Value::as_str) != Some(internal) {
            return Err(Error::new(403, "Invalid database connection."));
        }
        let mut result = json!({"id":scope,"name":row.get("name"),"params":{"driver":params.get("driver"),"database":params.get("database")}});
        if let Some(environment) = row
            .get("environment")
            .filter(|v| v.is_string() && v.as_str() != Some(""))
        {
            result["environment"] = environment.clone();
        }
        Ok(result)
    };
    match command {
        "get_connections" => Ok(json!(
            value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter(|v| v.get("id").and_then(Value::as_str) == Some(internal))
                .map(public_connection)
                .collect::<Result<Vec<_>>>()?
        )),
        "get_connection_by_id" => public_connection(&value),
        "get_connections_with_groups" => Ok(
            json!({"groups":[],"connections":public_result("get_connections",object(&value)?.get("connections").cloned().unwrap_or(Value::Null),scope,internal,profile)?}),
        ),
        "get_connection_groups" | "list_connection_tags" | "get_connection_tags" => Ok(json!([])),
        "get_registered_drivers" => Ok(json!(
            value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .cloned()
                .map(|v| capabilities(v, profile))
                .collect::<Result<Vec<_>>>()?
        )),
        "get_driver_manifest" | "get_connection_metadata" => capabilities(value, profile),
        "get_active_connections" | "get_last_open_connections" => Ok(
            if value
                .as_array()
                .is_some_and(|v| v.contains(&json!(internal)))
            {
                json!([scope])
            } else {
                json!([])
            },
        ),
        "get_last_active_connection" => Ok(if value == internal {
            json!(scope)
        } else {
            Value::Null
        }),
        _ => Ok(value),
    }
}
