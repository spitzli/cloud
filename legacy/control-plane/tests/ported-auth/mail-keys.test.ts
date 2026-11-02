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
import {
  getTenantMailKeys,
  manageTenantMailKeys,
} from "../../src/modules/mail/mail-keys";
import {
  TurboSMTPError,
  type TurboSMTPKeyOptions,
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
test("tenant keys isolate provider child authorization, roles, scopes and one-time secrets", async () => {
  await database.query(tenantSchemaSQL);
  await database.query(accessSchemaSQL);
  await database.query(platformSchemaSQL);
  await database.query(mailSchemaSQL);
  const previous = (
    await database.query(
      "SELECT * FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const admin = await identity(),
    other = await identity(),
    member = await identity();
  const rows = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Key A'),(webdock_auth.next_snowflake(),'Key B') RETURNING id",
    )
  ).rows;
  const [one, two] = rows.map((row) => row.id);
  const mappings = (
    await database.query(
      "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [[one, two]],
    )
  ).rows;
  const org = (id: string) =>
    mappings.find((row) => row.customer_id === id)!.organization_id;
  await database.query(
    "INSERT INTO webdock_auth.member(\"organizationId\",\"userId\",role,\"createdAt\") VALUES($1,$2,'admin',now()),($3,$4,'admin',now()),($1,$5,'member',now())",
    [org(one), admin.id, org(two), other.id, member.id],
  );
  const address = `${randomBytes(8).toString("hex")}@example.invalid`,
    id = String(Number.parseInt(randomBytes(6).toString("hex"), 16));
  await database.query(
    "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state) VALUES($1,$2,$3,'ready')",
    [one, id, address],
  );
  const setEnabled = (enabled: boolean) =>
    database.query(
      "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET settings=$1",
      [JSON.stringify({ mailEnabled: enabled })],
    );
  let active = true,
    creates = 0,
    deletes = 0;
  const metadata = {
    consumerKey: "own-key",
    label: "App",
    creation_time: "2026-10-04 12:00:00",
    ips: [],
    is_legacy: false,
    permissions: ["SEND_SMTP"],
  };
  const deps = {
    provider: async () => ({
      getSubaccount: async (requested: string | number) => {
        assert.equal(requested, id);
        return { id, email: address, active };
      },
      authorizeSubaccount: async (email: string) => {
        assert.equal(email, address);
        return "child-token";
      },
    }),
    childProvider: (token: string) => {
      assert.equal(token, "child-token");
      return {
        listConsumerKeys: async () => ({
          count: 1,
          results: [{ ...metadata, consumerSecret: "never-return" }],
        }),
        createConsumerKey: async (
          auth: string,
          label: string,
          options?: TurboSMTPKeyOptions,
        ) => {
          assert.equal(auth, "child-token");
          assert.equal(label, "Website");
          assert.deepEqual(options?.permissions, ["SEND_SMTP", "SEND_API"]);
          creates++;
          return { consumerKey: "new-key", consumerSecret: "one-time-secret" };
        },
        deleteConsumerKey: async (key: string) => {
          assert.equal(key, "own-key");
          deletes++;
        },
      };
    },
  };
  const create = {
      action: "create",
      label: "Website",
      permissions: ["SEND_SMTP", "SEND_API"],
    },
    revoke = { action: "revoke", consumerKey: "own-key", confirm: "yes" };
  try {
    await setEnabled(true);
    for (const who of [other, member]) {
      await assert.rejects(getTenantMailKeys(who.headers, one, deps));
      await assert.rejects(
        manageTenantMailKeys(who.headers, one, create, deps),
      );
    }
    await assert.rejects(getTenantMailKeys(admin.headers, two, deps));
    await assert.rejects(
      manageTenantMailKeys(
        admin.headers,
        one,
        { ...create, permissions: ["APIS"] },
        deps,
      ),
    );
    await assert.rejects(
      manageTenantMailKeys(
        admin.headers,
        one,
        { ...create, ips: ["192.0.2.0/24"] },
        deps,
      ),
    );
    await assert.rejects(
      manageTenantMailKeys(
        admin.headers,
        one,
        { ...revoke, consumerKey: "another-tenant-key" },
        deps,
      ),
    );
    assert.equal(deletes, 0);
    const created = await manageTenantMailKeys(
      admin.headers,
      one,
      create,
      deps,
    );
    assert.equal(created.created?.consumerSecret, "one-time-secret");
    assert.equal(creates, 1);
    assert.ok(
      !JSON.stringify(
        await getTenantMailKeys(admin.headers, one, deps),
      ).includes("never-return"),
    );
    const stored = (
      await database.query(
        "SELECT row_to_json(a) AS account FROM webdock_auth.mail_tenant_account a WHERE customer_id=$1",
        [one],
      )
    ).rows;
    const events = (
      await database.query(
        "SELECT * FROM webdock_auth.access_event WHERE target_id=$1",
        [one],
      )
    ).rows;
    assert.ok(!JSON.stringify([stored, events]).includes("one-time-secret"));
    assert.ok(!JSON.stringify(events).includes("own-key"));
    active = false;
    await assert.rejects(
      manageTenantMailKeys(admin.headers, one, create, deps),
      /paused/,
    );
    await setEnabled(false);
    await assert.rejects(
      manageTenantMailKeys(admin.headers, one, create, deps),
      /disabled/,
    );
    assert.equal(
      (await getTenantMailKeys(admin.headers, one, deps))!.keys.length,
      1,
    );
    await manageTenantMailKeys(admin.headers, one, revoke, deps);
    assert.equal(deletes, 1);
    await database.query(
      'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
      [admin.id],
    );
    await assert.rejects(getTenantMailKeys(admin.headers, one, deps));
    await assert.rejects(
      manageTenantMailKeys(admin.headers, one, revoke, deps),
    );
    await database.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [admin.id],
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [one],
    );
    await assert.rejects(
      manageTenantMailKeys(admin.headers, one, revoke, deps),
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
      [one],
    );
    const bad = {
      ...deps,
      provider: async () => ({
        ...(await deps.provider()),
        getSubaccount: async () => ({
          id,
          email: "wrong@example.invalid",
          active: true,
        }),
      }),
    };
    await assert.rejects(
      getTenantMailKeys(admin.headers, one, bad),
      /verified/,
    );
    const fail = {
      ...deps,
      provider: async () => ({
        ...(await deps.provider()),
        authorizeSubaccount: async () => {
          throw new TurboSMTPError("upstream", 401);
        },
      }),
    };
    await assert.rejects(
      getTenantMailKeys(admin.headers, one, fail),
      /Could not load/,
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
