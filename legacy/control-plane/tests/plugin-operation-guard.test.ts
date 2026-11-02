import { test, expect } from "bun:test";
import { createHostingOperations } from "../../../modules/hosting/src/server/operations";
test("application guard revalidates plugin authority after checking exact lease", async () => {
  let live = true;
  const row = {
    id: "1",
    cluster_id: "2",
    generation: 3,
    status: "running",
    lease_until: new Date(Date.now() + 100000),
  };
  const db: any = { query: async () => ({ rows: [row] }) };
  const ops = createHostingOperations({
    transaction: async (run: any) => run(db),
    verifyAgent: async () => {},
    operationAuthority: async () => ({ required: true, allowed: live }),
    env: {},
  } as any);
  expect(typeof (ops as any).guardOperation).toBe("function");
  expect(
    await (ops as any).guardOperation(
      { clusterID: "2" },
      { id: "1", generation: 3 },
    ),
  ).toEqual({ valid: true });
  live = false;
  await expect(
    (ops as any).guardOperation({ clusterID: "2" }, { id: "1", generation: 3 }),
  ).rejects.toMatchObject({ status: 403 });
  live = true;
  await expect(
    (ops as any).guardOperation({ clusterID: "2" }, { id: "1", generation: 4 }),
  ).rejects.toMatchObject({ status: 409 });
});
test("revoked queued work is retained as failed instead of starving other project claims", async () => {
  const row: any = {
    id: "1",
    project_id: "2",
    app_id: "3",
    target_revision: 1,
    status: "queued",
  };
  const db: any = {
    query: async (sql: string) => {
      if (sql.startsWith("SELECT o.*")) return { rows: [row] };
      if (sql.includes("SET status='failed'")) row.status = "failed";
      return { rows: [], rowCount: 1 };
    },
  };
  const ops = createHostingOperations({
    transaction: async (run: any) => run(db),
    verifyAgent: async () => ({
      observation: { capabilities: { provisioningGuardVersion: 1 } },
    }),
    operationAuthority: async () => ({ required: true, allowed: false }),
    env: {},
  } as any);
  expect(await ops.claimOperation({ clusterID: "9" } as any)).toBeNull();
  expect(row.status).toBe("failed");
});
