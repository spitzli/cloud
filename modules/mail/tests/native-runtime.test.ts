import { test } from "node:test";
import assert from "node:assert/strict";
import type { Pool } from "pg";
import type { Connection } from "../src/server/native-ports.ts";
import { createNativeKeys } from "../src/server/native-keys.ts";
import { createNativeService } from "../src/server/native-service.ts";
import { createNativeAgent } from "../src/server/native-agent.ts";
import { createNativeSendTransport } from "../src/server/native-send-transport.ts";

const denied = async (): Promise<never> => {
  throw Error("authorization denied");
};
test("native runtimes keep separate installation configuration without a global locator", () => {
  const enabled = createNativeKeys({
    env: { SPITZLI_NATIVE_MAIL_SENDING_ENABLED: "true" },
    authorizeMail: denied,
  });
  const disabled = createNativeKeys({ env: {}, authorizeMail: denied });
  assert.doesNotThrow(() => enabled.requireNativeSending());
  assert.throws(
    () => disabled.requireNativeSending(),
    /Mail access is unavailable/,
  );
  assert.doesNotThrow(() => enabled.requireNativeSending());
});

test("native activation refuses first provisioning during draining but permits retained-instance resume", async () => {
  let retained = false;
  const intents: string[] = [];
  const database = {
    query: async () => ({
      rows: retained
        ? [
            {
              customer_id: "123",
              desired_enabled: false,
              state: "suspended",
              revision: "3",
            },
          ]
        : [],
    }),
    connect: async () => {
      throw Error("retained transaction reached");
    },
  } as unknown as Pool;
  const runtime = createNativeService({
    database,
    identity: { subject: denied },
    env: {
      SPITZLI_NATIVE_MAIL_ENABLED: "true",
      MAIL_MANAGED_CLUSTER_ID: "456",
    },
    requireMail: (intent) => {
      intents.push(intent ?? "access");
      if (intent === "provision") throw Error("module draining");
    },
    withAuthenticatedDatabase: async (_headers, run) => run(),
    getTenant: async () => ({ operator: true, tenant: { status: "active" } }),
    requireAccessOperator: async () => ({ user: { id: "789" } }),
    getPlatformSettings: async () => ({ mailEnabled: true, mailRegion: "eu" }),
  });
  await assert.rejects(
    runtime.manageTenantMailService(new Headers(), "123", {
      action: "activate",
      revision: "0",
    }),
    /module draining/,
  );
  assert.deepEqual(intents, ["access", "provision"]);
  retained = true;
  intents.length = 0;
  await assert.rejects(
    runtime.manageTenantMailService(new Headers(), "123", {
      action: "activate",
      revision: "3",
    }),
    /retained transaction reached/,
  );
  assert.deepEqual(intents, ["access"]);
});

test("native agent proof is checked before consulting protected service rows", async () => {
  let queries = 0;
  const db = {
    query: async () => {
      queries++;
      return { rows: [] };
    },
  } as unknown as Connection;
  const runtime = createNativeAgent({
    env: {},
    database: {} as Pool,
    transaction: async (run) => run(db),
    verifyAgent: denied,
  });
  await assert.rejects(
    runtime.verifyMailAgent(db, {
      clusterID: "123",
      generation: 1,
      credentialHash: "invalid",
    }),
    /authorization denied/,
  );
  assert.equal(queries, 0);
});

test("relay factories cannot issue provider credentials before native authorization", async () => {
  let providers = 0,
    queries = 0;
  const db = {
    query: async () => {
      queries++;
      return { rows: [] };
    },
  } as unknown as Connection;
  const runtime = createNativeSendTransport({
    env: {},
    transaction: async (run) => run(db),
    authorizeMail: denied,
    requireNativeSending: () => {},
    getPlatformSettings: async () => ({ mailEnabled: true, mailRegion: "eu" }),
    decryptMailSecret: () => {
      throw Error("unexpected decryption");
    },
    byokPolicy: async () => ({ turbosmtp: true }),
    credentialMaster: async () => {
      providers++;
      throw Error("unexpected provider");
    },
    credentialIssuer: () => {
      providers++;
      throw Error("unexpected provider");
    },
  });
  await assert.rejects(
    runtime.configureNativeTransport(
      { subject: "123", sessionID: "session" },
      "456",
      "byok",
    ),
    /authorization denied/,
  );
  assert.equal(providers, 0);
  assert.equal(queries, 0);
});

test("native activation and reconciliation recheck selected operator authority inside the transaction", async () => {
  let role = "operator", banned = false, reconcile = false, writes = 0;
  const local = { subject: "789", role: "user", banned: false, emailVerified: false, twoFactorEnabled: false, mustChangePassword: true };
  const client = {
    query: async (sql: string, values?: unknown[]) => {
      if (sql.includes('FROM webdock_auth."user"')) return { rows: sql.includes("role='operator'") ? [] : [local], rowCount: sql.includes("role='operator'") ? 0 : 1 };
      if (sql.includes("SELECT c.status")) return { rows: [{ status: "active" }] };
      if (sql.includes("SELECT settings")) return { rows: [{ settings: { mailEnabled: true } }] };
      if (sql.includes("SELECT * FROM webdock_mail.service")) return { rows: reconcile ? [{ desired_enabled: true, state: "needs_review", revision: "1" }] : [] };
      if (sql.includes("verified_at")) return { rows: [{}], rowCount: 1 };
      if (sql.includes("to_regclass")) return { rows: [{ present: false }] };
      if (sql.startsWith("INSERT INTO webdock_mail.service")) { writes++; return { rows: [{ customer_id: "123", desired_enabled: true, state: "pending", revision: values?.[4], host_id: "primary" }] }; }
      return { rows: [], rowCount: 0 };
    },
    release() {},
  };
  const database = { connect: async () => client, query: async () => ({ rows: reconcile ? [{ customer_id: "123", desired_enabled: true, state: "needs_review", revision: "1" }] : [] }) } as unknown as Pool;
  const runtime = createNativeService({
    database,
    identity: { subject: async (db, subject, options) => {
      assert.equal(db, client); assert.equal(subject, "789"); assert.equal(options?.lock, true);
      await db.query('SELECT id FROM webdock_auth."user" WHERE id=$1 FOR SHARE', [subject]);
      return { ...local, role, banned, emailVerified: true, mustChangePassword: false, operatorAuthorized: role === "operator" };
    } },
    env: { SPITZLI_NATIVE_MAIL_ENABLED: "true", AUTH_TEST_MAIL: "true" },
    requireMail() {}, withAuthenticatedDatabase: async (_headers, run) => run(),
    getTenant: async () => ({ operator: true, tenant: { status: "active" } }),
    requireAccessOperator: async () => ({ user: { id: "789" } }),
    getPlatformSettings: async () => ({ mailEnabled: true, mailRegion: "eu" }),
  });
  await runtime.manageTenantMailService(new Headers(), "123", { action: "activate", revision: "0" });
  reconcile = true;
  await runtime.manageTenantMailService(new Headers(), "123", { action: "reconcile", revision: "1" });
  assert.equal(writes, 2);
  role = "user";
  await assert.rejects(runtime.manageTenantMailService(new Headers(), "123", { action: "reconcile", revision: "1" }), /operator/);
  role = "operator"; banned = true;
  await assert.rejects(runtime.manageTenantMailService(new Headers(), "123", { action: "reconcile", revision: "1" }), /operator/);
  assert.equal(writes, 2);
});
