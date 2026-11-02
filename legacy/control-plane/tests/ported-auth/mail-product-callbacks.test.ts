import assert from "node:assert/strict";
import test from "node:test";
import { createHash, randomBytes, randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { acceptProviderEvents, parseProviderEvents, processCallbackInbox } =
  await import("../../src/modules/mail/product/callback-inbox");
const { submitMail } = await import("../../src/modules/mail/product/sending");
const { encryptMailSecret } =
  await import("../../src/modules/platform/platform");
test.after(() => database.end());
test("callback IDs are lossless and documented status/timestamp aliases are normalized", () => {
  const [event] = parseProviderEvents(
    '{"mid":9007199254740993123,"id":9007199254740993124,"status":"REPORT","email":"a@example.com","Timestamp":1791806400}',
  );
  assert.equal(event.mid, "9007199254740993123");
  assert.equal(event.providerEventID, "9007199254740993124");
  assert.equal(event.type, "email.complained");
  assert.equal(
    parseProviderEvents(
      '{"mid":"123","status":"UNSUBSCRIBED","email":"a@example.com","timestamp":1791806400}',
    )[0].type,
    "email.unsubscribed",
  );
});
test("early callback is scoped, durable and authoritative over a later timeout", async () => {
  await withMailProductFixture(async (f) => {
    const secret = randomBytes(32).toString("base64url");
    const source = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_callback_source(secret_hash,kind,verified_at) VALUES($1,'account',now()) RETURNING id",
        [createHash("sha256").update(secret).digest("hex")],
      )
    ).rows[0].id;
    await f.db.query(
      "INSERT INTO webdock_auth.mail_callback_source_connection(source_id,connection_id) VALUES($1,$2)",
      [source, f.actorA.connectionID],
    );
    await f.db.query(
      "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready','{\"active\":true}')",
      [
        f.actorA.customerID,
        "fixture-" + f.actorA.customerID,
        f.suffix + "@example.invalid",
      ],
    );
    await f.db.query(
      "INSERT INTO webdock_auth.mail_connection_secret(customer_id,connection_id,version,encrypted_secret,key_suffix,active) VALUES($1,$2,1,$3,'test',true)",
      [
        f.actorA.customerID,
        f.actorA.connectionID,
        encryptMailSecret(
          { consumerKey: "fixture", consumerSecret: "fixture" },
          `mail-send:${f.actorA.customerID}:${f.actorA.connectionID}:1`,
        ),
      ],
    );
    const request = (body: unknown, auth = true) =>
      new Request(
        "https://auth.webdock.dev/api/mail/provider-events/" + source,
        {
          method: "POST",
          headers: {
            "content-type": "application/json",
            ...(auth
              ? {
                  authorization:
                    "Basic " +
                    Buffer.from("webdock:" + secret).toString("base64"),
                }
              : {}),
          },
          body: JSON.stringify(body),
        },
      );
    let payload: Record<string, unknown> = {};
    let calls = 0;
    try {
      assert.equal(
        (await acceptProviderEvents(source, request({}, false))).status,
        401,
      );
      const result = await submitMail(
        f.actorA,
        {
          from: "sender@" + f.domainA.name,
          to: "recipient@example.com",
          subject: "early",
          text: "test",
        },
        {
          key: "early",
          send: async (_credential, _body, reference) => {
            calls++;
            payload = {
              mid: "9007199254740993123",
              id: "42",
              reference_id: reference,
              email: "recipient@example.com",
              status: "DELIVERED",
              timestamp: Math.floor(Date.now() / 1000),
            };
            assert.equal(
              (await acceptProviderEvents(source, request(payload))).status,
              200,
            );
            assert.equal(
              (
                await processCallbackInbox({
                  limit: 20,
                  deadline: new Date(Date.now() + 5000),
                })
              ).processed,
              1,
            );
            return { kind: "unknown", code: "lost_response" };
          },
        },
      );
      assert.equal(result.status, 200);
      assert.equal(calls, 1);
      assert.equal(
        (
          await f.db.query(
            "SELECT consumed FROM webdock_auth.mail_usage_period WHERE customer_id=$1",
            [f.actorA.customerID],
          )
        ).rows[0].consumed,
        "1",
      );
      assert.equal(
        (
          await acceptProviderEvents(
            source,
            request([
              payload,
              { ...payload, id: "43", reference_id: randomUUID() },
              { bad: "raw-secret-not-stored" },
            ]),
          )
        ).status,
        200,
      );
      await processCallbackInbox({
        limit: 20,
        deadline: new Date(Date.now() + 5000),
      });
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_auth.mail_event WHERE customer_id=$1 AND type='email.delivered'",
            [f.actorA.customerID],
          )
        ).rows[0].count,
        "1",
      );
      const rows = (
        await f.db.query(
          "SELECT status,payload FROM webdock_auth.mail_callback_inbox WHERE source_id=$1",
          [source],
        )
      ).rows;
      assert.ok(rows.some((r) => r.status === "quarantined"));
      assert.ok(!JSON.stringify(rows).includes("raw-secret-not-stored"));
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_callback_inbox WHERE source_id=$1",
        [source],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_callback_source_connection WHERE source_id=$1",
        [source],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_callback_source WHERE id=$1",
        [source],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
        [f.actorA.customerID],
      );
    }
  });
});
