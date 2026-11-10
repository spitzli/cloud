import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createDatabaseService,
  type DatabaseServiceDependencies,
} from "./service";
import type { DatabaseReader } from "@spitzli/core/identity";
import type { HostingActor } from "@spitzli/hosting-contracts";
const actor: HostingActor = {
  subject: "local-user",
  sessionID: "local-session",
  source: "studio",
  scopes: ["hosting:read", "hosting:write"],
};
function fixture(rowChanges: Record<string, unknown> = {}) {
  const calls: string[] = [];
  const targets: unknown[] = [];
  const row = {
    id: "123",
    project_id: "234",
    customer_id: "345",
    name: "Customer database",
    backend_id: "456",
    revision: 1,
    credential_revision: 2,
    confirmed_credential_revision: 2,
    state: "ready",
    backend_state: "ready",
    endpoint_ready: true,
    ca_certificate: "private-ca",
    confirmed_credentials_encrypted: "private-sealed",
    ...rowChanges,
  };
  const db = {
    query: async (sql: string) => {
      calls.push(sql);
      if (sql.includes("FOR SHARE OF d,b")) return { rows: [row] };
      if (sql.includes("INSERT INTO webdock_auth.managed_database_launch"))
        return { rows: [{ expires_at: new Date(Date.now() + 60000) }] };
      throw Error("Unexpected query");
    },
  } as unknown as DatabaseReader;
  const dependencies: DatabaseServiceDependencies = {
    storedConsoleIdentity: () => null,
    withStoredIdentity: async () => {
      throw Error("Unexpected provider identity");
    },
    authorizeHosting: async (_actor, target) => {
      targets.push(target);
      return { operator: false, canWrite: true };
    },
    hostingAudit: async () => {},
    encryptMailSecret: () => {
      throw Error("Unexpected encryption");
    },
    decryptMailSecret: () => {
      throw Error("Unexpected decryption");
    },
    gatewayOrigin: () => "https://gateway.example",
  };
  return {
    db,
    calls,
    targets,
    service: createDatabaseService(dependencies),
    dependencies,
  };
}
test("managed launch retains row locks, tenant-admin authorization and credential-free public response", async () => {
  const { service, db, calls, targets } = fixture();
  const launch = await service.executeDatabase(db, actor, {
    action: "openManaged",
    databaseID: "123",
  });
  assert.ok(calls[0].includes("FOR SHARE OF d,b"));
  assert.deepEqual(targets, [
    {
      permission: "org:databases:manage",
      projectID: "234",
      write: true,
      tenantAdmin: true,
      live: true,
    },
  ]);
  assert.equal((launch as { binding: { id: string } }).binding.id, "123");
  assert.equal(JSON.stringify(launch).includes("private-"), false);
});
test("pending revision, unhealthy backend and unavailable authorization prevent launch issuance", async () => {
  for (const patch of [
    { confirmed_credential_revision: 1 },
    { pending_backend_id: "789" },
    { endpoint_ready: false },
    { state: "suspended" },
  ]) {
    const { service, db, calls } = fixture(patch);
    await assert.rejects(
      service.executeDatabase(db, actor, {
        action: "openManaged",
        databaseID: "123",
      }),
    );
    assert.equal(calls.length, 1);
  }
  const { dependencies, db, calls } = fixture();
  dependencies.authorizeHosting = async () => {
    throw Error("authorization-denied");
  };
  await assert.rejects(
    createDatabaseService(dependencies).executeDatabase(db, actor, {
      action: "list",
      customerID: "345",
    }),
  );
  assert.equal(calls.length, 0);
});
test("non-Studio actors cannot launch or manage bindings", async () => {
  const { service, db, calls } = fixture();
  await assert.rejects(
    service.executeDatabase(
      db,
      { ...actor, source: "oauth" },
      { action: "openManaged", databaseID: "123" },
    ),
  );
  assert.equal(calls.length, 0);
});

test("stored provider sessions revalidate before reading managed credentials and reject a different subject", async () => {
  const f = fixture();
  const reference = {
    issuer: "https://fixture.clerk.accounts.dev",
    userID: "user_one",
    sessionID: "sess_one",
    organizationID: "org_one",
  };
  const session = {
    subject: actor.subject,
    session_id: null,
    clerk_identity: reference,
    database_id: "123",
    backend_id: "456",
    database_revision: 1,
    credential_revision: 2,
  };
  const db = {
    query: async (sql: string, args?: unknown[]) => {
      if (sql.includes("FROM webdock_auth.database_session"))
        return { rows: [] };
      if (sql.includes("FROM webdock_auth.managed_database_session"))
        return { rows: [session] };
      return f.db.query(sql, args);
    },
  } as DatabaseReader;
  let verified = 0;
  f.dependencies.withStoredIdentity = async (stored, run) => {
    assert.deepEqual(stored, reference);
    verified++;
    return run({ userID: "different-subject", sessionID: "sess_one" });
  };
  await assert.rejects(
    createDatabaseService(f.dependencies).inspectSession(db, "a".repeat(43)),
  );
  assert.equal(verified, 1);
  assert.deepEqual(f.targets, []);
  f.dependencies.withStoredIdentity = async () => {
    throw Error("provider-revoked");
  };
  await assert.rejects(
    createDatabaseService(f.dependencies).inspectSession(db, "a".repeat(43)),
    /provider-revoked/,
  );
  assert.deepEqual(f.targets, []);
});
