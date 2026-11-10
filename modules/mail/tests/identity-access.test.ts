import { test } from "node:test";
import assert from "node:assert/strict";
import type {
  DatabaseReader,
  IdentityDirectory,
  SessionIdentity,
  TenantMembership,
} from "@spitzli/core/identity";
import { createNativeMailAuthorization } from "../src/server/native-access.ts";
import { createMailAuthorization } from "../src/server/product-access.ts";
function fixture() {
  let identity: SessionIdentity | null = {
    subject: "123",
    sessionID: "session-123",
    role: "user",
    banned: false,
    emailVerified: true,
    mustChangePassword: false,
    twoFactorEnabled: false,
    preview: false,
  };
  let membership: TenantMembership | null = {
    memberID: "member-123",
    role: "admin",
  };
  let grant = false;
  const calls: string[] = [];
  let reads = 0;
  const directory: IdentityDirectory = {
    session: async () => {
      reads++;
      return identity;
    },
    subject: async () => identity,
    membership: async () => membership,
  };
  const db = {
    query: async (sql: string) => {
      calls.push(sql);
      assert.doesNotMatch(
        sql,
        /webdock_auth\.(?:"user"|session|member|studio_tenant_preview)/,
      );
      let rows: unknown[] = [];
      if (sql.includes("mail_connection m"))
        rows = [
          {
            customer_status: "active",
            kind: "platform",
            state: "ready",
            settings: { mailGatewayEnabled: true, mailEnabled: true },
          },
        ];
      else if (sql.includes("webdock_admin.customers"))
        rows = [{ status: "active" }];
      else if (sql.includes("webdock_mail.service")) rows = [{ ready: true }];
      else if (sql.includes("mailbox_grant"))
        rows = grant ? [{ id: "789", grantRevision: "4" }] : [];
      else if (sql.includes("mail_api_key"))
        rows = [{ permission: "sending_access" }];
      else if (sql.includes("mail_message")) rows = [{}];
      return { rows, rowCount: rows.length };
    },
  } as unknown as DatabaseReader;
  return {
    db,
    directory,
    calls,
    get reads() {
      return reads;
    },
    setIdentity: (value: SessionIdentity | null) => (identity = value),
    get identity() {
      return identity!;
    },
    setMembership: (value: TenantMembership | null) => (membership = value),
    grant: () => (grant = true),
  };
}
test("native Mail authorizes through an alternate identity directory and rechecks revocations", async () => {
  const f = fixture(),
    policy = createNativeMailAuthorization(f.directory),
    actor = { subject: "123", sessionID: "session-123" };
  assert.equal(
    (await policy.authorizeMail(f.db, actor, "456", true)).canManage,
    true,
  );
  f.setMembership({ memberID: null, role: null });
  await assert.rejects(policy.authorizeMail(f.db, actor, "456"));
  f.setMembership({ memberID: "member-123", role: "admin" });
  f.setIdentity({ ...f.identity, preview: true });
  await assert.rejects(policy.authorizeMail(f.db, actor, "456"));
  f.setIdentity(null);
  await assert.rejects(policy.authorizeMail(f.db, actor, "456"));
  assert.equal(f.reads, 4);
});
test("operator management never grants mailbox content, including queued work and changed grant revisions", async () => {
  const f = fixture(),
    policy = createNativeMailAuthorization(f.directory),
    actor = { subject: "123", sessionID: "session-123" };
  f.setIdentity({ ...f.identity, role: "operator", twoFactorEnabled: true });
  f.setMembership({ memberID: null, role: null });
  assert.equal((await policy.authorizeMail(f.db, actor, "456")).operator, true);
  await assert.rejects(policy.authorizeMailbox(f.db, actor, "456", "789"));
  f.grant();
  await policy.authorizeMailbox(f.db, actor, "456", "789");
  await policy.authorizeQueuedMailbox(f.db, {
    subject: "123",
    customerID: "456",
    mailboxID: "789",
    grantRevision: "4",
  });
  await assert.rejects(
    policy.authorizeQueuedMailbox(f.db, {
      subject: "123",
      customerID: "456",
      mailboxID: "789",
      grantRevision: "3",
    }),
  );
  f.setIdentity({ ...f.identity, banned: true });
  await assert.rejects(
    policy.authorizeQueuedMailbox(f.db, {
      subject: "123",
      customerID: "456",
      mailboxID: "789",
      grantRevision: "4",
    }),
  );
});
test("product Mail keeps local access rules independent of provider identity types", async () => {
  const f = fixture(),
    policy = createMailAuthorization({
      identity: f.directory,
      byokAllowed: async () => true,
    });
  const actor = {
    customerID: "456",
    connectionID: "789",
    principal: {
      kind: "session" as const,
      userID: "123",
      sessionID: "session-123",
    },
  };
  await policy.assertMailAccess(actor, "manage", f.db);
  for (const changed of [
    { preview: true },
    { banned: true },
    { emailVerified: false },
    { mustChangePassword: true },
    { role: "operator", twoFactorEnabled: false },
    { subject: "other" },
  ]) {
    const original = f.identity;
    f.setIdentity({ ...original, ...changed });
    await assert.rejects(policy.assertMailAccess(actor, "read", f.db));
    f.setIdentity(original);
  }
  f.setMembership({ memberID: "member-123", role: "member" });
  await policy.assertMailAccess(actor, "send", f.db);
  await assert.rejects(policy.assertMailAccess(actor, "manage", f.db));
  f.setMembership(null);
  await assert.rejects(policy.assertMailAccess(actor, "read", f.db));
});
test("API keys and jobs do not acquire session authority or bypass their live checks", async () => {
  const f = fixture(),
    policy = createMailAuthorization({
      identity: f.directory,
      byokAllowed: async () => true,
    });
  const key = {
    customerID: "456",
    connectionID: "789",
    principal: { kind: "api_key" as const, keyID: "12" },
  };
  await policy.assertMailAccess(key, "send", f.db);
  await assert.rejects(policy.assertMailAccess(key, "manage", f.db));
  const job = { ...key, principal: { kind: "job" as const, jobID: "34" } };
  await policy.assertMailAccess(job, "send", f.db);
  await assert.rejects(policy.assertMailAccess(job, "read", f.db));
  assert.equal(f.reads, 0);
  assert.ok(f.calls.some((sql) => sql.includes("lease_until>now()")));
  assert.ok(f.calls.some((sql) => sql.includes("revoked_at IS NULL")));
});
test("onboarding admission rechecks a live manager and never turns tenant admin into platform operator", async () => {
  const f = fixture(),
    policy = createMailAuthorization({
      identity: f.directory,
      byokAllowed: async () => false,
    });
  const reference = { subject: "123", sessionID: "session-123" };
  assert.deepEqual(await policy.assertMailManager(reference, "456", f.db), {
    subject: "123",
    operator: false,
  });
  f.setMembership({ memberID: "member-123", role: "member" });
  await assert.rejects(policy.assertMailManager(reference, "456", f.db));
  f.setIdentity({ ...f.identity, role: "operator", twoFactorEnabled: true });
  assert.equal(
    (await policy.assertMailManager(reference, "456", f.db)).operator,
    true,
  );
  f.setMembership(null);
  await assert.rejects(policy.assertMailManager(reference, "456", f.db));
});

test("native Mail uses provider-qualified grants without requiring a legacy membership row", async () => {
  const f = fixture();
  f.setMembership({ memberID: null, role: "admin" });
  const provider = { operator: false, canManage: true, clerkGrant: '["issuer","user","org","membership"]' };
  let live: typeof provider | null = provider;
  const auth = createNativeMailAuthorization(f.directory, async () => live);
  const actor = { subject: "123", sessionID: "session-123" };
  const access = await auth.authorizeMail(f.db, actor, "456", true);
  assert.equal(access.clerkGrant, provider.clerkGrant);
  assert.equal(access.memberID, null);
  live = null;
  await assert.rejects(() => auth.authorizeMail(f.db, actor, "456", true));
  live = provider;
  f.setMembership({ memberID: null, role: null });
  assert.equal((await auth.authorizeMail(f.db, actor, "456", true)).canManage, true);
});

test("fresh Clerk Mail role overrides stale identity operator privilege", async () => {
  const f = fixture();
  f.setIdentity({ ...f.identity, role: "operator", twoFactorEnabled: true });
  f.setMembership({ memberID: null, role: "member" });
  let live = { operator: false, canManage: false, clerkGrant: "verified" };
  const policy = createNativeMailAuthorization(f.directory, async () => live);
  const actor = { subject: "123", sessionID: "session-123" };
  assert.equal((await policy.authorizeMail(f.db, actor, "456")).operator, false);
  await assert.rejects(policy.authorizeMail(f.db, actor, "456", true));
  f.setIdentity({ ...f.identity, role: "user" });
  f.setMembership({ memberID: null, role: null });
  live = { operator: true, canManage: true, clerkGrant: "verified" };
  assert.equal((await policy.authorizeMail(f.db, actor, "456", true)).operator, true);
});
