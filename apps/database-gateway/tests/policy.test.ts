import test from "node:test";
import assert from "node:assert/strict";
import { authorizeRpc, publicResult } from "../src/policy";
import type { RuntimeAccess } from "@spitzli/database-contracts";

const access: RuntimeAccess = { scopeID: "123", subject: "alice", connectionID: "internal", profile: "read", runtimeOrigin: "https://private", proxySecret: "secret", expiresAt: new Date(Date.now()+60000).toISOString() };
test("RPC policy denies arbitrary connection targets, configuration, uploads and unknown commands", () => {
  for (const command of ["save_connection", "save_config", "install_plugin", "dump_database", "import_database", "run_ssh_command", "new_upstream_command"]) assert.throws(() => authorizeRpc(command, {}, access));
  assert.throws(() => authorizeRpc("execute_query", { connectionId: "someone-else", query: "select 1" }, access));
  assert.throws(() => authorizeRpc("insert_record", { connectionId: "123" }, access));
});
test("SQL remains intact and is executed only through the registered connection and DB role", () => {
  assert.deepEqual(authorizeRpc("execute_query", { connectionId: "123", query: "select '123'" }, access), { connectionId: "internal", query: "select '123'" });
});
test("updated upstream metadata and transaction cleanup stay scoped to the granted connection", () => {
  for (const command of ["get_table_query_template", "release_query_session", "set_selected_databases"]) {
    assert.deepEqual(authorizeRpc(command, { connectionId: "123", sessionId: "tab-1" }, access), { connectionId: "internal", sessionId: "tab-1" });
    assert.throws(() => authorizeRpc(command, { connectionId: "other" }, access));
  }
  assert.deepEqual(authorizeRpc("get_data_types", { driver: "postgres" }, access), { driver: "postgres" });
  assert.throws(() => authorizeRpc("execute_clipboard_import", { req: { connection_id: "other" } }, access));
});
test("public results use session-local connection IDs and strip connection secrets", () => {
  const result = publicResult("get_connections", [{ id: "internal", name: "Orders", params: { driver: "postgres", database: "orders", password: "secret", host: "private", username: "owner" } }, { id: "other", params: {} }], access);
  assert.deepEqual(result, [{ id: "123", name: "Orders", params: { driver: "postgres", database: "orders" } }]);
});
test("capabilities reflect the granted profile even when the backend driver supports writes", () => {
  const result = publicResult("get_driver_manifest", { capabilities: { readonly: false, manage_tables: true, user_management: true } }, access) as { capabilities: Record<string, unknown> };
  assert.equal(result.capabilities.readonly, true);
  assert.equal(result.capabilities.manage_tables, false);
  assert.equal(result.capabilities.user_management, false);
});
test("saved SQL tabs survive a new gateway session without retaining the previous scope ID", () => {
  const preferences = { tabs: [{ id: "tab", connectionId: "123", query: "select '123'" }], active_tab_id: "tab" };
  const saved = authorizeRpc("save_editor_preferences", { connectionId: "123", preferences }, access) as { preferences: unknown };
  const restored = publicResult("load_editor_preferences", saved.preferences, { ...access, scopeID: "456" }) as typeof preferences;
  assert.equal(restored.tabs[0].connectionId, "456");
  assert.equal(restored.tabs[0].query, "select '123'");
  assert.throws(() => authorizeRpc("save_editor_preferences", { connectionId: "123", preferences: { tabs: [{ connectionId: "other" }] } }, access));
});
