import assert from "node:assert/strict";
import test from "node:test";
import { database } from "../../src/db/runtime";
import {
  platformSchemaSQL,
  validatePlatformSettings,
  encryptMailSecret,
  decryptMailSecret,
  savePlatformSettings,
  getPlatformSettings,
  saveMailCredentials,
  getMailConnectionStatus,
  clearMailCredentials,
  mailProvider,
} from "../../src/modules/platform/platform";

const input = {
  name: "Webdock QA",
  supportEmail: "support@example.invalid",
  cmsEnabled: "yes",
  mailEnabled: "yes",
  mailSendingIP: "203.0.113.5",
  mailDefaultLimit: "1000",
  mailRegion: "eu",
};
test.after(() => database.end());
test("mail gateway controls default off and can be set independently", () => {
  const defaults = validatePlatformSettings(input);
  assert.equal(defaults.mailGatewayEnabled, false);
  assert.equal(defaults.mailDispatchEnabled, false);
  assert.equal(defaults.mailWebhooksEnabled, false);
  const enabled = validatePlatformSettings({
    ...input,
    mailGatewayEnabled: "yes",
    mailDispatchEnabled: "",
    mailWebhooksEnabled: "yes",
  });
  assert.equal(enabled.mailGatewayEnabled, true);
  assert.equal(enabled.mailDispatchEnabled, false);
  assert.equal(enabled.mailWebhooksEnabled, true);
});
test("platform configuration validates actual settings and encrypted secrets bind their purpose", () => {
  assert.equal(validatePlatformSettings(input).mailEnabled, true);
  for (const override of [
    { name: "" },
    { supportEmail: "bad\naddress" },
    { mailSendingIP: "localhost" },
    { mailDefaultLimit: "-1" },
    { mailDefaultLimit: "1e3" },
    { mailRegion: "unknown" },
  ])
    assert.throws(() => validatePlatformSettings({ ...input, ...override }));
  const secret = "a sufficiently long key just for the fixture";
  const sealed = encryptMailSecret(
    { credential: "fixture-private" },
    "tenant:123",
    secret,
  );
  assert.ok(!sealed.includes("fixture-private"));
  assert.deepEqual(decryptMailSecret(sealed, "tenant:123", secret), {
    credential: "fixture-private",
  });
  assert.throws(() => decryptMailSecret(sealed, "tenant:456", secret));
  assert.throws(() =>
    decryptMailSecret(sealed, "tenant:123", secret + "wrong"),
  );
  const parts = sealed.split(".");
  parts[3] = (parts[3][0] === "a" ? "b" : "a") + parts[3].slice(1);
  assert.throws(() => decryptMailSecret(parts.join("."), "tenant:123", secret));
});

test("only current root operators can save settings and credentials; status never reveals secrets", async () => {
  const url = new URL(process.env.DATABASE_URL!);
  assert.ok(["localhost", "127.0.0.1"].includes(url.hostname));
  assert.equal(url.pathname, "/webdock_api_test");
  await database.query(platformSchemaSQL);
  const create = async (role: string) =>
    (
      await database.query(
        'INSERT INTO webdock_auth."user"(name,email,role,"emailVerified","twoFactorEnabled","mustChangePassword") VALUES($1,$2,$3,true,true,false) RETURNING id',
        [
          "Platform fixture",
          `platform-${role}-${Date.now()}@example.invalid`,
          role,
        ],
      )
    ).rows[0].id;
  const root = await create("operator"),
    customer = await create("user");
  try {
    await assert.rejects(savePlatformSettings(customer, input));
    await assert.rejects(
      saveMailCredentials(customer, "fixture-key", "fixture-secret"),
    );
    await savePlatformSettings(root, input);
    assert.equal((await getPlatformSettings()).name, "Webdock QA");
    await saveMailCredentials(
      root,
      "fixture-master-key-1234",
      "fixture-master-secret",
    );
    const status = await getMailConnectionStatus();
    assert.deepEqual(status, { configured: true, consumerKeySuffix: "1234" });
    assert.ok(!JSON.stringify(status).includes("fixture-master"));
    const row = (
      await database.query(
        "SELECT encrypted_secret FROM webdock_auth.platform_mail_credential WHERE id=true",
      )
    ).rows[0];
    assert.ok(!row.encrypted_secret.includes("fixture-master"));
    await mailProvider(); // Construction does not send a provider request.
    await database.query(
      'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
      [root],
    );
    await assert.rejects(clearMailCredentials(root));
    await database.query(
      'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
      [root],
    );
    await clearMailCredentials(root);
    assert.equal((await getMailConnectionStatus()).configured, false);
    assert.ok(
      (
        await database.query(
          "SELECT 1 FROM webdock_auth.access_event WHERE actor_id=$1 AND action='platform-settings' AND outcome='succeeded'",
          [root],
        )
      ).rowCount,
    );
  } finally {
    await database.query(
      "DELETE FROM webdock_auth.platform_mail_credential WHERE updated_by=$1",
      [root],
    );
    await database.query(
      "DELETE FROM webdock_auth.platform_settings WHERE updated_by=$1",
      [root],
    );
    await database.query(
      'DELETE FROM webdock_auth."user" WHERE id=ANY($1::text[])',
      [[root, customer]],
    );
  }
});
