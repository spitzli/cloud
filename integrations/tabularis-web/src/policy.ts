import type { RuntimeAccess } from "@spitzli/database-contracts";

export class GatewayError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
const commands = new Set(
  `get_connections get_connections_with_groups get_connection_by_id get_connection_groups
get_registered_drivers get_driver_manifest get_connection_metadata get_active_connections register_active_connection disconnect_connection test_connection
get_available_databases set_selected_databases get_schemas get_tables get_columns get_foreign_keys get_indexes get_views get_view_columns get_table_query_template get_data_types
get_materialized_views get_materialized_view_columns get_materialized_view_definition get_routines get_triggers get_schema_snapshot
get_selected_schemas set_selected_schemas get_schema_preference set_schema_preference get_view_definition get_routine_parameters
get_routine_definition build_routine_call_sql get_routine_create_template get_routine_edit_script get_trigger_definition
execute_query execute_query_batch count_query explain_query_plan cancel_query release_query_session ping_connection get_server_now
get_saved_queries save_query update_saved_query delete_saved_query get_query_history add_query_history_entry delete_query_history_entry clear_query_history
load_editor_preferences save_editor_preferences delete_editor_preferences get_last_active_connection set_last_active_connection
get_last_open_connections set_last_open_connections get_keybindings get_ui_state set_ui_state
is_debug_mode list_connection_tags get_connection_tags list_notebooks create_notebook load_notebook save_notebook rename_notebook delete_notebook delete_ui_state
insert_record update_record delete_record create_view alter_view drop_view refresh_materialized_view create_trigger drop_trigger drop_routine
get_create_table_sql get_add_column_sql get_alter_column_sql get_create_index_sql get_create_foreign_key_sql drop_foreign_key_action drop_index_action`.split(
    /\s+/,
  ),
);
const writes = new Set([
  "insert_record",
  "update_record",
  "delete_record",
  "refresh_materialized_view",
]);
const ddl = new Set([
  "create_view",
  "alter_view",
  "drop_view",
  "create_trigger",
  "drop_trigger",
  "drop_routine",
  "drop_foreign_key_action",
  "drop_index_action",
]);

export function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new GatewayError(400, "Invalid database request.");
  return value as Record<string, unknown>;
}

/** SQL is deliberately not parsed for authorization. The bound database role is authoritative. */
export function authorizeRpc(
  command: string,
  input: unknown,
  access: RuntimeAccess,
): unknown {
  if (
    !commands.has(command) ||
    (writes.has(command) && access.profile === "read") ||
    (ddl.has(command) && access.profile !== "schema")
  ) {
    throw new GatewayError(403, "This database operation is not permitted.");
  }
  const args = input == null ? null : { ...record(input) };
  const connection = (value: unknown) => {
    if (value !== access.scopeID)
      throw new GatewayError(403, "Invalid database connection.");
    return access.connectionID;
  };
  if (args) {
    for (const field of ["connectionId", "connection_id"]) {
      if (args[field] != null) args[field] = connection(args[field]);
    }
    if (args.connectionIds !== undefined) {
      if (!Array.isArray(args.connectionIds))
        throw new GatewayError(400, "Invalid connections.");
      args.connectionIds = args.connectionIds.map(connection);
    }
    if (command === "get_connection_by_id") args.id = connection(args.id);
    if (command === "save_editor_preferences") {
      const preferences = record(args.preferences);
      if (!Array.isArray(preferences.tabs))
        throw new GatewayError(400, "Invalid editor preferences.");
      args.preferences = {
        ...preferences,
        tabs: preferences.tabs.map((tab) => ({
          ...record(tab),
          connectionId: connection(record(tab).connectionId),
        })),
      };
    }
  }
  if (command === "test_connection") {
    const request = record(args?.request);
    connection(request.connection_id);
    // Gateway probes the saved upstream connection; caller parameters are never forwarded.
    return { request: { connection_id: access.connectionID } };
  }
  return args;
}

function capabilities(value: unknown, access: RuntimeAccess) {
  const result = { ...record(value) };
  if (result.capabilities)
    result.capabilities = {
      ...record(result.capabilities),
      readonly:
        access.profile === "read" ||
        record(result.capabilities).readonly === true,
      manage_tables:
        access.profile === "schema" &&
        record(result.capabilities).manage_tables !== false,
      alter_column:
        access.profile === "schema" &&
        record(result.capabilities).alter_column === true,
      create_foreign_keys:
        access.profile === "schema" &&
        record(result.capabilities).create_foreign_keys === true,
      user_management: false,
      routine_management:
        access.profile === "schema" &&
        record(result.capabilities).routine_management === true,
    };
  return result;
}

export function publicResult(
  command: string,
  value: unknown,
  access: RuntimeAccess,
): unknown {
  if (command === "load_editor_preferences" && value) {
    const preferences = record(value);
    return {
      ...preferences,
      tabs: Array.isArray(preferences.tabs)
        ? preferences.tabs.map((tab) => ({
            ...record(tab),
            connectionId: access.scopeID,
          }))
        : [],
    };
  }
  const connection = (entry: unknown) => {
    const row = record(entry),
      params = record(row.params);
    return {
      id: access.scopeID,
      name: row.name,
      ...(row.environment ? { environment: row.environment } : {}),
      params: { driver: params.driver, database: params.database },
    };
  };
  if (command === "get_connections")
    return (Array.isArray(value) ? value : [])
      .filter((row) => record(row).id === access.connectionID)
      .map(connection);
  if (command === "get_connection_by_id") return connection(value);
  if (command === "get_connections_with_groups")
    return {
      groups: [],
      connections: publicResult(
        "get_connections",
        record(value).connections,
        access,
      ),
    };
  if (
    command === "get_connection_groups" ||
    command === "list_connection_tags" ||
    command === "get_connection_tags"
  )
    return [];
  if (command === "get_registered_drivers")
    return (Array.isArray(value) ? value : []).map((row) =>
      capabilities(row, access),
    );
  if (
    command === "get_driver_manifest" ||
    command === "get_connection_metadata"
  )
    return capabilities(value, access);
  if (
    command === "get_active_connections" ||
    command === "get_last_open_connections"
  )
    return Array.isArray(value) && value.includes(access.connectionID)
      ? [access.scopeID]
      : [];
  if (command === "get_last_active_connection")
    return value === access.connectionID ? access.scopeID : null;
  return value;
}
