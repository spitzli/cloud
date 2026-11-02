import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { accessSchemaSQL } from "../../src/modules/access/access-management";
import {
  platformSchemaSQL,
  decryptMailSecret,
} from "../../src/modules/platform/platform";
import {
  getTenantMail,
  mailSchemaSQL,
  manageTenantMail,
  normalizeMailDomain,
} from "../../src/modules/mail/tenant-mail";
import {
  TurboSMTPError,
  type TurboSMTPCreateRequest,
  type TurboSMTPSubaccount,
} from "../../src/modules/mail/turbosmtp";
const origin = process.env.BETTER_AUTH_URL!;
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(async () => {
  await auth.$context;
  await database.end();
});
async function identity(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `mail-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Mail test",
    password,
    operator,
    mustChangePassword: false,
  });
  const response = await auth.handler(
    new Request(origin + "/api/auth/sign-in/email", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        "x-webdock-client-ip": `192.0.2.${1 + (randomBytes(1)[0] % 250)}`,
      },
      body: JSON.stringify({ email: user.email, password }),
    }),
  );
  assert.equal(response.status, 200);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  return {
    ...user,
    headers: new Headers({
      Origin: origin,
      Cookie: response.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
    }),
  };
}
test("Domain inputs reject URLs, private IPs and malformed labels", () => {
  assert.equal(normalizeMailDomain(" EXAMPLE.COM "), "example.com");
  for (const value of [
    "http://example.com",
    "127.0.0.1",
    "localhost",
    "-bad.com",
    "example.com/path",
    "a..com",
  ])
    assert.throws(() => normalizeMailDomain(value));
});
test("Mail isolates two tenants, reserves uncertain provisioning and verifies only public ownership", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(platformSchemaSQL);
  await database.query(mailSchemaSQL);
  const previous = (
    await database.query(
      "SELECT * FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const root = await identity(true),
    admin = await identity(),
    other = await identity(),
    pending = await identity();
  const tenants = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mail A'),(webdock_auth.next_snowflake(),'Mail B') RETURNING id",
    )
  ).rows;
  const [one, two] = tenants.map((t) => t.id);
  const mappings = (
    await database.query(
      "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [[one, two]],
    )
  ).rows;
  const org = (id: string) =>
    mappings.find((m) => m.customer_id === id)!.organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now()),($3,$4,\'admin\',now())',
    [org(one), admin.id, org(two), other.id],
  );
  const settings = {
    mailEnabled: true,
    mailSendingIP: "203.0.113.8",
    mailDefaultLimit: 123,
  };
  const setSettings = (enabled: boolean) =>
    database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET settings=$1",
      [JSON.stringify({ ...settings, mailEnabled: enabled })],
    );
  const prefix = randomBytes(6).toString("hex"),
    address = `${prefix}@example.invalid`;
  let creates = 0,
    failCreate = true,
    request: TurboSMTPCreateRequest | undefined;
  const account: TurboSMTPSubaccount = {
    id: String(Number.parseInt(prefix, 16)),
    email: address,
    active: true,
    limit: 123,
    sent: 3,
    interval: "Monthly",
  };
  const calls: string[] = [];
  const deps = {
    provider: async () => ({
      createSubaccount: async (body: TurboSMTPCreateRequest) => {
        creates++;
        request = body;
        if (failCreate) throw new TurboSMTPError("timeout");
        return account;
      },
      listSubaccounts: async () => ({ count: 1, results: [account] }),
      getSubaccount: async (id: string | number) => {
        calls.push(String(id));
        return account;
      },
      getSubaccountPlan: async (id: string | number) => {
        calls.push(String(id));
        return account;
      },
      setSubaccountActive: async (id: string | number, active: boolean) => {
        calls.push(String(id));
        return { ...account, active };
      },
      setSubaccountLimit: async (id: string | number, limit: number) => {
        calls.push(String(id));
        return { ...account, limit };
      },
    }),
  };
  const provision = {
    action: "provision",
    email: address,
    firstName: "Jane",
    lastName: "Doe",
    policyAgree: "yes",
  };
  try {
    await setSettings(false);
    assert.equal((await getTenantMail(admin.headers, one)).enabled, false);
    await assert.rejects(
      manageTenantMail(root.headers, one, provision, deps),
      /disabled/,
    );
    await setSettings(true);
    await assert.rejects(getTenantMail(admin.headers, two));
    await assert.rejects(getTenantMail(pending.headers, one));
    for (const action of ["provision", "link", "status", "quota"])
      await assert.rejects(
        manageTenantMail(admin.headers, one, { ...provision, action }, deps),
        /operator/,
      );
    for (const status of [400, 401, 403]) {
      const rejected = {
        provider: async () => ({
          ...(await deps.provider()),
          createSubaccount: async () => {
            throw new TurboSMTPError("upstream", status);
          },
        }),
      };
      await assert.rejects(
        manageTenantMail(root.headers, one, provision, rejected),
        /provider rejected/,
      );
      assert.equal((await getTenantMail(root.headers, one)).account, null);
    }
    await assert.rejects(
      manageTenantMail(root.headers, one, provision, deps),
      /review/,
    );
    assert.equal(creates, 1);
    assert.equal(request!.ip, settings.mailSendingIP);
    assert.equal(request!.policy_agree, true);
    assert.equal(request!.first_name, "Jane");
    assert.equal(request!.confirm_password, request!.password);
    const reserved = (
      await database.query(
        "SELECT * FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
        [one],
      )
    ).rows[0];
    await assert.rejects(
      manageTenantMail(admin.headers, one, { action: "refresh" }, deps),
    );
    assert.equal(reserved.state, "needs_review");
    assert.ok(!JSON.stringify(reserved).includes(request!.password));
    assert.equal(
      decryptMailSecret(reserved.encrypted_password, one),
      request!.password,
    );
    failCreate = false;
    await assert.rejects(
      manageTenantMail(root.headers, one, provision, deps),
      /already reserved/,
    );
    assert.equal(creates, 1);
    await assert.rejects(
      manageTenantMail(
        root.headers,
        one,
        {
          action: "link",
          providerID: account.id,
          email: "wrong@example.invalid",
          confirm: "yes",
        },
        deps,
      ),
    );
    await database.query(
      "UPDATE webdock_auth.mail_tenant_account SET state='provisioning' WHERE customer_id=$1",
      [one],
    );
    await assert.rejects(
      manageTenantMail(
        root.headers,
        one,
        {
          action: "link",
          providerID: account.id,
          email: address,
          confirm: "yes",
        },
        deps,
      ),
    );
    await database.query(
      "UPDATE webdock_auth.mail_tenant_account SET created_at=now()-interval '11 minutes' WHERE customer_id=$1",
      [one],
    );
    assert.equal(
      (await getTenantMail(admin.headers, one)).account!.state,
      "needs_review",
    );
    await manageTenantMail(
      root.headers,
      one,
      {
        action: "link",
        providerID: account.id,
        email: address,
        confirm: "yes",
      },
      deps,
    );
    await assert.rejects(
      manageTenantMail(
        root.headers,
        two,
        {
          action: "link",
          providerID: account.id,
          email: address,
          confirm: "yes",
        },
        deps,
      ),
    );
    await manageTenantMail(
      admin.headers,
      one,
      { action: "refresh", providerID: "123" },
      deps,
    );
    await manageTenantMail(
      root.headers,
      one,
      { action: "quota", limit: "44", providerID: "123" },
      deps,
    );
    await manageTenantMail(
      root.headers,
      one,
      { action: "status", active: "no", providerID: "123" },
      deps,
    );
    assert.ok(calls.every((id) => id === account.id));
    assert.equal(
      (await getTenantMail(admin.headers, one)).account!.active,
      false,
    );
    const domain = `${prefix}.example.com`;
    await manageTenantMail(admin.headers, one, {
      action: "add-domain",
      domain,
    });
    const entry = (await getTenantMail(admin.headers, one)).domains[0];
    const dns =
      (token: string): typeof fetch =>
      async (url, options) => {
        const parsed = new URL(String(url));
        assert.equal(parsed.origin, "https://cloudflare-dns.com");
        assert.equal(options?.redirect, "error");
        return Response.json({
          Status: 0,
          Answer: [
            {
              type: 16,
              name: `_webdock-mail.${domain}.`,
              data: `"webdock=${token}"`,
            },
          ],
        });
      };
    await assert.rejects(
      manageTenantMail(
        other.headers,
        two,
        { action: "verify-domain", domainID: entry.id },
        { dnsFetch: dns(entry.token) },
      ),
    );
    await assert.rejects(
      manageTenantMail(
        admin.headers,
        one,
        { action: "verify-domain", domainID: entry.id },
        { dnsFetch: dns("wrong") },
      ),
      /does not match/,
    );
    await manageTenantMail(
      admin.headers,
      one,
      { action: "verify-domain", domainID: entry.id },
      { dnsFetch: dns(entry.token) },
    );
    const verified = (await getTenantMail(admin.headers, one)).domains[0];
    assert.equal(verified.status, "ownership_verified");
    assert.equal("providerStatus" in verified, false);
    await manageTenantMail(other.headers, two, {
      action: "add-domain",
      domain,
    });
    const second = (await getTenantMail(other.headers, two)).domains[0];
    await assert.rejects(
      manageTenantMail(
        other.headers,
        two,
        { action: "verify-domain", domainID: second.id },
        { dnsFetch: dns(second.token) },
      ),
      /elsewhere/,
    );
    await assert.rejects(
      manageTenantMail(other.headers, two, {
        action: "remove-domain",
        domainID: entry.id,
        confirm: "yes",
      }),
    );
    await assert.rejects(
      manageTenantMail(admin.headers, one, {
        action: "remove-domain",
        domainID: entry.id,
      }),
    );
    await database.query(
      "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"=$1",
      [admin.id],
    );
    await assert.rejects(
      manageTenantMail(admin.headers, one, {
        action: "remove-domain",
        domainID: entry.id,
        confirm: "yes",
      }),
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [one],
    );
    await assert.rejects(
      manageTenantMail(root.headers, one, { action: "refresh" }, deps),
    );
    await assert.rejects(getTenantMail(admin.headers, one));
    await database.query(
      'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
      [other.id],
    );
    await assert.rejects(getTenantMail(other.headers, two));
    const successAddress = `success-${address}`;
    const successAccount = {
      ...account,
      id: String(Number(account.id) + 1),
      email: successAddress,
    };
    let requestedLimit: number | undefined;
    const successful = {
      provider: async () => ({
        ...(await deps.provider()),
        createSubaccount: async () => successAccount,
        setSubaccountLimit: async (id: string | number, limit: number) => {
          assert.equal(id, successAccount.id);
          requestedLimit = limit;
          return successAccount;
        },
      }),
    };
    const failedQuota = {
      provider: async () => ({
        ...(await successful.provider()),
        setSubaccountLimit: async () => {
          throw new TurboSMTPError("upstream", 401);
        },
      }),
    };
    await assert.rejects(
      manageTenantMail(
        root.headers,
        two,
        { ...provision, email: successAddress },
        failedQuota,
      ),
      /review/,
    );
    await database.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [other.id],
    );
    await assert.rejects(
      manageTenantMail(other.headers, two, { action: "refresh" }, successful),
      /reconcile/,
    );
    await assert.rejects(
      manageTenantMail(
        root.headers,
        two,
        { action: "quota", limit: "123" },
        successful,
      ),
      /reconcile/,
    );
    const reconcile = {
      provider: async () => ({
        ...(await successful.provider()),
        listSubaccounts: async () => ({ count: 1, results: [successAccount] }),
        getSubaccount: async () => successAccount,
      }),
    };
    await manageTenantMail(
      root.headers,
      two,
      {
        action: "link",
        providerID: successAccount.id,
        email: successAddress,
        confirm: "yes",
      },
      reconcile,
    );
    assert.equal(requestedLimit, settings.mailDefaultLimit);
    assert.equal(
      (await getTenantMail(root.headers, two)).account!.state,
      "ready",
    );
    const third = (
      await database.query(
        "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mail Retry') RETURNING id",
      )
    ).rows[0].id;
    const retryAccount = {
      ...successAccount,
      id: String(Number(account.id) + 2),
      email: `retry-${address}`,
    };
    let attempts = 0;
    const retry = {
      provider: async () => ({
        ...(await deps.provider()),
        createSubaccount: async () => {
          attempts++;
          if (attempts === 1) throw new TurboSMTPError("upstream", 401);
          return retryAccount;
        },
        setSubaccountLimit: async () => retryAccount,
      }),
    };
    await assert.rejects(
      manageTenantMail(
        root.headers,
        third,
        { ...provision, email: retryAccount.email },
        retry,
      ),
      /provider rejected/,
    );
    assert.equal(attempts, 1, "no automatic retry");
    await manageTenantMail(
      root.headers,
      third,
      { ...provision, email: retryAccount.email },
      retry,
    );
    assert.equal(attempts, 2);
    assert.equal(
      (await getTenantMail(root.headers, third)).account!.state,
      "ready",
    );
    assert.ok(
      (
        await database.query(
          "SELECT id FROM webdock_auth.access_event WHERE target_id=$1 AND action LIKE 'mail-%'",
          [one],
        )
      ).rowCount! > 5,
    );
  } finally {
    if (previous)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1,updated_by=$2,updated_at=$3 WHERE id=true",
        [previous.settings, previous.updated_by, previous.updated_at],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});
