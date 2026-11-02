import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import { auth } from "../../src/modules/auth/auth";
import { database, closeDatabases } from "../../src/db/runtime";
import { configureViewCache, ViewCache } from "../../src/cache";
import { getPlans } from "../../src/modules/plans/plans";
afterEach(restoreMocks);

test.after(async () => { await auth.$context; await closeDatabases(); });
test("plan catalog caches only after live operator checks and reloads each revision", async t => {
  await auth.$context;
  const store = new Map<string, string>();
  let revision = "1", name = "Starter", loads = 0, hits = 0, signedIn = true;
  const user = { id: "123", role: "operator", banned: false, emailVerified: true, mustChangePassword: false, twoFactorEnabled: true };
  mockMethod(auth.api, "getSession", async () => signedIn ? { user, session: { id: "session-plan-cache" } } : null);
  mockMethod(database, "query", async (sql: string, values?: unknown[]) => {
    if (sql.includes("to_regclass('webdock_auth.clerk_identity')")) return {rows:[{present:false}]};
    if (sql.includes("webdock_cache.version")) {
      assert.deepEqual(values, [["webdock_auth.plan_template"]]);
      return { rows: [{ version: revision }] };
    }
    if (sql.includes('FROM webdock_auth."user"')) return { rows: [user] };
    if (sql.includes("FROM webdock_auth.plan_template")) {
      loads++;
      return { rows: [{ id: "45", name, description: "Public plan", allowances: {}, createdAt: new Date("2026-10-15T00:00:00Z") }] };
    }
    throw Error("Unexpected plan catalog query");
  });
  configureViewCache(new ViewCache({
    get: async key => { hits++; return store.get(key) ?? null; },
    set: async (key, value, ttl) => { assert.equal(ttl, 60); store.set(key, value); },
  }), { environment: "plan-cache-test" });
  t.after(() => configureViewCache(undefined, { environment: "test" }));
  const headers = new Headers();
  const first = await getPlans(headers);
  assert.deepEqual(await getPlans(headers), first);
  assert.equal(loads, 1, "warm authorized catalog skips the plan query");
  assert.equal(first[0].createdAt, "2026-10-15T00:00:00.000Z");
  name = "Updated"; revision = "2";
  assert.equal((await getPlans(headers))[0].name, "Updated");
  assert.equal(loads, 2);
  const beforeDenied = hits;
  user.twoFactorEnabled = false;
  await assert.rejects(getPlans(headers), /security setup/);
  user.twoFactorEnabled = true; user.role = "user";
  await assert.rejects(getPlans(headers), /operator access/);
  user.role = "operator"; signedIn = false;
  await assert.rejects(getPlans(headers), /Sign in/);
  assert.equal(hits, beforeDenied, "unauthorized requests never reach the cache");
});
