import { configureFullProducts } from "../../src/full-products";
configureFullProducts();
import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../../src/modules/auth/bootstrap";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import {
  accessSchemaSQL,
  AccessError,
  accountSites,
} from "../../src/modules/access/access-management";
import {
  getTenant,
  listTenants,
  listTenantInvitations,
  manageTenant,
  validateTenantProfile,
} from "../../src/modules/access/tenants";
import { currentClaims } from "../../src/modules/auth/authorization";
import { testOutbox } from "../../src/modules/auth/mail";
const origin = process.env.BETTER_AUTH_URL!;
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database and test outbox required");
test.after(async () => {
  await auth.$context;
  await database.end();
});
const call = (headers: Headers, path: string, body?: unknown) =>
  auth.handler(
    new Request(origin + "/api/auth" + path, {
      method: body ? "POST" : "GET",
      headers: new Headers([...headers, ["Content-Type", "application/json"]]),
      body: body ? JSON.stringify(body) : undefined,
    }),
  );
async function signIn(email: string, password: string) {
  const response = await call(
    new Headers({ Origin: origin }),
    "/sign-in/email",
    { email, password },
  );
  assert.equal(response.status, 200, await response.text());
  return new Headers({
    Origin: origin,
    Cookie: response.headers
      .getSetCookie()
      .map((cookie) => cookie.split(";")[0])
      .join("; "),
  });
}
async function identity(name: string, operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `tenant-${name}-${randomBytes(6).toString("hex")}@example.invalid`,
    name,
    password,
    operator,
    mustChangePassword: false,
  });
  const headers = await signIn(user.email, password);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  return { ...user, headers };
}
test("Tenant profile validation", () => {
  assert.equal(
    validateTenantProfile({
      customer_type: "person",
      name: " Jane ",
      country: "de",
    }).country,
    "DE",
  );
  for (const input of [
    { customer_type: "operator", name: "Bad" },
    { customer_type: "person", name: "A", contact_email: "bad" },
    { customer_type: "person", name: "A", phone: "x".repeat(51) },
  ] as Record<string, string>[])
    assert.throws(() => validateTenantProfile(input));
});
test("Two tenants isolate profiles, membership, invitations, native endpoints and website claims", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  const operator = await identity("operator", true),
    admin = await identity("admin"),
    stranger = await identity("stranger"),
    member = await identity("member");
  const customers = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name,notes) VALUES(webdock_auth.next_snowflake(),'Tenant One','secret operator notes'),(webdock_auth.next_snowflake(),'Tenant Two','other notes') RETURNING id,name",
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
    mappings.find((m) => m.customer_id === id)!.organization_id;
  await database.query(
    "INSERT INTO webdock_auth.member(\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,'admin',now()),($1,$3,'member',now()),($4,$5,'admin',now())",
    [org(one.id), admin.id, member.id, org(two.id), stranger.id],
  );
  assert.deepEqual(
    (await listTenants(admin.headers)).map((t) => t.id),
    [one.id],
  );
  const profile = await getTenant(admin.headers, one.id);
  assert.equal(profile.tenant.notes, undefined);
  assert.equal(
    profile.members.some((m) => m.email === stranger.email),
    false,
  );
  await assert.rejects(getTenant(admin.headers, two.id));
  await assert.rejects(getTenant(new Headers(), one.id));
  for (const [headers, target, input] of [
    [
      admin.headers,
      two.id,
      { action: "profile", customer_type: "person", name: "Hijack" },
    ],
    [
      admin.headers,
      two.id,
      {
        action: "invite",
        email: "bad@example.invalid",
        name: "Bad",
        role: "admin",
      },
    ],
    [
      member.headers,
      one.id,
      { action: "profile", customer_type: "person", name: "Bad" },
    ],
    [
      member.headers,
      one.id,
      {
        action: "invite",
        email: "bad@example.invalid",
        name: "Bad",
        role: "member",
      },
    ],
  ] as [Headers, string, Record<string, string>][])
    await assert.rejects(manageTenant(headers, target, input));
  await manageTenant(admin.headers, one.id, {
    action: "profile",
    customer_type: "person",
    name: "Tenant Person",
    first_name: "Jane",
    last_name: "Doe",
    country: "de",
    city: "Berlin",
    notes: "overwritten",
    status: "archived",
  });
  assert.deepEqual(
    (
      await database.query(
        "SELECT name,status,notes,country FROM webdock_admin.customers WHERE id=$1",
        [one.id],
      )
    ).rows[0],
    {
      name: "Tenant Person",
      status: "active",
      notes: "secret operator notes",
      country: "DE",
    },
  );
  assert.equal(
    (
      await database.query(
        "SELECT name FROM webdock_auth.organization WHERE id=$1",
        [org(one.id)],
      )
    ).rows[0].name,
    "Tenant Person",
  );
  const adminMembership = (
    await database.query(
      'SELECT id FROM webdock_auth.member WHERE "userId"=$1 AND "organizationId"=$2',
      [admin.id, org(one.id)],
    )
  ).rows[0].id;
  await assert.rejects(
    manageTenant(operator.headers, one.id, {
      action: "role",
      member: adminMembership,
      role: "member",
    }),
    /at least one/,
  );
  await assert.rejects(
    manageTenant(operator.headers, one.id, {
      action: "remove",
      member: adminMembership,
    }),
    /at least one/,
  );
  await assert.rejects(
    manageTenant(admin.headers, one.id, {
      action: "role",
      member: adminMembership,
      role: "admin",
    }),
  );
  await assert.rejects(
    manageTenant(operator.headers, two.id, {
      action: "remove",
      member: adminMembership,
    }),
  );
  for (const [path, body] of [
    [
      "/organization/update-member-role",
      { organizationId: org(one.id), memberId: adminMembership, role: "owner" },
    ],
    [
      "/organization/remove-member",
      { organizationId: org(one.id), memberIdOrEmail: adminMembership },
    ],
    ["/organization/leave", { organizationId: org(one.id) }],
    [
      "/organization/invite-member",
      {
        organizationId: org(one.id),
        email: "bypass@example.invalid",
        role: "admin",
      },
    ],
  ] as const)
    assert.equal((await call(admin.headers, path, body)).status, 403, path);
  for (const path of [
    `/organization/get-full-organization?organizationId=${org(two.id)}`,
    `/organization/list-members?organizationId=${org(two.id)}`,
    "/organization/list",
  ])
    assert.equal((await call(admin.headers, path)).status, 403);
  assert.equal(
    (
      await call(operator.headers, "/organization/update-member-role", {
        organizationId: org(one.id),
        memberId: adminMembership,
        role: "member",
      })
    ).status,
    403,
  );
  await assert.rejects(
    manageTenant(admin.headers, one.id, {
      action: "invite",
      email: operator.email,
      name: "Operator",
      role: "admin",
    }),
  );
  await assert.rejects(
    manageTenant(admin.headers, one.id, {
      action: "invite",
      email: "owner@example.invalid",
      name: "Owner",
      role: "owner",
    }),
  );
  const email = `invite-${randomBytes(8).toString("hex")}@example.invalid`;
  await manageTenant(admin.headers, one.id, {
    action: "invite",
    email,
    name: "Invited customer",
    role: "admin",
  });
  const invitation = (
    await database.query(
      "SELECT id,role FROM webdock_auth.invitation WHERE email=$1 AND \"organizationId\"=$2 AND status='pending'",
      [email, org(one.id)],
    )
  ).rows[0];
  await assert.rejects(
    manageTenant(admin.headers, one.id, {
      action: "invite",
      email,
      name: "Invited customer",
      role: "member",
    }),
    /pending invitation/,
  );
  assert.equal(invitation.role, "admin");
  const user = (
    await database.query('SELECT id FROM webdock_auth."user" WHERE email=$1', [
      email,
    ])
  ).rows[0];
  assert.equal(
    (
      await database.query(
        'SELECT id FROM webdock_auth.member WHERE "userId"=$1',
        [user.id],
      )
    ).rowCount,
    0,
  );
  const mail = testOutbox.findLast((m) => m.to === email)!;
  const link = new URL(mail.text.match(/https?:\/\/\S+/)![0]),
    token = link.pathname.split("/").at(-1)!;
  const password = randomBytes(24).toString("base64url");
  assert.equal(
    (
      await call(new Headers({ Origin: origin }), "/reset-password", {
        token,
        newPassword: password,
      })
    ).status,
    200,
  );
  const invitedHeaders = await signIn(email, password);
  assert.equal((await listTenants(invitedHeaders)).length, 0);
  assert.deepEqual(
    (await listTenantInvitations(invitedHeaders)).map((i) => i.id),
    [invitation.id],
  );
  assert.equal((await listTenantInvitations(stranger.headers)).length, 0);
  await assert.rejects(getTenant(invitedHeaders, one.id));
  assert.equal(
    (
      await call(stranger.headers, "/organization/accept-invitation", {
        invitationId: invitation.id,
      })
    ).status,
    403,
  );
  assert.equal(
    (
      await call(
        stranger.headers,
        `/organization/get-invitation?id=${invitation.id}`,
      )
    ).status,
    403,
  );
  await manageTenant(admin.headers, one.id, {
    action: "resend",
    invitation: invitation.id,
  });
  for (const action of ["cancel", "resend"])
    await assert.rejects(
      manageTenant(stranger.headers, two.id, {
        action,
        invitation: invitation.id,
      }),
    );
  await database.query(
    "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
    [one.id],
  );
  assert.equal(
    (
      await call(invitedHeaders, "/organization/accept-invitation", {
        invitationId: invitation.id,
      })
    ).status,
    403,
  );
  assert.equal((await listTenantInvitations(invitedHeaders)).length, 0);
  await database.query(
    "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
    [one.id],
  );
  await database.query(
    'UPDATE webdock_auth."user" SET "mustChangePassword"=true WHERE id=$1',
    [user.id],
  );
  assert.equal(
    (
      await call(invitedHeaders, "/organization/accept-invitation", {
        invitationId: invitation.id,
      })
    ).status,
    403,
  );
  await database.query(
    'UPDATE webdock_auth."user" SET "mustChangePassword"=false WHERE id=$1',
    [user.id],
  );
  assert.equal(
    (
      await call(invitedHeaders, "/organization/accept-invitation", {
        invitationId: invitation.id,
      })
    ).status,
    200,
  );
  assert.equal((await listTenants(invitedHeaders))[0].id, one.id);
  assert.equal((await getTenant(invitedHeaders, one.id)).canManage, true);
  await assert.rejects(getTenant(invitedHeaders, two.id));
  const site = await registerApplication({
    label: "Tenant website",
    origin: "http://127.0.0.1:3202",
    logoutPath: "/admin/login",
    headers: operator.headers,
    organizationID: org(one.id),
  });
  assert.equal(
    (await currentClaims(user.id, site.binding)).webdock_role,
    "admin",
    "Mapped tenant admins inherit their own CMS access",
  );
  const contextualSite = (await accountSites(invitedHeaders)).find(
    (item) => item.id === site.binding,
  ) as { customerID?: string; customerName?: string };
  assert.equal(contextualSite.customerID, one.id);
  assert.equal(
    contextualSite.customerName,
    (await getTenant(invitedHeaders, one.id)).tenant.name,
  );
  assert.ok(
    !(await accountSites(stranger.headers)).some(
      (item) => item.id === site.binding,
    ),
  );
  assert.equal(
    (await currentClaims(admin.id, site.binding)).webdock_role,
    "admin",
  );
  assert.equal(
    (await currentClaims(stranger.id, site.binding)).disabled,
    true,
    "Other tenant admins cannot cross the binding boundary",
  );
  assert.equal(
    (await currentClaims(member.id, site.binding)).disabled,
    true,
    "Ordinary membership alone cannot edit a CMS",
  );
  await database.query(
    'UPDATE webdock_auth.member SET role=\'owner\' WHERE "userId"=$1 AND "organizationId"=$2',
    [user.id, org(one.id)],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).webdock_role,
    "admin",
    "Mapped tenant owners inherit CMS administration",
  );
  await manageTenant(operator.headers, one.id, {
    action: "role",
    member: adminMembership,
    role: "owner",
  });
  await database.query(
    'UPDATE webdock_auth.member SET role=\'admin\' WHERE "userId"=$1 AND "organizationId"=$2',
    [user.id, org(one.id)],
  );
  for (const [column, blocked, restored] of [
    ["emailVerified", false, true],
    ["mustChangePassword", true, false],
  ] as const) {
    await database.query(
      `UPDATE webdock_auth."user" SET "${column}"=$1 WHERE id=$2`,
      [blocked, user.id],
    );
    assert.equal(
      (await currentClaims(user.id, site.binding)).disabled,
      true,
      column,
    );
    await database.query(
      `UPDATE webdock_auth."user" SET "${column}"=$1 WHERE id=$2`,
      [restored, user.id],
    );
  }
  await database.query(
    "UPDATE webdock_auth.app_binding SET enabled=false WHERE id=$1",
    [site.binding],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).disabled,
    true,
    "Disabled bindings override inherited access",
  );
  await database.query(
    "UPDATE webdock_auth.app_binding SET enabled=true WHERE id=$1",
    [site.binding],
  );
  await database.query(
    'UPDATE webdock_auth."oauthClient" SET disabled=true WHERE "clientId"=$1',
    [site.clientID],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).disabled,
    true,
    "Disabled clients override inherited access",
  );
  await database.query(
    'UPDATE webdock_auth."oauthClient" SET disabled=false WHERE "clientId"=$1',
    [site.clientID],
  );
  await database.query(
    "INSERT INTO webdock_auth.project_grant(user_id,binding_id,organization_id,role,enabled) VALUES($1,$2,$3,'reader',true)",
    [user.id, site.binding, org(one.id)],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).webdock_role,
    "admin",
    "Enabled grants do not reduce tenant admin authority",
  );
  await database.query(
    "UPDATE webdock_auth.project_grant SET enabled=false WHERE user_id=$1 AND binding_id=$2",
    [user.id, site.binding],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).disabled,
    true,
    "Explicit revocation overrides inherited access",
  );
  assert.equal((await getTenant(invitedHeaders, one.id)).sites.length, 0);
  await database.query(
    "UPDATE webdock_auth.project_grant SET enabled=true,organization_id=$3 WHERE user_id=$1 AND binding_id=$2",
    [user.id, site.binding, org(two.id)],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).disabled,
    true,
    "A mismatched explicit grant fails closed",
  );
  await database.query(
    "UPDATE webdock_auth.project_grant SET organization_id=$3 WHERE user_id=$1 AND binding_id=$2",
    [user.id, site.binding, org(one.id)],
  );
  assert.equal(
    (await getTenant(invitedHeaders, one.id)).sites[0].role,
    "admin",
  );
  await database.query(
    "DELETE FROM webdock_auth.project_grant WHERE user_id=$1 AND binding_id=$2",
    [user.id, site.binding],
  );
  assert.equal(
    (await currentClaims(user.id, site.binding)).webdock_role,
    "admin",
    "Inherited access applies without an explicit grant",
  );
  await database.query(
    "INSERT INTO webdock_auth.project_grant(user_id,binding_id,organization_id,role,enabled) VALUES($1,$2,$3,'editor',true)",
    [member.id, site.binding, org(one.id)],
  );
  assert.equal(
    (await currentClaims(member.id, site.binding)).webdock_role,
    "editor",
    "Ordinary members keep explicit site roles",
  );
  await database.query(
    "UPDATE webdock_auth.project_grant SET enabled=false WHERE user_id=$1 AND binding_id=$2",
    [member.id, site.binding],
  );
  assert.equal(
    (await currentClaims(member.id, site.binding)).disabled,
    true,
    "Ordinary member grants remain revocable",
  );
  const previousPlatform = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  try {
    await database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"cmsEnabled\":false}') ON CONFLICT(id) DO UPDATE SET settings=EXCLUDED.settings",
    );
    assert.equal(
      (await currentClaims(user.id, site.binding)).disabled,
      true,
      "Root CMS switch revokes customer access",
    );
    assert.equal(
      (await currentClaims(operator.id, site.binding)).webdock_role,
      "operator",
      "Operator maintenance remains available",
    );
    assert.equal((await getTenant(invitedHeaders, one.id)).sites.length, 0);
  } finally {
    if (previousPlatform)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [JSON.stringify(previousPlatform.settings)],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
  assert.equal((await getTenant(admin.headers, one.id)).sites[0].role, "admin");
  await database.query(
    "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
    [one.id],
  );
  await assert.rejects(getTenant(invitedHeaders, one.id));
  assert.equal((await listTenants(invitedHeaders)).length, 0);
  assert.equal((await currentClaims(user.id, site.binding)).disabled, true);
  await assert.rejects(
    manageTenant(operator.headers, one.id, {
      action: "invite",
      email: "archived@example.invalid",
      name: "Archived",
      role: "member",
    }),
  );
  assert.equal(
    (
      await call(
        invitedHeaders,
        `/organization/get-full-organization?organizationId=${org(one.id)}`,
      )
    ).status,
    403,
  );
  await database.query(
    "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
    [one.id],
  );
  await database.query(
    'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
    [user.id],
  );
  await assert.rejects(getTenant(invitedHeaders, one.id));
  assert.equal((await currentClaims(user.id, site.binding)).disabled, true);
  await database.query(
    'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
    [user.id],
  );
  const invitedMembership = (
    await database.query(
      'SELECT id FROM webdock_auth.member WHERE "userId"=$1 AND "organizationId"=$2',
      [user.id, org(one.id)],
    )
  ).rows[0].id;
  const concurrentDemotions = await Promise.allSettled(
    [adminMembership, invitedMembership].map((memberID) =>
      manageTenant(operator.headers, one.id, {
        action: "role",
        member: memberID,
        role: "member",
      }),
    ),
  );
  assert.equal(
    concurrentDemotions.filter((result) => result.status === "fulfilled")
      .length,
    1,
    "Concurrent demotions preserve one usable customer administrator",
  );
  await manageTenant(operator.headers, one.id, {
    action: "role",
    member: adminMembership,
    role: "owner",
  });
  await manageTenant(operator.headers, one.id, {
    action: "role",
    member: invitedMembership,
    role: "member",
  });
  assert.equal((await getTenant(invitedHeaders, one.id)).canManage, false);
  await manageTenant(operator.headers, one.id, {
    action: "remove",
    member: invitedMembership,
  });
  await assert.rejects(getTenant(invitedHeaders, one.id));
  assert.equal((await currentClaims(user.id, site.binding)).disabled, true);
  const pendingEmail = `cancel-${randomBytes(8).toString("hex")}@example.invalid`;
  await manageTenant(operator.headers, one.id, {
    action: "invite",
    email: pendingEmail,
    name: "Pending",
    role: "member",
  });
  const pending = (
    await database.query(
      "SELECT id FROM webdock_auth.invitation WHERE email=$1",
      [pendingEmail],
    )
  ).rows[0];
  const operatorMembership = (
    await database.query(
      'SELECT id FROM webdock_auth.member WHERE "userId"=$1 AND "organizationId"=$2',
      [operator.id, org(one.id)],
    )
  ).rows[0].id;
  await assert.rejects(
    manageTenant(operator.headers, one.id, {
      action: "remove",
      member: operatorMembership,
    }),
  );
  await assert.rejects(
    manageTenant(operator.headers, one.id, {
      action: "role",
      member: operatorMembership,
      role: "member",
    }),
  );
  await manageTenant(admin.headers, one.id, {
    action: "cancel",
    invitation: pending.id,
  });
  assert.equal(
    (
      await database.query(
        "SELECT status FROM webdock_auth.invitation WHERE id=$1",
        [pending.id],
      )
    ).rows[0].status,
    "canceled",
  );
  await assert.rejects(
    manageTenant(admin.headers, one.id, {
      action: "resend",
      invitation: pending.id,
    }),
  );
  assert.ok(
    (
      await database.query(
        "SELECT id FROM webdock_auth.access_event WHERE target_id=$1 AND outcome='succeeded'",
        [one.id],
      )
    ).rowCount! > 5,
  );
  if (process.env.TENANT_BROWSER_QA === "true")
    writeFileSync(
      "/tmp/webdock-tenant-qa.json",
      JSON.stringify({
        origin,
        customerID: one.id,
        otherCustomerID: two.id,
        userID: admin.id,
        cookie: admin.headers.get("cookie"),
        operatorCookie: operator.headers.get("cookie"),
        userIDs: [operator.id, admin.id, stranger.id, member.id, user.id],
      }),
      { mode: 0o600 },
    );
});

test("failed invitation delivery is reported and audited, and the same invitation can be resent", async () => {
  const operator = await identity("mail-failure-operator", true);
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mail recovery') RETURNING id",
    )
  ).rows[0];
  const existing = await identity("mail-existing");
  const newEmail = `mail-new-${randomBytes(6).toString("hex")}@example.invalid`;
  for (const email of [newEmail, existing.email]) {
    const previousHost = process.env.SMTP_HOST;
    try {
      process.env.AUTH_TEST_MAIL = "false";
      delete process.env.SMTP_HOST;
      await assert.rejects(
        manageTenant(operator.headers, customer.id, {
          action: "invite",
          email,
          name: "Mail recovery",
          role: "member",
        }),
        (error) =>
          error instanceof AccessError &&
          /email.*not.*sent/i.test(error.message),
      );
    } finally {
      process.env.AUTH_TEST_MAIL = "true";
      if (previousHost === undefined) delete process.env.SMTP_HOST;
      else process.env.SMTP_HOST = previousHost;
    }
    const invitation = (
      await database.query(
        "SELECT id,status FROM webdock_auth.invitation WHERE email=$1",
        [email],
      )
    ).rows[0];
    assert.equal(invitation.status, "pending");
    const { handleNativeStudioRequest } =
      await import("../../src/modules/portal/studio-api");
    try {
      process.env.AUTH_TEST_MAIL = "false";
      delete process.env.SMTP_HOST;
      const response = await handleNativeStudioRequest(
        new Request(origin + "/v1/auth/studio", {
          method: "POST",
          headers: new Headers([
            ...operator.headers,
            ["content-type", "application/json"],
          ]),
          body: JSON.stringify({
            operation: "manageTenant",
            args: [
              customer.id,
              { action: "resend", invitation: invitation.id },
            ],
          }),
        }),
      );
      assert.equal(response.ok, false);
      const body = await response.json();
      assert.equal(body.error.kind, "AccessError");
      assert.match(body.error.message, /email.*not.*sent/i);
      assert.doesNotMatch(body.error.message, /SMTP|password|transport/i);
    } finally {
      process.env.AUTH_TEST_MAIL = "true";
      if (previousHost === undefined) delete process.env.SMTP_HOST;
      else process.env.SMTP_HOST = previousHost;
    }
    assert.equal(
      (
        await database.query(
          "SELECT outcome FROM webdock_auth.access_event WHERE target_id=$1 AND action='tenant-invite' ORDER BY created_at DESC LIMIT 1",
          [customer.id],
        )
      ).rows[0].outcome,
      "failed",
    );
    const before = testOutbox.filter((mail) => mail.to === email).length;
    await manageTenant(operator.headers, customer.id, {
      action: "resend",
      invitation: invitation.id,
    });
    assert.equal(
      testOutbox.filter((mail) => mail.to === email).length,
      before + 1,
    );
    assert.equal(
      (
        await database.query(
          "SELECT id FROM webdock_auth.invitation WHERE email=$1",
          [email],
        )
      ).rows[0].id,
      invitation.id,
    );
  }
});
