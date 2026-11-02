import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../../src/modules/auth/bootstrap";
import { currentClaims } from "../../src/modules/auth/authorization";
import { testOutbox } from "../../src/modules/auth/mail";
import {
  accessSchemaSQL,
  manageAccess,
  listAccess,
  accountSites,
} from "../../src/modules/access/access-management";
const origin = process.env.BETTER_AUTH_URL!;
const dbURL = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(dbURL.hostname) ||
  dbURL.pathname !== "/webdock_api_test"
)
  throw Error("Disposable local DB only");
test.after(async () => {
  await auth.$context;
  await database.end();
});
test("Operator invites a customer; setup, membership and website grants control SSO", async () => {
  await database.query(accessSchemaSQL);
  const password = randomBytes(24).toString("base64url");
  const operator = await createIdentity({
    email: `access-${Date.now()}@example.invalid`,
    name: "Access operator",
    password,
    operator: true,
    mustChangePassword: false,
  });
  const signIn = async (email: string, password: string) => {
    const r = await auth.handler(
      new Request(origin + "/api/auth/sign-in/email", {
        method: "POST",
        headers: { Origin: origin, "Content-Type": "application/json" },
        body: JSON.stringify({ email, password }),
      }),
    );
    assert.equal(r.status, 200);
    return new Headers({
      Cookie: r.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
      Origin: origin,
    });
  };
  const headers = await signIn(operator.email, password);
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
    [operator.id],
  );
  await assert.rejects(
    manageAccess(new Headers(), {
      action: "create-organization",
      name: "Forbidden",
    }),
  );
  const org = await manageAccess(headers, {
    action: "create-organization",
    name: "Customer test",
  });
  const app = await registerApplication({
    label: "Invited customer site",
    origin: "http://127.0.0.1:3190",
    logoutPath: "/admin/login",
    headers,
  });
  const unrelated = await registerApplication({
    label: "Other site",
    origin: "http://127.0.0.1:3191",
    logoutPath: "/admin/login",
    headers,
  });
  await manageAccess(headers, {
    action: "link-website",
    binding: app.binding,
    organization: org.id!,
  });
  const email = `invite-${Date.now()}@example.invalid`;
  const invite = await manageAccess(headers, {
    action: "invite",
    name: "Customer editor",
    email,
    binding: app.binding,
    role: "editor",
  });
  assert.ok(invite.id);
  const user = (
    await database.query('SELECT * FROM webdock_auth."user" WHERE email=$1', [
      email,
    ])
  ).rows[0];
  assert.equal(user.role, "user");
  assert.equal(user.emailVerified, false);
  assert.equal((await currentClaims(user.id, app.binding)).disabled, true);
  const mail = testOutbox.findLast((m) => m.to === email)!;
  assert.ok(mail.text.includes("reset-password"));
  const link = new URL(mail.text.match(/https?:\/\/\S+/)![0]);
  const token = link.pathname.split("/").at(-1)!;
  const newPassword = randomBytes(24).toString("base64url");
  const reset = await auth.handler(
    new Request(origin + "/api/auth/reset-password", {
      method: "POST",
      headers: { Origin: origin, "Content-Type": "application/json" },
      body: JSON.stringify({ token, newPassword }),
    }),
  );
  assert.equal(reset.status, 200);
  const customerHeaders = await signIn(email, newPassword);
  assert.equal(
    (await currentClaims(user.id, app.binding)).disabled,
    true,
    "No access before accepting invitation",
  );
  await auth.api.acceptInvitation({
    headers: customerHeaders,
    body: { invitationId: invite.id! },
  });
  assert.equal(
    (await currentClaims(user.id, app.binding)).webdock_role,
    "editor",
  );
  assert.equal(
    (await currentClaims(user.id, unrelated.binding)).disabled,
    true,
  );
  assert.equal((await accountSites(customerHeaders)).length, 1);
  await assert.rejects(listAccess(customerHeaders, ""));
  await assert.rejects(
    manageAccess(customerHeaders, {
      action: "suspend",
      user: operator.id,
      blocked: "true",
    }),
  );
  await assert.rejects(
    manageAccess(headers, {
      action: "suspend",
      user: operator.id,
      blocked: "true",
    }),
  );
  await assert.rejects(
    manageAccess(headers, {
      action: "invite",
      name: "Bad",
      email,
      binding: app.binding,
      role: "operator",
    }),
  );
  const otherPassword = randomBytes(24).toString("base64url");
  const otherOperator = await createIdentity({
    email: `second-operator-${Date.now()}@example.invalid`,
    name: "Second operator",
    password: otherPassword,
    operator: true,
    mustChangePassword: false,
  });
  const secondHeaders = await signIn(otherOperator.email, otherPassword);
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
    [otherOperator.id],
  );
  const pendingEmail = `pending-${Date.now()}@example.invalid`;
  const pending = await manageAccess(headers, {
    action: "invite",
    name: "Pending customer",
    email: pendingEmail,
    binding: app.binding,
    role: "editor",
  });
  await manageAccess(secondHeaders, {
    action: "cancel-invite",
    id: pending.id!,
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
  assert.equal(
    (
      await database.query(
        'SELECT g.enabled FROM webdock_auth.project_grant g JOIN webdock_auth."user" u ON u.id=g.user_id WHERE u.email=$1',
        [pendingEmail],
      )
    ).rows[0].enabled,
    false,
    "Cancellation withdraws pending website permissions",
  );
  const grant = (
    await database.query(
      "SELECT id FROM webdock_auth.project_grant WHERE user_id=$1 AND binding_id=$2",
      [user.id, app.binding],
    )
  ).rows[0];
  await manageAccess(headers, {
    action: "change-access",
    grant: grant.id,
    role: "reader",
  });
  assert.equal(
    (await currentClaims(user.id, app.binding)).webdock_role,
    "reader",
  );
  await manageAccess(headers, { action: "revoke-access", grant: grant.id });
  assert.equal((await currentClaims(user.id, app.binding)).disabled, true);
  await manageAccess(headers, {
    action: "suspend",
    user: user.id,
    blocked: "true",
  });
  assert.equal(await auth.api.getSession({ headers: customerHeaders }), null);
  assert.ok(
    (
      await database.query(
        "SELECT id FROM webdock_auth.access_event WHERE actor_id=$1",
        [operator.id],
      )
    ).rowCount! >= 6,
  );
});
