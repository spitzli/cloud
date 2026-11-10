import test from "node:test";
import assert from "node:assert/strict";
import type { DatabaseReader } from "@spitzli/core/identity";
import type { HostingActor } from "@spitzli/hosting-contracts";
import { createHostingSetupSource } from "../src/server/setup-source.ts";
const actor: HostingActor = {
  subject: "123",
  sessionID: "456",
  source: "studio",
  scopes: ["hosting:read"],
};
function fixture(available: readonly string[]) {
  const sql: string[] = [];
  let databaseCalls = 0,
    gitCalls = 0;
  const database = {
    query: async (query: string) => {
      sql.push(query);
      let rows: unknown[] = [];
      if (query.includes("SELECT p.name"))
        rows = [{ name: "Project", customerName: "Customer" }];
      else if (query.includes("FROM webdock_auth.mail_connection"))
        rows = [{ id: "159", domainID: "357" }];
      return { rows, rowCount: rows.length };
    },
  } as unknown as DatabaseReader;
  const never = async () => {
    throw Error("Unexpected provider call");
  };
  const runtime = createHostingSetupSource({
    setupProductAvailable: (product) => available.includes(product),
    setupBuildCapabilities: async () => ({
      isolated: true,
      githubActions: false,
    }),
    readSetup: never,
    createGitHubProvider: () => {
      throw Error("Unexpected provider call");
    },
    transaction: (run) => run(database),
    authorizeHosting: async () => ({
      operator: false,
      liveReads: true,
      customerID: "789",
      project: { provider: "k3s", mode: "selfservice" },
      canWrite: true,
    }),
    executeGitDeployment: async () => {
      gitCalls++;
      return { docs: [] };
    },
    executeDatabaseCommand: async () => {
      databaseCalls++;
      throw Error("Database options selected");
    },
    env: { SPITZLI_SETUP_ENABLED: "true" },
  });
  return {
    runtime,
    sql,
    get databaseCalls() {
      return databaseCalls;
    },
    get gitCalls() {
      return gitCalls;
    },
  };
}
test("Hosting-only setup options never read disabled Mail rows or invoke disabled Database services", async () => {
  const f = fixture([]);
  const options = await f.runtime.executeSetupSource(actor, {
    action: "setup.options",
    projectID: "246",
  });
  assert.deepEqual(options.mail, []);
  assert.deepEqual(options.databases, []);
  assert.deepEqual(options.plans, []);
  assert.equal(options.enabled, true);
  assert.equal(f.gitCalls, 1);
  assert.equal(f.databaseCalls, 0);
  assert(
    !f.sql.some(
      (query) =>
        query.includes("mail_connection") ||
        query.includes("mail_sender_domain"),
    ),
  );
});
test("setup option availability is independent per product and does not swallow enabled-service failures", async () => {
  const mail = fixture(["mail"]);
  const options = await mail.runtime.executeSetupSource(actor, {
    action: "setup.options",
    projectID: "246",
  });
  assert.equal(options.mail.length, 1);
  assert.equal(mail.databaseCalls, 0);
  const database = fixture(["databases"]);
  await assert.rejects(
    database.runtime.executeSetupSource(actor, {
      action: "setup.options",
      projectID: "246",
    }),
    /Database options selected/,
  );
  assert(database.databaseCalls > 0);
  assert(!database.sql.some((query) => query.includes("mail_connection")));
});
