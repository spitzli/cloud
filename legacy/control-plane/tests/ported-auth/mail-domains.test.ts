import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { tenantSchemaSQL } from "../../src/modules/access/tenant-schema";
import { accessSchemaSQL } from "../../src/modules/access/access-management";
import { platformSchemaSQL } from "../../src/modules/platform/platform";
import { mailSchemaSQL } from "../../src/modules/mail/tenant-mail";
import { TurboSMTPError } from "../../src/modules/mail/turbosmtp";
import { trackingSchemaSQL } from "../../src/modules/mail/mail-tracking";
import {
  senderDomainSchemaSQL,
  getTenantSenderDomains,
  manageTenantSenderDomain,
} from "../../src/modules/mail/mail-domains";
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
test("Sender-domain writes are tenant-bound, child-only and fail closed", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(platformSchemaSQL);
  await database.query(mailSchemaSQL);
  await database.query(senderDomainSchemaSQL);
  await database.query(trackingSchemaSQL);
  const previous = (
    await database.query(
      "SELECT * FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const root = await identity(true),
    admin = await identity(),
    other = await identity();
  const [one, two] = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Domain A'),(webdock_auth.next_snowflake(),'Domain B') RETURNING id",
    )
  ).rows.map((t) => t.id);
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
  const prefix = randomBytes(6).toString("hex"),
    domain = `${prefix}.example.com`,
    email = `${prefix}@example.com`,
    providerID = String(Number.parseInt(prefix, 16));
  await database.query(
    "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state) VALUES($1,$2,$3,'ready')",
    [one, providerID, email],
  );
  const local = (
    await database.query(
      "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token) VALUES($1,$2,'proof') RETURNING id",
      [one, domain],
    )
  ).rows[0].id;
  const input = { action: "register" as const, domainID: local },
    result = {
      id: "domain-provider",
      domain,
      spf_verified: true,
      dkim_verified: true,
      dmarc_verified: true,
    };
  let calls = 0,
    active = true,
    mismatch = false,
    listed = [result],
    failPost = false;
  const deps = {
    provider: async () => ({
      getSubaccount: async (id: string | number) => {
        assert.equal(id, providerID);
        return {
          id: providerID,
          email: mismatch ? "wrong@example.com" : email,
          active,
        };
      },
      authorizeSubaccount: async (address: string) => {
        assert.equal(address, email);
        return "child-only";
      },
    }),
    childProvider: (token: string) => {
      assert.equal(token, "child-only");
      return {
        registerSenderDomain: async (name: string) => {
          calls++;
          assert.equal(name, domain);
          if (failPost) throw new TurboSMTPError("timeout");
          return result;
        },
        deleteSenderDomain: async (id: string) => {
          assert.equal(id, result.id);
          listed = [];
        },
        listSenderDomains: async () => listed,
      };
    },
    dnsFetch: (async () => Response.json({ Status: 0 })) as typeof fetch,
  };
  const enabled = (value: boolean) =>
    database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET settings=$1",
      [JSON.stringify({ mailEnabled: value })],
    );
  try {
    await enabled(true);
    await assert.rejects(
      manageTenantSenderDomain(other.headers, two, input, deps),
      /Domain not found/,
    );
    await assert.rejects(getTenantSenderDomains(other.headers, one, deps));
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /ownership/,
    );
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='ownership_verified' WHERE id=$1",
      [local],
    );
    await enabled(false);
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /disabled/,
    );
    await enabled(true);
    active = false;
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /paused/,
    );
    active = true;
    mismatch = true;
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /could not be verified/,
    );
    mismatch = false;
    await database.query(
      "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"=$1",
      [admin.id],
    );
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /administrator/,
    );
    await database.query(
      "UPDATE webdock_auth.member SET role='admin' WHERE \"userId\"=$1",
      [admin.id],
    );
    assert.equal(calls, 0);
    await manageTenantSenderDomain(admin.headers, one, input, deps);
    assert.equal(
      (await getTenantSenderDomains(admin.headers, one, deps)).domains[0]
        .providerStatus,
      "verified",
    );
    assert.equal(
      (await getTenantSenderDomains(other.headers, two, deps)).domains.length,
      0,
    );
    const tracking = (
      await database.query(
        "INSERT INTO webdock_auth.mail_tracking_domain(customer_id,sender_domain_id,domain) VALUES($1,$2,$3) RETURNING id",
        [one, local, `links.${domain}`],
      )
    ).rows[0];
    await assert.rejects(
      manageTenantSenderDomain(
        admin.headers,
        one,
        { ...input, action: "unregister", confirm: "yes" },
        deps,
      ),
      /associated tracking domains/,
    );
    assert.equal(
      (
        await database.query(
          "SELECT provider_operation FROM webdock_auth.mail_sender_domain WHERE id=$1",
          [local],
        )
      ).rows[0].provider_operation,
      null,
    );
    await database.query(
      "DELETE FROM webdock_auth.mail_tracking_domain WHERE id=$1",
      [tracking.id],
    );
    await assert.rejects(
      manageTenantSenderDomain(
        admin.headers,
        one,
        { ...input, action: "unregister" },
        deps,
      ),
      /Confirm/,
    );
    listed = [{ ...result, id: "different-id" }];
    await assert.rejects(
      manageTenantSenderDomain(
        admin.headers,
        one,
        { ...input, action: "unregister", confirm: "yes" },
        deps,
      ),
      /exact mapped/,
    );
    listed = [result];
    const failedDelete = {
      ...deps,
      childProvider: (token: string) => ({
        ...deps.childProvider(token),
        deleteSenderDomain: async () => {},
      }),
    };
    await assert.rejects(
      manageTenantSenderDomain(
        admin.headers,
        one,
        { ...input, action: "unregister", confirm: "yes" },
        failedDelete,
      ),
      /not confirmed/,
    );
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='pending' WHERE id=$1",
      [local],
    );
    await manageTenantSenderDomain(
      admin.headers,
      one,
      { ...input, action: "unregister", confirm: "yes" },
      deps,
    );
    const removed = (await getTenantSenderDomains(admin.headers, one, deps))
      .domains[0];
    assert.equal(removed.providerStatus, "not_registered");
    assert.equal(removed.status, "pending");
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='ownership_verified' WHERE id=$1",
      [local],
    );
    listed = [result];
    await manageTenantSenderDomain(admin.headers, one, input, deps);
    listed = [{ ...result, domain: "unrelated.example.com" }];
    await assert.rejects(
      manageTenantSenderDomain(
        admin.headers,
        one,
        { ...input, action: "refresh" },
        deps,
      ),
      /exact sender domain/,
    );
    let view = await getTenantSenderDomains(admin.headers, one, deps);
    assert.equal(view.domains[0].providerStatus, "pending");
    assert.equal(view.domains[0].spfVerified, undefined);
    listed = [result];
    failPost = true;
    const before = calls;
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /could not be confirmed/,
    );
    assert.equal(calls, before + 1);
    view = await getTenantSenderDomains(admin.headers, one, deps);
    assert.equal(view.domains[0].providerStatus, "pending");
    const parallelRows = (
      await database.query(
        "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status) SELECT $1, name, 'proof','ownership_verified' FROM unnest($2::text[]) name RETURNING id,domain",
        [one, [0, 1, 2, 3].map((i) => `${i}.${domain}`)],
      )
    ).rows;
    const parallelResults = parallelRows.map((row) => ({
      ...result,
      id: `provider-${row.id}`,
      domain: row.domain,
    }));
    let arrived = 0,
      release: () => void = () => {};
    const barrier = new Promise<void>((resolve) => {
      release = resolve;
    });
    const parallelDeps = {
      ...deps,
      childProvider: (token: string) => {
        assert.equal(token, "child-only");
        return {
          deleteSenderDomain: async () => {},
          registerSenderDomain: async (name: string) => {
            if (++arrived === 4) release();
            await barrier;
            return parallelResults.find((row) => row.domain === name)!;
          },
          listSenderDomains: async () => parallelResults,
        };
      },
    };
    let deadline: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([
        Promise.all(
          parallelRows.map((row) =>
            manageTenantSenderDomain(
              admin.headers,
              one,
              { action: "register", domainID: row.id },
              parallelDeps,
            ),
          ),
        ),
        new Promise<never>((_, reject) => {
          deadline = setTimeout(() => {
            release();
            reject(Error("Concurrent domain operations deadlocked"));
          }, 3000);
        }),
      ]);
    } finally {
      clearTimeout(deadline);
    }
    assert.equal(arrived, 4);
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET provider_operation='another',provider_operation_started_at=now() WHERE id=$1",
      [local],
    );
    await assert.rejects(
      manageTenantSenderDomain(admin.headers, one, input, deps),
      /already running/,
    );
    await database.query(
      "UPDATE webdock_auth.mail_sender_domain SET provider_operation=NULL WHERE id=$1",
      [local],
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [one],
    );
    await assert.rejects(
      manageTenantSenderDomain(root.headers, one, input, deps),
      /administrator/,
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
