import { test, expect } from "bun:test";
import { createHostingDatabaseMaintenance } from "../../../modules/databases/src/server/database-maintenance";

function fixture(changes: Record<string, unknown> = {}) {
  const reference = {
    issuer: "https://fixture.clerk.accounts.dev",
    userID: "user_new",
    sessionID: "sess_new",
    organizationID: null,
  };
  const actor = {
    subject: "10",
    sessionID: "sess_new",
    source: "studio" as const,
    scopes: ["hosting:read", "hosting:write"],
    consoleIdentity: reference,
  };
  const pending = {
    id: "40",
    backend_id: "50",
    action: "create",
    generation: 0,
    lease_until: null,
    lease_token_hash: null,
    checkpoint: null,
    revision: 1,
    ...changes,
  };
  const backend = {
    id: "50",
    cluster_id: "30",
    cnpg_uid: null,
    revision: 1,
    observed_revision: 0,
    state: "provisioning",
  };
  const row = {
    id: "60",
    backend_id: "50",
    project_id: "20",
    customer_id: "80",
    revision: 1,
    state: "needs-reconciliation",
    operation_id: "40",
  };
  const writes: { sql: string; values: any[] }[] = [];
  const db: any = {
    query: async (sql: string, values: any[] = []) => {
      if (sql.includes("FROM webdock_auth.hosting_database_backend"))
        return { rows: [backend] };
      if (sql.includes("FROM webdock_auth.hosting_database WHERE"))
        return { rows: [row] };
      if (sql.includes("FROM webdock_auth.hosting_database_job"))
        return { rows: [pending] };
      if (sql.startsWith("UPDATE")) writes.push({ sql, values });
      return { rows: [] };
    },
  };
  const service = createHostingDatabaseMaintenance({
    databaseAccess: async (
      _db: any,
      _actor: any,
      _id: string,
      write: boolean,
      operator: boolean,
    ) => {
      expect(write && operator).toBe(true);
      return row;
    },
    assertDatabaseMaintenanceAllowed: () => {},
    clusterLock: async () => {},
    replay: async () => ({ hash: "fixture" }),
    freshDatabaseCluster: async () => ({}),
    databaseView: async () => ({ id: row.id }),
    recordResult: async () => {},
    audit: async () => {},
  } as any);
  return {
    writes,
    actor,
    backend,
    row,
    pending,
    run: () =>
      service.databaseMaintenance(db, actor, {
        action: "databases.reconcile",
        databaseID: "60",
        revision: 1,
        idempotencyKey: "fixture-reconcile",
      }),
  };
}
test("operator can requeue the same never-leased initial create with refreshed identity", async () => {
  const f = fixture();
  await f.run();
  const queue = f.writes.find((w) => w.sql.includes("SET status='queued'"))!;
  expect(queue).toBeDefined();
  expect(queue.values).toContain(f.actor.subject);
  expect(queue.values).toContain(f.actor.sessionID);
  expect(queue.values).toContain(JSON.stringify(f.actor.consoleIdentity));
  expect(f.writes.some((w) => w.sql.includes("SET state='provisioning'"))).toBe(
    true,
  );
  expect(f.writes.some((w) => w.sql.includes("SET checkpoint"))).toBe(false);
});
for (const changes of [
  { generation: 1 },
  { lease_until: new Date() },
  { lease_token_hash: "issued" },
  { action: "migrate" },
  { revision: 2 },
  { checkpoint: {} },
])
  test(`unconfirmed operation ${JSON.stringify(changes)} cannot replay`, async () => {
    const f = fixture(changes);
    await expect(f.run()).rejects.toThrow("inspect");
    expect(f.writes).toHaveLength(0);
  });
test("observed/changed installation is not mistaken for an untouched create", async () => {
  for (const change of [
    { cnpg_uid: "observed" },
    { observed_revision: 1 },
    { revision: 2 },
    { state: "ready" },
  ]) {
    const f = fixture();
    Object.assign(f.backend, change);
    await expect(f.run()).rejects.toThrow("inspect");
    expect(f.writes).toHaveLength(0);
  }
});
test("confirmed reconciliation still observes and never replays create", async () => {
  const f = fixture({ generation: 1, checkpoint: { phase: "configuring" } });
  f.backend.cnpg_uid = "observed" as any;
  await f.run();
  const checkpoint = f.writes.find((w) => w.sql.includes("SET checkpoint"))!;
  expect(JSON.parse(checkpoint.values[1])).toEqual({
    phase: "observing",
    cnpgUID: "observed",
  });
  expect(f.writes.some((w) => w.sql.includes("SET state='provisioning'"))).toBe(
    false,
  );
});
