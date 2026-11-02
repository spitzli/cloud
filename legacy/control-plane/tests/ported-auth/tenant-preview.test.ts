import { configureFullProducts } from "../../src/full-products";
configureFullProducts();
import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../../src/modules/auth/bootstrap";
import { handleStudioRequest } from "../../src/modules/portal/studio-api";
import { tenantPreviewSchemaSQL } from "../../src/modules/access/tenant-preview-schema";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { accessSchemaSQL } from "../../src/modules/access/access-management";
import { plansSchemaSQL } from "../../src/modules/plans/plans";
import { storageUsageSchemaSQL } from "../../src/modules/platform/storage-usage";
import { mailSchemaSQL } from "../../src/modules/mail/tenant-mail";
import { platformSchemaSQL } from "../../src/modules/platform/platform";
afterEach(restoreMocks);
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable test database/outbox required");
const origin = process.env.BETTER_AUTH_URL!;
test.after(async () => {
  await auth.$context;
  await database.end();
});
async function identity(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `preview-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Preview fixture",
    password,
    operator,
    mustChangePassword: false,
  });
  const response = await auth.api.signInEmail({
    body: { email: user.email, password },
    asResponse: true,
  });
  assert.equal(response.status, 200);
  const headers = new Headers({
    Origin: origin,
    Cookie: response.headers
      .getSetCookie()
      .map((c) => c.split(";")[0])
      .join("; "),
  });
  const session = await auth.api.getSession({ headers });
  assert.ok(session);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  return { user, headers, sid: session.session.id };
}
test("Session-bound tenant preview is scoped, read-only, auditable and restrictive until explicit exit", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(tenantPreviewSchemaSQL);
  await database.query(plansSchemaSQL);
  await database.query(storageUsageSchemaSQL);
  await database.query(mailSchemaSQL);
  await database.query(platformSchemaSQL);
  const operator = await identity(true),
    ordinary = await identity();
  const otherSession = await (
    await auth.$context
  ).internalAdapter.createSession(operator.user.id);
  const studio = await registerApplication({
    label: "Preview Studio",
    origin: "http://127.0.0.1:3120",
    logoutPath: "/login",
    headers: operator.headers,
  });
  process.env.WEBDOCK_STUDIO_CLIENT_ID = studio.clientID;
  const customers = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name,notes) VALUES(webdock_auth.next_snowflake(),'Preview One','private operator note'),(webdock_auth.next_snowflake(),'Preview Two','private other note') RETURNING id,name",
    )
  ).rows;
  const [one, two] = customers;
  const mappings = (
    await database.query(
      "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [customers.map((c) => c.id)],
    )
  ).rows;
  const org = (id: string) =>
    mappings.find((m) => m.customer_id === id).organization_id;
  const own = await registerApplication({
    label: "Own preview site",
    origin: "http://127.0.0.1:3221",
    logoutPath: "/admin/login",
    organizationID: org(one.id),
    headers: operator.headers,
  });
  const foreign = await registerApplication({
    label: "Foreign preview site",
    origin: "http://127.0.0.1:3222",
    logoutPath: "/admin/login",
    organizationID: org(two.id),
    headers: operator.headers,
  });
  const call = async (
    operation: string,
    args: unknown[] = [],
    actor = operator,
  ) => {
    const request = new Request(origin + "/api/studio", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization:
          "Basic " +
          Buffer.from(`${studio.clientID}:fixture-secret`).toString("base64"),
      },
      body: JSON.stringify({ operation, args, accessToken: "fixture-token" }),
    });
    const response = await handleStudioRequest(request, {
      introspect: async () =>
        Response.json({
          active: true,
          client_id: studio.clientID,
          sub: actor.user.id,
          sid: actor.sid,
          exp: Math.floor(Date.now() / 1000) + 300,
        }),
    });
    return { status: response.status, ...(await response.json()) };
  };
  assert.equal(
    (await call("startTenantPreview", [one.id], ordinary)).status,
    403,
  );
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
    [operator.user.id],
  );
  assert.equal((await call("startTenantPreview", [one.id])).status, 401);
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
    [operator.user.id],
  );
  const counts = async () =>
    (
      await database.query(
        'SELECT (SELECT count(*) FROM webdock_auth."user") AS users,(SELECT count(*) FROM webdock_auth.member) AS members',
      )
    ).rows[0];
  const before = await counts();
  const started = await call("startTenantPreview", [one.id]);
  assert.equal(started.status, 200);
  assert.equal(started.data.customerID, one.id);
  assert.equal(started.data.role, "admin");
  assert.equal(started.data.readOnly, true);
  assert.ok(
    Date.parse(started.data.expiresAt) > Date.now() + 14 * 60_000 &&
      Date.parse(started.data.expiresAt) <= Date.now() + 15 * 60_000,
  );
  assert.deepEqual(
    await counts(),
    before,
    "Preview must not create users or memberships",
  );
  const session = (await call("session")).data;
  assert.equal(session.user.id, operator.user.id);
  assert.equal(session.user.role, "user");
  assert.equal(session.operator, false);
  assert.equal(session.preview.status, "active");
  assert.equal(session.preview.customerName, one.name);
  assert.equal(
    (await call("session", [], { ...operator, sid: otherSession.id })).data
      .operator,
    true,
    "Other browser sessions remain operators",
  );
  assert.deepEqual(
    (await call("listTenants")).data.map((t: { id: string }) => t.id),
    [one.id],
  );
  assert.deepEqual((await call("listTenantInvitations")).data, []);
  assert.deepEqual(
    (await call("accountSites")).data.map(
      (s: { id: string; role: string }) => ({ id: s.id, role: s.role }),
    ),
    [{ id: own.binding, role: "admin" }],
  );
  const tenant = (await call("getTenant", [one.id])).data;
  assert.equal(tenant.operator, false);
  assert.equal(tenant.canManage, false);
  assert.equal(tenant.tenant.notes, undefined);
  assert.deepEqual(tenant.members, []);
  assert.equal(tenant.sites[0].role, "admin");
  for (const op of [
    "getTenant",
    "getTenantPlan",
    "getTenantStorage",
    "getTenantMail",
    "getTenantMailService",
  ]) {
    assert.equal((await call(op, [one.id])).status, 200);
    assert.equal((await call(op, [two.id])).status, 403);
  }
  await database.query(
    `INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready','{"active":true,"limit":1000,"sent":42}')`,
    [
      one.id,
      "preview-provider-" + one.id,
      "preview-" + one.id + "@example.invalid",
    ],
  );
  const mail = (await call("getTenantMail", [one.id])).data;
  assert.equal(mail.tenant.id, one.id);
  assert.equal(mail.operator, false);
  assert.equal(mail.canManage, false);
  assert.equal(mail.account.sent, 42);
  assert.equal(mail.account.limit, 1000);
  assert.equal(typeof mail.enabled, "boolean");
  assert.equal(typeof mail.configured, "boolean");
  for (const [op, args] of [
    ["manageTenant", [one.id, { action: "profile", name: "Changed" }]],
    ["manageAccess", [{ action: "create-organization", name: "Bad" }]],
    ["managePlans", [{}]],
    ["manageTenantMail", [one.id, {}]],
    [
      "manageTenantMailService",
      [one.id, { action: "activate", revision: "0" }],
    ],
    ["manageTenantStorage", [one.id, {}]],
    ["getTenantMailKeys", [one.id]],
    ["getOffer", ["opaque"]],
    ["acceptOffer", ["opaque"]],
    ["requireAccessOperator", []],
    ["listAccess", [""]],
    ["getPlatformSettings", []],
    ["startTenantPreview", [two.id]],
  ] as [string, unknown[]][]) {
    assert.equal((await call(op, args)).status, 403, op);
  }
  const privateNote = "must-not-enter-preview-audit";
  assert.equal(
    (
      await call("manageTenant", [
        one.id,
        { action: "profile", notes: privateNote },
      ])
    ).status,
    403,
  );
  const deniedEvents = (
    await database.query(
      "SELECT actor_id,action,target_id,outcome FROM webdock_auth.access_event WHERE actor_id=$1 AND action LIKE 'tenant-preview-denied:%'",
      [operator.user.id],
    )
  ).rows;
  assert.ok(
    deniedEvents.some(
      (e) =>
        e.action === "tenant-preview-denied:manageTenant" &&
        e.target_id === one.id &&
        e.outcome === "denied",
    ),
  );
  assert.ok(
    deniedEvents.every(
      (e) => e.actor_id === operator.user.id && e.target_id === one.id,
    ),
  );
  assert.equal(JSON.stringify(deniedEvents).includes(privateNote), false);
  await database.query(
    "UPDATE webdock_auth.app_binding SET enabled=false WHERE id=$1",
    [own.binding],
  );
  assert.deepEqual((await call("accountSites")).data, []);
  await database.query(
    "UPDATE webdock_auth.app_binding SET enabled=true WHERE id=$1",
    [own.binding],
  );
  await database.query(
    'UPDATE webdock_auth."oauthClient" SET disabled=true WHERE "clientId"=$1',
    [own.clientID],
  );
  assert.deepEqual((await call("accountSites")).data, []);
  await database.query(
    'UPDATE webdock_auth."oauthClient" SET disabled=false WHERE "clientId"=$1',
    [own.clientID],
  );
  const settings = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  try {
    await database.query(
      `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"cmsEnabled":false}') ON CONFLICT(id) DO UPDATE SET settings=webdock_auth.platform_settings.settings || '{"cmsEnabled":false}'::jsonb`,
    );
    assert.deepEqual(
      (await call("accountSites")).data,
      [],
      "CMS switch applies to the customer view",
    );
  } finally {
    if (settings)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [settings.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
  await database.query(
    "UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 second' WHERE session_id=$1",
    [operator.sid],
  );
  assert.equal((await call("session")).data.preview.status, "expired");
  assert.equal((await call("accountSites")).status, 403);
  assert.equal((await call("getTenant", [one.id])).status, 403);
  assert.equal((await call("manageTenant", [one.id, {}])).status, 403);
  await call("session");
  assert.equal(
    (
      await database.query(
        "SELECT count(*)::int AS count FROM webdock_auth.access_event WHERE actor_id=$1 AND target_id=$2 AND action='tenant-preview-expired'",
        [operator.user.id, one.id],
      )
    ).rows[0].count,
    1,
    "Expiry is audited once",
  );
  const exit = await call("exitTenantPreview");
  assert.equal(exit.status, 200);
  assert.equal(exit.data.customerID, one.id);
  assert.equal((await call("session")).data.operator, true);
  assert.equal((await call("session")).data.preview, undefined);
  assert.ok(
    (await call("accountSites")).data.some(
      (s: { id: string }) => s.id === foreign.binding,
    ),
  );
  assert.equal((await call("startTenantPreview", [one.id])).status, 200);
  await database.query(
    "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
    [one.id],
  );
  assert.equal((await call("session")).data.preview.status, "expired");
  assert.equal((await call("accountSites")).status, 403);
  assert.equal((await call("exitTenantPreview")).status, 200);
  assert.equal((await call("startTenantPreview", [one.id])).status, 403);
  await database.query(
    "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
    [one.id],
  );
  await call("startTenantPreview", [one.id]);
  const newOrg = (
    await database.query(
      'INSERT INTO webdock_auth.organization(name,slug,"createdAt") VALUES($1,$2,now()) RETURNING id',
      ["Remap fixture", "preview-remap-" + randomBytes(6).toString("hex")],
    )
  ).rows[0].id;
  await database.query(
    "UPDATE webdock_auth.tenant_customer SET organization_id=$1 WHERE customer_id=$2",
    [newOrg, one.id],
  );
  assert.equal((await call("session")).data.preview.status, "expired");
  await database.query(
    "UPDATE webdock_auth.tenant_customer SET organization_id=$1 WHERE customer_id=$2",
    [org(one.id), one.id],
  );
  assert.equal(
    (await call("session")).data.preview.status,
    "expired",
    "Restoring a mapping never revives the context",
  );
  await call("exitTenantPreview");
  await call("startTenantPreview", [one.id]);
  for (const [column, blocked, restored] of [
    ["emailVerified", false, true],
    ["mustChangePassword", true, false],
    ["twoFactorEnabled", false, true],
  ] as const) {
    await database.query(
      `UPDATE webdock_auth."user" SET "${column}"=$1 WHERE id=$2`,
      [blocked, operator.user.id],
    );
    assert.equal((await call("accountSites")).status, 401, column);
    await database.query(
      `UPDATE webdock_auth."user" SET "${column}"=$1 WHERE id=$2`,
      [restored, operator.user.id],
    );
  }
  await database.query(
    `UPDATE webdock_auth.session SET "expiresAt"=now()-interval '1 second' WHERE id=$1`,
    [operator.sid],
  );
  assert.equal((await call("session")).status, 401);
  await database.query(
    `UPDATE webdock_auth.session SET "expiresAt"=now()+interval '1 hour' WHERE id=$1`,
    [operator.sid],
  );
  await database.query(
    'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
    [operator.user.id],
  );
  assert.equal((await call("session")).status, 401);
  await database.query(
    'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
    [operator.user.id],
  );
  await database.query(
    "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
    [operator.user.id],
  );
  assert.equal((await call("accountSites")).status, 403);
  await database.query(
    "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
    [operator.user.id],
  );
  await database.query("DELETE FROM webdock_auth.session WHERE id=$1", [
    operator.sid,
  ]);
  assert.equal((await call("session")).status, 401);
  assert.equal(
    (await call("session", [], { ...operator, sid: otherSession.id })).data
      .operator,
    true,
  );
  const events = (
    await database.query(
      "SELECT action FROM webdock_auth.access_event WHERE actor_id=$1 AND target_id=$2 AND action LIKE 'tenant-preview-%'",
      [operator.user.id, one.id],
    )
  ).rows.map((r) => r.action);
  assert.ok(events.includes("tenant-preview-start"));
  assert.ok(events.includes("tenant-preview-exit"));
});

test("Preview start works with runtime column grants and a read-only tenant mapping", async (t) => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(tenantPreviewSchemaSQL);
  const operator = await identity(true);
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Restricted runtime preview') RETURNING id",
    )
  ).rows[0];
  const role = "preview_runtime_" + randomBytes(6).toString("hex");
  const connection = await database.connect();
  let mocked: ReturnType<typeof mockMethod> | undefined;
  try {
    await connection.query(`CREATE ROLE "${role}" NOLOGIN NOSUPERUSER`);
    await connection.query(
      `GRANT USAGE ON SCHEMA webdock_auth,webdock_admin TO "${role}"`,
    );
    await connection.query(
      `GRANT SELECT ON webdock_auth."user",webdock_auth.session,webdock_auth.tenant_customer TO "${role}"`,
    );
    await connection.query(
      `GRANT UPDATE(id) ON webdock_auth."user",webdock_auth.session TO "${role}"`,
    );
    await connection.query(
      `GRANT SELECT(id,name,status),UPDATE(name) ON webdock_admin.customers TO "${role}"`,
    );
    await connection.query(
      `GRANT SELECT,INSERT ON webdock_auth.studio_tenant_preview TO "${role}"`,
    );
    await connection.query(
      `GRANT INSERT ON webdock_auth.access_event TO "${role}"`,
    );
    await connection.query(
      `GRANT SELECT,UPDATE ON webdock_auth.snowflake_state TO "${role}"`,
    );
    await connection.query(
      `GRANT EXECUTE ON FUNCTION webdock_auth.next_snowflake() TO "${role}"`,
    );
    await connection.query(`SET ROLE "${role}"`);
    assert.equal(
      (
        await connection.query(
          "SELECT has_table_privilege(current_user,'webdock_auth.tenant_customer','UPDATE') AS writable",
        )
      ).rows[0].writable,
      false,
    );
    // Exercise the actual transaction with restricted grants, holding the borrowed
    // connection until its role has been reset by this test.
    mocked = mockMethod(
      database,
      "connect",
      async () =>
        ({
          query: connection.query.bind(connection),
          release: () => {},
        }) as never,
    );
    const { startTenantPreview } =
      await import("../../src/modules/access/tenant-preview");
    const preview = await startTenantPreview(
      { userID: operator.user.id, sessionID: operator.sid },
      customer.id,
    );
    assert.equal(preview.customerID, customer.id);
    assert.equal(preview.status, "active");
  } finally {
    mocked?.mock.restore();
    await connection.query("RESET ROLE");
    await connection.query(`DROP OWNED BY "${role}"`);
    await connection.query(`DROP ROLE "${role}"`);
    connection.release();
  }
});
