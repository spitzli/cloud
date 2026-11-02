import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { pluginProvisioningSchemaSQL } from "../../../modules/hosting/src/server/plugin-provisioning-schema";
import { pluginProvisioningRlsSQL } from "../src/db/plugin-provisioning-rls";
import { pluginTemplateSchemaSQL } from "../src/modules/hosting/plugin-template-schema";
import { pluginScopeSQL } from "../src/modules/plugins/scope-rls";
test("numbered upgrade contains current provisioning and publication authority", async () => {
  const sql = await readFile(
    new URL("../drizzle/0048_plugin_app_provisioning.sql", import.meta.url),
    "utf8",
  );
  for (const source of [
    pluginProvisioningSchemaSQL,
    pluginScopeSQL,
    pluginProvisioningRlsSQL,
    pluginTemplateSchemaSQL,
  ])
    expect(sql.includes(source.trim())).toBe(true);
  expect(sql).toContain("FORCE ROW LEVEL SECURITY");
  expect(sql).toContain("provision_setup_agent_read");
});
