import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import { configureViewCache, ViewCache } from "../../src/cache";
import { database, authDatabase, closeDatabases } from "../../src/db/runtime";
import { executeHosting } from "../../src/modules/hosting/service";
import type { HostingActor } from "@webdock/hosting-contracts";
afterEach(restoreMocks);

const actor: HostingActor = {
  subject: "alice",
  sessionID: "session-a",
  source: "studio",
  scopes: ["hosting:read", "hosting:write"],
};
test.after(closeDatabases);

function fixture(t: test.TestContext) {
  const values = new Map<string, string>(),
    versions = new Map<string, number>(),
    expires = new Map<string, number>();
  const statements: string[] = [],
    domains: string[][] = [],
    ttls: number[] = [];
  const state = {
    session: true,
    operator: true,
    member: "admin",
    preview: false,
    appStatus: "ready",
    name: "First project",
    bindingReady: true,
    cacheFails: false,
    lastSeen: Date.now(),
    provider: "k3s" as string | null,
  };
  configureViewCache(
    new ViewCache({
      get: async (key) => {
        if (state.cacheFails) throw Error("cache offline");
        return (expires.get(key) ?? 0) > Date.now()
          ? (values.get(key) ?? null)
          : null;
      },
      set: async (key, value, ttlSeconds) => {
        ttls.push(ttlSeconds);
        values.set(key, value);
        expires.set(key, Date.now() + ttlSeconds * 1000);
      },
    }),
    { environment: "hosting-cache-test", defaultTTLSeconds: 60 },
  );
  t.after(() =>
    configureViewCache(undefined, { environment: "hosting-cache-test" }),
  );
  const rows = (items: Record<string, unknown>[] = []) => ({
    rows: items,
    rowCount: items.length,
  });
  // These fixtures model unlinked legacy accounts; provider-link reads use the auth pool.
  mockMethod(authDatabase, "query", async (sql: string) => {
    if (sql.includes("to_regclass('webdock_auth.clerk_identity')"))
      return rows([{ present: false }]);
    if (sql.includes("to_regclass('webdock_auth.clerk_organization')"))
      return rows([{ present: false }]);
    throw Error("Unexpected identity query: " + sql);
  });
  mockMethod(database, "connect", async () => ({
    release() {},
    async query(sql: string, params: unknown[] = []) {
      statements.push(sql);
      if (
        ["BEGIN", "COMMIT", "ROLLBACK"].includes(sql) ||
        sql.startsWith("SET LOCAL") ||
        sql.includes("lock_hosting_project")
      )
        return rows();
      if (sql.includes("FOR SHARE OF u,s"))
        return rows(
          state.session
            ? [
                {
                  id: params[0],
                  role: state.operator ? "operator" : "user",
                  banned: false,
                  emailVerified: true,
                  mustChangePassword: false,
                  twoFactorEnabled: state.operator,
                },
              ]
            : [],
        );
      if (sql.includes("FROM webdock_auth.studio_tenant_preview"))
        return rows(
          state.preview
            ? [
                {
                  customer_id: "11",
                  organization_id: "org",
                  expires_at: new Date(Date.now() + 60_000),
                },
              ]
            : [],
        );
      if (sql.includes("SELECT p.id,p.customer_id,p.status"))
        return rows([
          {
            id: params[0],
            customer_id: params[0] === "202" ? "22" : "11",
            status: "active",
            provider: state.provider,
            mode: "selfservice",
          },
        ]);
      if (sql.includes("SELECT c.status,t.organization_id"))
        return rows([{ status: "active", organization_id: "org" }]);
      if (
        sql.includes("SELECT organization_id FROM webdock_auth.tenant_customer")
      )
        return rows([{ organization_id: "org" }]);
      if (sql.includes("FROM webdock_auth.member WHERE"))
        return rows(state.member ? [{ id: "member", role: state.member }] : []);
      if (sql.includes("webdock_cache.version")) {
        const tables = params[0] as string[];
        domains.push(tables);
        return rows([
          {
            version: JSON.stringify(
              tables.map((table) => [table, versions.get(table) ?? 0]),
            ),
          },
        ]);
      }
      if (sql.startsWith('SELECT p.id AS "projectID"'))
        return rows([
          {
            projectID: params[0],
            name: state.name,
            customerID: "11",
            customerName: "Customer",
          },
        ]);
      if (sql.startsWith("SELECT name FROM webdock_admin.projects"))
        return rows([{ name: state.name }]);
      if (
        sql.startsWith(
          "SELECT id,name,lifecycle,status FROM webdock_auth.hosting_app",
        )
      )
        return rows([
          {
            id: "301",
            name: "App",
            lifecycle: "active",
            status: state.appStatus,
          },
        ]);
      if (
        sql.includes(
          "count(*)::int AS count FROM webdock_auth.hosting_database",
        )
      )
        return rows([{ count: 1 }]);
      if (sql.startsWith("SELECT * FROM webdock_auth.hosting_service_binding"))
        return rows([
          {
            id: "401",
            app_id: "301",
            project_id: "101",
            service: {
              kind: "mail",
              variable: "MAIL",
              connectionID: "501",
              domainID: "601",
            },
            state: "connected",
            secret_encrypted: "must-not-cache",
          },
        ]);
      if (sql.startsWith("SELECT 1 FROM webdock_auth.hosting_service_binding"))
        return rows(state.bindingReady ? [{}] : []);
      if (sql.includes("configuration->>'name'"))
        return rows([{ id: "701", name: "Setup", state: "draft" }]);
      if (sql.includes("FROM webdock_auth.git_source s JOIN"))
        return rows([
          { targetID: "301", repositoryName: "owner/repo", branch: "main" },
        ]);
      if (sql.includes("count(*)::int AS n")) return rows([{ n: 1 }]);
      if (sql.startsWith("SELECT a.id,a.name,a.status"))
        return rows([
          { id: "301", name: String(params[0]), status: state.appStatus },
        ]);
      if (sql.startsWith("SELECT o.id,o.action,o.status"))
        return rows([
          { id: "801", action: "apps.deploy", status: state.appStatus },
        ]);
      if (sql.startsWith("SELECT h.project_id"))
        return rows([
          {
            projectID: "101",
            name: state.name,
            provider: "k3s",
            mode: "selfservice",
          },
        ]);
      if (sql.startsWith("SELECT id FROM webdock_auth.hosting_cluster"))
        return rows([{ id: "901" }]);
      if (
        sql.startsWith(
          "SELECT id,name,provider,region FROM webdock_auth.hosting_cluster",
        )
      )
        return rows([
          { id: "901", name: "Cluster", provider: "k3s", region: "DE" },
        ]);
      if (sql.includes("FROM webdock_auth.hosting_cluster c LEFT JOIN"))
        return rows([
          {
            id: params[0],
            name: "Cluster",
            verified: true,
            revoked: false,
            lastSeen: new Date(state.lastSeen),
            observation: {},
          },
        ]);
      throw Error("Unexpected query: " + sql);
    },
  }));
  return {
    state,
    values,
    statements,
    domains,
    ttls,
    bump(table: string) {
      versions.set(table, (versions.get(table) ?? 0) + 1);
    },
    call: (command: unknown, caller = actor) => executeHosting(caller, command),
    count: (fragment: string) =>
      statements.filter((sql) => sql.includes(fragment)).length,
  };
}

test("operations cache skips data queries but checks operator authorization on every hit", async (t) => {
  const f = fixture(t),
    command = {
      action: "operations.overview",
      page: 1,
      limit: 10,
      search: "App",
      sort: "name",
    };
  const first = await f.call(command);
  assert.deepEqual(await f.call(command), first);
  assert.equal(f.count("SELECT a.id,a.name,a.status"), 1);
  assert.equal(f.count("FOR SHARE OF u,s"), 2);
  assert.equal(f.ttls[0], 15);
  assert.ok(f.domains[0].includes("webdock_auth.hosting_operation"));
  for (const changed of [
    { page: 2 },
    { limit: 20 },
    { search: "Other" },
    { sort: "-name" },
  ])
    await f.call({ ...command, ...changed });
  await f.call(command, { ...actor, subject: "bob", sessionID: "session-b" });
  assert.equal(
    f.count("SELECT a.id,a.name,a.status"),
    6,
    "Filters, pagination and callers cannot share entries",
  );
  f.state.appStatus = "failed";
  f.bump("webdock_auth.hosting_app");
  assert.equal(
    ((await f.call(command)) as { docs: { status: string }[] }).docs[0].status,
    "failed",
  );
  assert.equal(f.count("SELECT a.id,a.name,a.status"), 7);
  f.state.operator = false;
  await assert.rejects(f.call(command), /operator/);
  assert.equal(f.count("SELECT a.id,a.name,a.status"), 7);
  f.state.operator = true;
  f.state.session = false;
  await assert.rejects(f.call(command), /Sign in/);
});

test("project caches rederive permissions, reflect version changes, and fail fresh on cache outages", async (t) => {
  const f = fixture(t),
    command = { action: "projects.overview", projectID: "101" };
  const first = (await f.call(command)) as {
    project: { canManage: boolean; name: string };
    bindings: { state: string }[];
  };
  assert.equal(first.project.canManage, true);
  assert.deepEqual(await f.call(command), first);
  assert.equal(f.count("SELECT id,name,lifecycle,status"), 1);
  assert.equal(f.count("FOR SHARE OF u,s"), 2);
  assert.equal(f.ttls[0], 10);
  assert.ok(
    ![...f.values.values()].some((value) => value.includes("must-not-cache")),
  );
  assert.ok(
    ![...f.values.values()].some((value) => value.includes("canManage")),
    "Management permissions must not be stored in presentation data",
  );
  f.state.preview = true;
  assert.equal(
    ((await f.call(command)) as typeof first).project.canManage,
    false,
    "Preview cannot reuse cached management permission",
  );
  f.state.preview = false;
  f.state.operator = false;
  f.state.member = "admin";
  assert.equal(
    ((await f.call(command)) as typeof first).project.canManage,
    true,
  );
  const dataReads = f.count("SELECT id,name,lifecycle,status");
  f.state.member = "member";
  assert.equal(
    ((await f.call(command)) as typeof first).project.canManage,
    false,
    "Membership downgrade must apply on a cache hit",
  );
  assert.equal(f.count("SELECT id,name,lifecycle,status"), dataReads);
  f.state.bindingReady = false;
  f.bump("webdock_auth.mail_api_key");
  assert.equal(
    ((await f.call(command)) as typeof first).bindings[0].state,
    "blocked",
  );
  f.state.name = "Updated";
  f.bump("webdock_admin.projects");
  assert.equal(
    ((await f.call(command)) as typeof first).project.name,
    "Updated",
  );
  f.state.cacheFails = true;
  f.state.name = "Fresh despite outage";
  assert.equal(
    ((await f.call(command)) as typeof first).project.name,
    "Fresh despite outage",
  );
  f.state.member = "";
  await assert.rejects(f.call(command), /unavailable/);
});

for (const command of [
  { action: "projects.get", projectID: "101" },
  { action: "projects.list", customerID: "11" },
  { action: "clusters.list" },
  { action: "clusters.get", clusterID: "901" },
]) {
  test(`${command.action} caches presentation after fresh authorization`, async (t) => {
    const f = fixture(t);
    const first = await f.call(command);
    const initialQueries = f.statements.length;
    assert.deepEqual(await f.call(command), first);
    assert.equal(f.count("FOR SHARE OF u,s"), 2);
    assert.ok(f.values.size > 0, "The read must populate the shared cache");
    assert.ok(
      f.statements.length - initialQueries < initialQueries,
      "A hit must skip presentation queries",
    );
    f.state.session = false;
    await assert.rejects(f.call(command), /Sign in/);
  });
}

test("cluster observations invalidate immediately and time-derived health refreshes after the short TTL", async (t) => {
  const f = fixture(t),
    command = { action: "clusters.get", clusterID: "901" };
  f.state.lastSeen = Date.now() - 89_000;
  assert.equal(
    ((await f.call(command)) as { state: string }).state,
    "connected",
  );
  const now = Date.now();
  mockMethod(Date, "now", () => now + 6_000);
  assert.equal(((await f.call(command)) as { state: string }).state, "stale");
  f.state.lastSeen = Date.now();
  f.bump("webdock_auth.hosting_agent");
  assert.equal(
    ((await f.call(command)) as { state: string }).state,
    "connected",
  );
  assert.deepEqual(f.ttls, [5, 5, 5]);
});

test("project context keys isolate project and customer identifiers", async (t) => {
  const f = fixture(t);
  const first = (await f.call({
    action: "projects.get",
    projectID: "101",
  })) as { projectID: string };
  const second = (await f.call({
    action: "projects.get",
    projectID: "202",
  })) as { projectID: string };
  assert.equal(first.projectID, "101");
  assert.equal(second.projectID, "202");
  assert.equal(f.count('SELECT p.id AS "projectID"'), 2);
  await f.call({ action: "projects.get", projectID: "101" });
  assert.equal(f.count('SELECT p.id AS "projectID"'), 2);
});

test("project configuration fields come from fresh authorization even when display context is cached", async (t) => {
  const f = fixture(t),
    command = { action: "projects.get", projectID: "101" };
  const first = (await f.call(command)) as {
    configured: boolean;
    provider: string | null;
  };
  assert.equal(first.configured, true);
  f.state.provider = null;
  const next = (await f.call(command)) as typeof first;
  assert.equal(next.configured, false);
  assert.equal(next.provider, null);
  assert.equal(f.count('SELECT p.id AS "projectID"'), 1);
});
