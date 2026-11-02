import { test, expect } from "bun:test";
import { buildDatabaseRls } from "../src/db/database-rls-schema";
test("preflight authority is an explicit SELECT-only policy and does not expand unrelated tables", () => {
  const plan = buildDatabaseRls([
    {
      schema: "webdock_auth",
      name: "tenant_storage",
      columns: ["id", "customer_id"],
    },
    { schema: "webdock_admin", name: "cms_instances", columns: ["id"] },
    { schema: "webdock_admin", name: "audit_events", columns: ["id"] },
  ]);
  const statements = plan.sql
    .split(";")
    .filter((sql) => sql.includes("installation-preflight"));
  expect(statements.length).toBe(1);
  for (const sql of statements) {
    expect(sql).toContain("FOR SELECT TO webdock_worker_runtime");
    expect(sql).not.toContain("audit_events");
    expect(sql).not.toContain("cms_instances");
  }
});
