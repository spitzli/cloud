import { test, expect } from "bun:test";
import { AsyncLocalStorage } from "node:async_hooks";
import { createHostingDatabaseAgent } from "../../../modules/databases/src/server/database-agent";
import { createHostingDatabaseOrders } from "../../../modules/databases/src/server/database-orders";
import {
  resolveConsolePrincipal,
  type ConsolePrincipal,
} from "../src/modules/auth/clerk-identity";

const reference = {
  issuer: "https://fixture.clerk.accounts.dev",
  userID: "user_one",
  sessionID: "sess_one",
  organizationID: "org_one",
};
const actor = {
  subject: "10",
  sessionID: reference.sessionID,
  source: "studio" as const,
  scopes: ["hosting:read", "hosting:write"],
  consoleIdentity: reference,
};
const agent = { clusterID: "30", generation: 1, credentialHash: "unused" };

function fixture(scenario = "clerk") {
  const context = new AsyncLocalStorage<ConsolePrincipal>();
  const calls: string[] = [];
  const row: any = {
    id: "40",
    subject: actor.subject,
    actor_session: actor.sessionID,
    project_id: "20",
    status: "queued",
    generation: 0,
    checkpoint: null,
    backend_id: "50",
    database_id: "60",
    database_action: "create",
    database_revision: 1,
    desired: scenario === "legacy" ? {} : { consoleIdentity: reference },
  };
  const db: any = {
    query: async (sql: string) => {
      calls.push(sql);
      if (sql.startsWith("SELECT verified"))
        return { rows: [{ verified: true, ownership: "platform" }] };
      if (sql.startsWith("SELECT o.*")) return { rows: [row] };
      if (sql.startsWith("SELECT * FROM webdock_auth.hosting_database_backend"))
        return {
          rows: [{ id: "50", revision: 1, cnpg_uid: null, profile: {} }],
        };
      if (sql.startsWith("SELECT d.*,p.plan"))
        return {
          rows: [
            {
              id: "60",
              revision: 1,
              credential_revision: 1,
              plan: { connectionLimit: 10 },
            },
          ],
        };
      return { rows: [] };
    },
  };
  let revalidations = 0,
    authorizations = 0;
  const operations = createHostingDatabaseAgent({
    transaction: async (run: any) => run(db),
    clusterLock: async () => {},
    verifyAgent: async () => ({
      observation: { capabilities: { databaseVersion: 1 } },
      last_seen: new Date(),
    }),
    operationAuthority: async () => ({
      required: true,
      allowed: scenario !== "revoked-plugin",
    }),
    withStoredIdentity: async (
      stored: typeof reference,
      run: (principal: ConsolePrincipal) => Promise<unknown>,
    ) => {
      revalidations++;
      const principal = await resolveConsolePrincipal(
        {
          issuer: reference.issuer,
          authorizedParties: ["https://console.example.test"],
          serviceKey: "s".repeat(32),
        },
        {
          verify: async () => {
            throw Error("Stored reference does not invent a browser request");
          },
          session: async () => ({
            id: "sess_one",
            userId: "user_one",
            status: scenario === "revoked-session" ? "revoked" : "active",
            expireAt: scenario === "expired-session" ? 1 : Date.now() + 60000,
          }),
          user: async () => ({
            id: "user_one",
            banned: false,
            locked: false,
            primaryEmailAddressId: "email",
            emailAddresses: [
              { id: "email", verification: { status: "verified" } },
            ],
          }),
          identity: async () => ({
            id: scenario === "wrong-subject" ? "99" : actor.subject,
            name: "Fixture",
            email: "fixture@example.test",
            role: "user",
            banned: false,
            enabled: true,
            emailVerified: true,
          }),
          organization: async () => ({
            id: "70",
            customerID: "80",
            active: true,
            enabled: true,
          }),
          memberships: async () => [
            {
              organization: { id: "org_one" },
              role: "org:admin",
              permissions:
                scenario === "revoked-permission"
                  ? []
                  : ["org:databases:manage"],
            },
          ],
        },
        {
          iss: stored.issuer,
          sub: stored.userID,
          sid: stored.sessionID,
          orgId: stored.organizationID,
          azp: "https://console.example.test",
          exp: Math.floor(Date.now() / 1000) + 60,
        },
      );
      return context.run(principal, () => run(principal));
    },
    authorizeHosting: async (value: typeof actor, target: any) => {
      authorizations++;
      expect(value.subject).toBe(row.subject);
      expect(target.projectID).toBe("20");
      if (
        scenario !== "legacy" &&
        !context.getStore()?.permissions?.includes("org:databases:manage")
      )
        throw Error("No current authorized session");
      return { operator: false };
    },
    decryptSecret: () => ({
      applicationPassword: "fixture",
      migrationPassword: "fixture",
    }),
    env: { BETTER_AUTH_SECRET: "fixture".repeat(8) },
  } as any);
  return {
    operations,
    calls,
    row,
    context,
    counts: () => ({ revalidations, authorizations }),
  };
}

test("database claim revalidates stored Clerk identity within its principal context", async () => {
  const f = fixture();
  expect((await f.operations.claimDatabaseOperation(agent))?.phase).toBe(
    "apply",
  );
  expect(f.counts()).toEqual({ revalidations: 1, authorizations: 1 });
  expect(f.context.getStore()).toBeUndefined();
});
for (const scenario of [
  "revoked-session",
  "expired-session",
  "wrong-subject",
  "revoked-permission",
  "revoked-plugin",
])
  test(`database claim never leases ${scenario}`, async () => {
    const f = fixture(scenario);
    expect(await f.operations.claimDatabaseOperation(agent)).toBeNull();
    expect(f.calls.some((sql) => sql.includes("SET status='running'"))).toBe(
      false,
    );
    expect(
      f.calls.some((sql) =>
        sql.includes("SET status='needs-reconciliation' WHERE id=$1"),
      ),
    ).toBe(true);
    expect(f.context.getStore()).toBeUndefined();
  });
test("legacy database claims retain existing session authorization", async () => {
  const f = fixture("legacy");
  expect((await f.operations.claimDatabaseOperation(agent))?.phase).toBe(
    "apply",
  );
  expect(f.counts()).toEqual({ revalidations: 0, authorizations: 1 });
});
test("database enqueue retains only the trusted actor's Console reference", async () => {
  let desired: any;
  const db: any = {
    query: async (sql: string, values: any[]) => {
      if (sql.startsWith("SELECT cluster_id"))
        return { rows: [{ cluster_id: "30" }] };
      if (sql.startsWith("SELECT customer_id"))
        return { rows: [{ customer_id: "80", project_id: "20" }] };
      if (sql.startsWith("INSERT INTO webdock_auth.hosting_operation")) {
        desired = JSON.parse(values[7]);
        return { rows: [{ id: "40" }] };
      }
      return { rows: [] };
    },
  };
  const service = createHostingDatabaseOrders({} as any);
  await service.enqueueDatabase(db, actor, "50", "60", "create", 1);
  expect(desired.consoleIdentity).toEqual(reference);
  await service.enqueueDatabase(
    db,
    { ...actor, consoleIdentity: undefined },
    "50",
    "60",
    "create",
    1,
  );
  expect(desired.consoleIdentity).toBeUndefined();
});
