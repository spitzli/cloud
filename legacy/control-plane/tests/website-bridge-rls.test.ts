import assert from "node:assert/strict";
import test from "node:test";
import { buildDatabaseRls } from "../src/db/database-rls-schema";

test("website bridge tables retain the website runtime scope during platform policy reinstall", () => {
  const names = ["cms_bridge_transfers", "cms_bridge_transfer_chunks"];
  const plan = buildDatabaseRls(
    names.map((name) => ({ schema: "webdock", name, columns: ["id"] })),
  );
  assert.deepEqual(plan.unknownTables, []);
  for (const name of names) {
    assert.ok(
      plan.sql.includes(
        `ALTER TABLE "webdock"."${name}" FORCE ROW LEVEL SECURITY`,
      ),
    );
    for (const role of ["webdock_runtime", "webdock_browser_website"])
      assert.ok(
        plan.sql.includes(
          `CREATE POLICY ${role}_insert ON "webdock"."${name}" FOR INSERT TO ${role} WITH CHECK(true)`,
        ),
      );
    for (const role of [
      "webdock_auth_runtime",
      "webdock_worker_runtime",
      "webdock_admin_runtime",
      "webdock_registry_runtime",
    ])
      assert.ok(
        !plan.sql.includes(
          `GRANT SELECT,INSERT,UPDATE,DELETE ON "webdock"."${name}" TO ${role}`,
        ),
      );
  }
});
