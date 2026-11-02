import { test, expect } from "bun:test";
import { buildDatabaseRls } from "../src/db/database-rls-schema";
import { setupPolicyFor } from "../src/db/database-setup-rls";
test("Clerk session mapping reads use exact setup/database-order actors or live approved Git releases, never blanket agent authority", () => {
  const plan = buildDatabaseRls([
    {
      schema: "webdock_auth",
      name: "clerk_identity_mapping",
      columns: [
        "issuer",
        "external_subject",
        "local_subject",
        "revoked_at",
        "created_at",
      ],
    },
    {
      schema: "webdock_auth",
      name: "clerk_session_mapping",
      columns: [
        "issuer",
        "external_subject",
        "external_session",
        "local_session_id",
        "expires_at",
      ],
    },
  ]);
  const policies = plan.sql
    .split(";")
    .filter(
      (sql) =>
        sql.includes("clerk_session_mapping") &&
        sql.includes("FOR SELECT TO webdock_worker_runtime"),
    )
    .join(";");
  expect(policies).toContain("r.status='deploying'");
  expect(policies).toContain("r.lease_until>now()");
  expect(policies).toContain("request_scope()->>'sessionID'");
  expect(policies).toContain("request_scope()->>'subject'");
  expect(policies).toContain("setup_identity");
  expect(policies).not.toContain("ARRAY['agent'");
  expect(plan.sql).toContain(
    "GRANT SELECT(issuer,external_session,external_subject,local_session_id,expires_at)",
  );
  const setup = setupPolicyFor({
    schema: "webdock_auth",
    name: "clerk_identity_mapping",
    columns: [],
  });
  expect(setup?.write).toBe("false");
  expect(setup?.read).toContain("setup_identity");
});
