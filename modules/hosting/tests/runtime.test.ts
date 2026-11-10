import test from "node:test";
import assert from "node:assert/strict";
import type {
  DatabaseReader,
  IdentityDirectory,
  LocalIdentity,
  SessionIdentity,
} from "@spitzli/core/identity";
import type { HostingActor } from "@spitzli/hosting-contracts";
import { createHostingAuthorization } from "../src/server/authorization.ts";
import { createHostingBridge } from "../src/server/bridge.ts";
const actor: HostingActor = {
  subject: "123",
  sessionID: "456",
  source: "studio",
  scopes: ["hosting:read", "hosting:write"],
};
function fixture() {
  let user: SessionIdentity | null = {
    subject: "123",
    sessionID: "456",
    role: "user",
    banned: false,
    emailVerified: true,
    mustChangePassword: false,
    twoFactorEnabled: false,
    preview: false,
  };
  let member = true,
    preview = false,
    policy = true;
  const locks: string[] = [];
  const identity: IdentityDirectory = {
    session: async (_db, _ref, options) => {
      assert.equal(options?.lock, true);
      locks.push("session");
      return user;
    },
    subject: async (_db, _id, options) => {
      assert.equal(options?.lock, true);
      locks.push("subject");
      return user;
    },
    membership: async (_db, _id, customer, options) => {
      assert.equal(customer, "789");
      assert.equal(options?.lock, true);
      locks.push("membership");
      return { memberID: null, role: member ? "admin" : null };
    },
  };
  const database = {
    query: async (sql: string) => {
      assert.doesNotMatch(
        sql,
        /(?:FROM|JOIN)\s+webdock_auth\.(?:"user"|session|member)\b/,
      );
      let rows: unknown[] = [];
      if (sql.includes("studio_tenant_preview") && preview)
        rows = [
          {
            customer_id: "789",
            organization_id: "org",
            expires_at: new Date(Date.now() + 60000),
            expired_at: null,
          },
        ];
      else if (sql.startsWith("SELECT p.id"))
        rows = [
          {
            id: "246",
            customer_id: "789",
            status: "active",
            mode: "selfservice",
            provider: "kubernetes",
            cluster_id: "135",
            revision: 1,
            own_images: false,
          },
        ];
      else if (sql.includes("SELECT c.status"))
        rows = [{ status: "active", organization_id: "org" }];
      else if (sql.includes("FROM webdock_auth.git_source") && policy)
        rows = [{ id: "159" }];
      return { rows, rowCount: rows.length };
    },
  } as unknown as DatabaseReader;
  const runtime = createHostingAuthorization({
    identity,
    currentConsolePrincipal: () => undefined,
    organizationPermission: async () => undefined,
    currentDatabaseScope: () => undefined,
  });
  return {
    runtime,
    database,
    locks,
    setUser: (value: SessionIdentity | null) => {
      user = value;
    },
    setMember: (value: boolean) => {
      member = value;
    },
    setPreview: (value: boolean) => {
      preview = value;
    },
    setPolicy: (value: boolean) => {
      policy = value;
    },
  };
}
test("Hosting uses local identity ports with locks and rechecks membership revocation", async () => {
  const f = fixture();
  const authorized = await f.runtime.authorizeHosting(
    actor,
    { projectID: "246", write: true, permission: "org:projects:manage" },
    f.database,
  );
  assert.equal(authorized.customerID, "789");
  assert.equal(authorized.canWrite, true);
  assert.deepEqual(f.locks, ["session", "membership"]);
  f.setMember(false);
  await assert.rejects(
    f.runtime.authorizeHosting(
      actor,
      { projectID: "246", write: true, permission: "org:projects:manage" },
      f.database,
    ),
  );
  f.setUser(null);
  await assert.rejects(
    f.runtime.authorizeHosting(actor, { projectID: "246" }, f.database),
  );
});
test("Hosting preview never grants writes or live reads, including to operators", async () => {
  const f = fixture();
  f.setPreview(true);
  f.setUser({
    subject: "123",
    sessionID: "456",
    role: "operator",
    banned: false,
    emailVerified: true,
    mustChangePassword: false,
    twoFactorEnabled: true,
    preview: true,
  });
  for (const target of [{ write: true }, { live: true }, { operator: true }])
    await assert.rejects(
      f.runtime.authorizeHosting(
        actor,
        { customerID: "789", ...target },
        f.database,
      ),
    );
  const read = await f.runtime.authorizeHosting(
    actor,
    { customerID: "789" },
    f.database,
  );
  assert.equal(read.operator, false);
  assert.equal(read.liveReads, false);
  assert.equal(read.canWrite, false);
});
test("automatic Git policy remains bound to the exact durable source and never becomes live/operator access", async () => {
  const f = fixture();
  const policyActor: HostingActor = {
    ...actor,
    source: "git-policy",
    sessionID: "",
    gitSourceID: "159",
    gitSourceRevision: 1,
  };
  await f.runtime.authorizeHosting(
    policyActor,
    { projectID: "246", write: true, permission: "org:projects:manage" },
    f.database,
  );
  assert.equal(f.locks[0], "subject");
  f.setPolicy(false);
  await assert.rejects(
    f.runtime.authorizeHosting(
      policyActor,
      { projectID: "246", write: true, permission: "org:projects:manage" },
      f.database,
    ),
  );
  await assert.rejects(
    f.runtime.authorizeHosting(
      policyActor,
      { projectID: "246", operator: true },
      f.database,
    ),
  );
});
test("shared Hosting bridge keeps module admission ahead of provider bindings and preserves deferred composition", async () => {
  const trace: string[] = [];
  let blocked = true;
  const unavailable = async () => {
    throw Error("Unexpected external call");
  };
  const runtime = createHostingBridge({
    requireHostingCommand: (action) => {
      trace.push(action);
      if (blocked) throw Error("disabled");
    },
    vercelSettings: () => {
      trace.push("settings");
      return null;
    },
    displayDatabaseCredentials: unavailable,
    executeGitOAuth: unavailable,
    auth: { handler: unavailable },
    database: fixture().database,
    mcpResource: "https://studio.example.test/api/mcp",
    currentHostingMCPClaims: async () => ({ disabled: true }),
    get executeHosting() {
      throw Error("Uninitialized peer must not be read at import time");
    },
    issueEnrollment: async () => {
      trace.push("enrollment");
      throw Error("stop after selected handler");
    },
    readHostingJSON: unavailable,
    requireHostingEnvironment: () => {},
    env: {},
  });
  assert.deepEqual(trace, []);
  await assert.rejects(
    runtime.executeHostingBridgeCommand(actor, {
      action: "enrollment.display",
      enrollmentID: "135",
    }),
    /disabled/,
  );
  assert.deepEqual(trace, ["enrollment.display"]);
  blocked = false;
  await assert.rejects(
    runtime.executeHostingBridgeCommand(actor, {
      action: "enrollment.display",
      enrollmentID: "135",
    }),
    /selected handler/,
  );
  assert.deepEqual(trace, [
    "enrollment.display",
    "enrollment.display",
    "settings",
    "enrollment",
  ]);
});

import { assertSetupProducts } from "../src/server/setup-admission.ts";
import { createRegistry } from "@spitzli/module-sdk";
import { hostingManifest } from "../src/manifest.ts";
import { mailManifest } from "@spitzli/module-mail/manifest";
import { databasesManifest } from "@spitzli/module-databases/manifest";
import type { SetupConfiguration } from "@spitzli/hosting-contracts";
test("setup admission cannot hide disabled Database or Mail products inside Hosting commands", () => {
  const database = {
    mode: "new",
    variable: "DATABASE_URL",
    name: "Setup database",
    planRevisionID: "123",
  } as SetupConfiguration["database"];
  const mail = { connectionID: "123", domainID: "456", variable: "MAIL_KEY" };
  for (const disabled of ["databases", "mail"]) {
    const registry = createRegistry(
      [hostingManifest, mailManifest, databasesManifest],
      {
        configVersion: 1,
        modules: {
          hosting: { enabled: true },
          databases: { enabled: disabled !== "databases" },
          mail: { enabled: disabled !== "mail" },
        },
      },
    );
    const ports = {
      requireDatabases: (intent: "access" | "provision" = "access") =>
        registry.require("databases", intent),
      requireMail: (intent: "access" | "provision" = "access") =>
        registry.require("mail", intent),
    };
    assert.throws(() => assertSetupProducts(ports, { database, mail }));
    assert.throws(() =>
      assertSetupProducts(ports, { database, mail }, { confirmed: true }),
    );
  }
});
test("draining refuses new Database setup but preserves confirmed setup recovery and existing-resource access", () => {
  const calls: string[] = [];
  const ports = {
    requireDatabases: (intent: "access" | "provision" = "access") => {
      calls.push(intent);
      if (intent === "provision") throw Error("draining");
    },
    requireMail: (intent: "access" | "provision" = "access") => {
      assert.equal(intent, "access");
    },
  };
  const database = {
    mode: "new",
    variable: "DATABASE_URL",
    name: "Setup database",
    planRevisionID: "123",
  } as SetupConfiguration["database"];
  const config = {
    database,
    mail: { connectionID: "123", domainID: "456", variable: "MAIL_KEY" },
  };
  assert.throws(() => assertSetupProducts(ports, config), /draining/);
  assert.doesNotThrow(() =>
    assertSetupProducts(ports, config, { confirmed: true }),
  );
  assert.doesNotThrow(() =>
    assertSetupProducts(ports, config, { databaseID: "789" }),
  );
  assert.deepEqual(calls, ["provision", "access", "access"]);
});
