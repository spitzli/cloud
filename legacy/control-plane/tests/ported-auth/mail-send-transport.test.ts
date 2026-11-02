import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { database } from "../../src/db/runtime";
import { auth } from "../../src/modules/auth/auth";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { configureNativeTransport } from "../../src/modules/mail/mail-send-transport";
import { mailSendSchemaSQL } from "../../src/modules/mail/mail-send-schema";
import { transaction } from "../../src/modules/hosting/db";
import { manageNativeMailKey } from "../../src/modules/mail/mail-native-keys";
const url = new URL(process.env.DATABASE_URL!);
if (
  url.hostname !== "127.0.0.1" ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(async () => {
  await auth.$context;
  await database.end();
});
test("managed relay setup keeps provider keys private, is idempotent and fences unknown creation", async () => {
  await database.query(mailSendSchemaSQL);
  const before = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const previous = process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED;
  process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED = "true";
  process.env.MAIL_ENCRYPTION_KEY ??= randomBytes(32).toString("base64");
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `send-${randomBytes(8).toString("hex")}@example.invalid`,
    name: "Mail setup fixture",
    password,
    operator: true,
    mustChangePassword: false,
  });
  const login = await auth.api.signInEmail({
    body: { email: user.email, password },
    asResponse: true,
  });
  const headers = new Headers({
    Cookie: login.headers
      .getSetCookie()
      .map((value) => value.split(";")[0])
      .join("; "),
  });
  const session = await auth.api.getSession({ headers });
  assert.ok(session);
  await database.query(
    'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
    [user.id],
  );
  const actor = { subject: user.id, sessionID: session.session.id };
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Transport fixture') RETURNING id",
    )
  ).rows[0].id;
  try {
    await database.query(
      `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"mailEnabled":true}') ON CONFLICT(id) DO UPDATE SET settings=jsonb_set(platform_settings.settings,'{mailEnabled}','true')`,
    );
    await database.query(
      `INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$1,$2,'ready','{"active":true}')`,
      [customer, customer + "@example.test"],
    );
    let creates = 0;
    const providers = {
      master: async () => ({
        getSubaccount: async () => ({
          id: customer,
          email: customer + "@example.test",
          active: true,
        }),
        authorizeSubaccount: async () => "child-only-token",
      }),
      child: () => ({
        createConsumerKey: async (
          token: string,
          _label: string,
          options: unknown,
        ) => {
          creates++;
          assert.equal(token, "child-only-token");
          assert.deepEqual(options, {
            permissions: ["SEND_API", "SEND_SMTP"],
            ips: [],
          });
          return {
            consumerKey: "private-key",
            consumerSecret: "private-secret",
          };
        },
      }),
    };
    assert.deepEqual(
      await configureNativeTransport(actor, customer, "managed", providers),
      { ready: true },
    );
    await configureNativeTransport(actor, customer, "managed", providers);
    assert.equal(creates, 1);
    const secret = (
      await database.query(
        "SELECT encrypted_credentials FROM webdock_mail.send_transport WHERE customer_id=$1",
        [customer],
      )
    ).rows[0].encrypted_credentials;
    assert.ok(secret);
    assert.equal(secret.includes("private-secret"), false);
    const created = await transaction((db) =>
      manageNativeMailKey(db, actor, customer, {
        action: "create",
        label: "App",
      }),
    );
    assert.match(created.token!, /^wd_mail_/);
    const stored = (
      await database.query(
        "SELECT token_hash FROM webdock_mail.api_key WHERE id=$1",
        [created.id],
      )
    ).rows[0];
    assert.notEqual(stored.token_hash, created.token);
    await transaction((db) =>
      manageNativeMailKey(db, actor, customer, {
        action: "revoke",
        keyID: created.id,
      }),
    );
    await database.query(
      "UPDATE webdock_mail.send_transport SET state='disabled' WHERE customer_id=$1",
      [customer],
    );
    const unavailable = {
      ...providers,
      child: () => ({
        createConsumerKey: async () => {
          creates++;
          throw Error("lost provider response");
        },
      }),
    };
    await assert.rejects(
      configureNativeTransport(actor, customer, "managed", unavailable),
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.send_transport WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].state,
      "needs_review",
    );
    await assert.rejects(
      configureNativeTransport(actor, customer, "managed", providers),
    );
    assert.equal(creates, 2);
  } finally {
    if (previous === undefined)
      delete process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED;
    else process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED = previous;
    await database.query(
      "DELETE FROM webdock_mail.api_key WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_mail.send_transport WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
      [customer],
    );
    const mapping = (
      await database.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id",
        [customer],
      )
    ).rows[0];
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customer,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      mapping.organization_id,
    ]);
    await database.query(
      "DELETE FROM webdock_auth.access_event WHERE actor_id=$1",
      [user.id],
    );
    await database.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
      user.id,
    ]);
    if (before)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [before.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});
