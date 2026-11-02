import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { Pool } from "pg";
import { nativeMailSchemaSQL } from "@webdock/mail-core/schema";
import { getMailService, requestMailService } from "@webdock/mail-core";
import { openSecret } from "@webdock/mail-core/secrets";
import { processMailOperation } from "../../src/provisioning.ts";

const url = new URL(process.env.DATABASE_URL!);
if (!["localhost", "127.0.0.1"].includes(url.hostname) || url.pathname !== "/webdock_admin_test" || process.env.AUTH_TEST_MAIL !== "true") throw new Error("Only disposable local Mail test databases are allowed");
const pool = new Pool({ connectionString: url.toString(), max: 4 });
test.after(() => pool.end());

test("worker persists credentials before external work, retains data on suspension and never retries unknown provisioning", async () => {
  await pool.query(nativeMailSchemaSQL);
  const key = randomBytes(32).toString("base64");
  const hostID = `worker-${randomBytes(6).toString("hex")}`;
  const actor = (await pool.query('INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"mustChangePassword","twoFactorEnabled") VALUES(\'Worker fixture\',$1,true,\'operator\',false,true) RETURNING id', [`${hostID}@example.invalid`])).rows[0].id;
  const customer = (await pool.query("INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_admin.next_snowflake(),'Worker fixture') RETURNING id")).rows[0].id;
  const previous = (await pool.query("SELECT settings FROM webdock_auth.platform_settings WHERE id=true")).rows[0];
  let fail = false;
  const runtime: Parameters<typeof processMailOperation>[1] = {
    provision: async (operation, _credentials, save) => {
      const record = (await pool.query("SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id=$1", [operation.customerID])).rows[0];
      assert.ok(record, "a bootstrap credential must survive a process crash");
      if (fail) throw new Error("Private runtime details must not escape");
      assert.ok(openSecret<{ bootstrapPassword: string }>(record.encrypted_credentials, `instance:${customer}`, key).bootstrapPassword);
      await save({ username: "admin", password: "fixture-permanent-secret" });
      return { internalURL: "http://172.31.0.2:8080", publicURL: "https://mail-fixture.example.invalid" };
    },
    suspend: async () => {},
  };
  const request = (enabled: boolean, expectedRevision: string) => requestMailService(pool, { customerID: customer, enabled, expectedRevision, actorID: actor, hostID });
  try {
    await pool.query("INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{\"mailEnabled\":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings");
    assert.equal(await processMailOperation(pool, runtime, hostID, key), false);
    await request(true, "0");
    assert.equal(await processMailOperation(pool, runtime, hostID, key), true);
    assert.equal((await getMailService(pool, customer)).state, "ready");
    const record = (await pool.query("SELECT * FROM webdock_mail.instance WHERE customer_id=$1", [customer])).rows[0];
    assert.ok(!JSON.stringify(record).includes("fixture-permanent-secret"));
    assert.deepEqual(openSecret(record.encrypted_credentials, `instance:${customer}`, key), { username: "admin", password: "fixture-permanent-secret" });
    await request(false, "1");
    await processMailOperation(pool, runtime, hostID, key);
    assert.equal((await getMailService(pool, customer)).state, "suspended");
    assert.equal((await pool.query("SELECT count(*)::int AS n FROM webdock_mail.instance WHERE customer_id=$1", [customer])).rows[0].n, 1);
    // Force an unknown resume outcome; persistence and review behavior must not depend on the runtime error text.
    fail = true;
    await request(true, "2");
    await processMailOperation(pool, runtime, hostID, key);
    assert.equal((await getMailService(pool, customer)).state, "needs_review");
    assert.equal(await processMailOperation(pool, runtime, hostID, key), false);
  } finally {
    await pool.query("DELETE FROM webdock_mail.instance WHERE customer_id=$1", [customer]);
    await pool.query("DELETE FROM webdock_mail.operation WHERE customer_id=$1", [customer]);
    await pool.query("DELETE FROM webdock_mail.service WHERE customer_id=$1", [customer]);
    await pool.query('DELETE FROM webdock_auth."user" WHERE id=$1', [actor]);
    const mapping = (await pool.query("DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id", [customer])).rows[0];
    await pool.query("DELETE FROM webdock_admin.customers WHERE id=$1", [customer]);
    await pool.query("DELETE FROM webdock_auth.organization WHERE id=$1", [mapping.organization_id]);
    if (previous) await pool.query("UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true", [previous.settings]);
    else await pool.query("DELETE FROM webdock_auth.platform_settings WHERE id=true");
  }
});
