import test from "node:test";
import assert from "node:assert/strict";
import type { DatabaseReader } from "@spitzli/core/identity";
import {
  setupConfigurationSchema,
  type HostingActor,
} from "@spitzli/hosting-contracts";
import {
  createHostingProjectSetup,
  type SetupRow,
} from "../src/server/project-setup.ts";
const actor: HostingActor = {
  subject: "123",
  sessionID: "456",
  source: "studio",
  scopes: ["hosting:read", "hosting:write"],
};
const base = {
  name: "web",
  source: { kind: "git", branch: "main", rootDirectory: "." },
  spec: {
    args: [],
    port: 3000,
    healthPath: "/",
    cpuMillicores: 100,
    memoryBytes: 67108864,
    ephemeralBytes: 67108864,
    replicas: 1,
  },
  autoPublish: false,
};
function fixture(product: "database" | "mail", draining = false) {
  const configuration = setupConfigurationSchema.parse({
    ...base,
    database:
      product === "database"
        ? {
            mode: "new",
            name: "db",
            planRevisionID: "135",
            variable: "DATABASE_URL",
          }
        : { mode: "none" },
    mail:
      product === "mail"
        ? { connectionID: "147", domainID: "258", variable: "MAIL_KEY" }
        : null,
  });
  const row: SetupRow = {
    id: "369",
    customer_id: "789",
    project_id: "246",
    subject: actor.subject,
    actor,
    configuration,
    environment_encrypted: null,
    revision: 1,
    state: "blocked",
    preview: {
      id: "c05c2467-8f5f-4779-9102-9a94c190b4dc",
      revision: 1,
      expiresAt: new Date(Date.now() + 60000).toISOString(),
      sourceSHA: null,
      image: null,
      subscriptionRevision: 1,
      blockers: [],
      offers: [],
    },
    confirmed_preview_id: draining
      ? "c05c2467-8f5f-4779-9102-9a94c190b4dc"
      : null,
    resources: {},
    steps: [],
    reason: null,
    generation: 1,
    lease_until: null,
  };
  const writes: string[] = [],
    admissions: string[] = [];
  const db = {
    query: async (sql: string) => {
      if (/^(INSERT|UPDATE)/.test(sql)) writes.push(sql);
      return {
        rows: sql.includes("RETURNING")
          ? [{ ...row, state: "running" }]
          : sql.startsWith("SELECT * FROM webdock_auth.hosting_setup")
            ? [row]
            : [],
        rowCount: 1,
      };
    },
  } as unknown as DatabaseReader;
  const never = async () => {
    throw Error("Unexpected provider or provisioning call");
  };
  const runtime = createHostingProjectSetup({
    authorizeSetupConfiguration: async () => {},
    requireDatabases: (intent) => {
      admissions.push("database:" + intent);
      if (product === "database" && (!draining || intent === "provision"))
        throw Error("database unavailable");
    },
    requireMail: (intent) => {
      admissions.push("mail:" + intent);
      if (product === "mail") throw Error("mail unavailable");
    },
    executeSetupSource: never,
    repairSetupEnvironment: never,
    resetGitSetupRetry: () => false,
    executeAppCommand: never,
    transaction: (run) => run(db),
    audit: async () => {},
    authorizeHosting: async () => ({
      operator: false,
      liveReads: true,
      customerID: "789",
      project: undefined,
      canWrite: true,
    }),
    replay: async () => ({ existing: null, hash: "hash" }),
    recordResult: async () => {},
    openEnvironment: () => ({}),
    sealEnvironment: () => "sealed",
    patchEnvironment: () => ({}),
    previewProjectSetup: never,
    assertSetupConfirmation: () => {},
    env: {},
  });
  return { runtime, row, configuration, writes, admissions };
}
test("crafted save, preview and confirm commands cannot admit disabled dependent products", async () => {
  for (const product of ["database", "mail"] as const)
    for (const action of ["setup.save", "setup.preview", "setup.confirm"]) {
      const f = fixture(product);
      const command =
        action === "setup.save"
          ? {
              action,
              projectID: "246",
              revision: 0,
              configuration: f.configuration,
              idempotencyKey: "setup-test-admission",
            }
          : action === "setup.preview"
            ? { action, projectID: "246", setupID: f.row.id, revision: 1 }
            : {
                action,
                projectID: "246",
                setupID: f.row.id,
                revision: 1,
                previewID: f.row.preview!.id,
                acceptedOfferIDs: [],
                idempotencyKey: "setup-test-admission",
              };
      await assert.rejects(
        f.runtime.executeProjectSetup(actor, command),
        new RegExp(product + " unavailable"),
      );
      assert.deepEqual(f.writes, []);
    }
});
test("a confirmed interrupted setup can queue its existing work while Database drains", async () => {
  const f = fixture("database", true);
  const result = await f.runtime.executeProjectSetup(actor, {
    action: "setup.resume",
    projectID: "246",
    setupID: f.row.id,
    revision: 1,
    idempotencyKey: "setup-test-resume",
  });
  assert.equal(result.state, "running");
  assert.deepEqual(f.admissions, ["database:access"]);
  assert(f.writes.some((sql) => sql.includes("state='running'")));
});
