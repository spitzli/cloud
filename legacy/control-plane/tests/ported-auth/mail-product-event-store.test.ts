import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { recordMailEvent } =
  await import("../../src/modules/mail/product/events");
test.after(() => database.end());
test("duplicate canonical events project once, suppress tenantwide and create one redacted outbox item", async () => {
  await withMailProductFixture(async (f) => {
    const m = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference) VALUES($1,$2,$3,'sender@example.com','test','accepted',$4) RETURNING id",
        [
          f.actorA.customerID,
          f.actorA.connectionID,
          f.domainA.id,
          randomUUID(),
        ],
      )
    ).rows[0];
    await f.db.query(
      "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category) VALUES($1,$2,$3,'hidden@example.com','bcc')",
      [f.actorA.customerID, f.actorA.connectionID, m.id],
    );
    await f.db.query(
      "INSERT INTO webdock_auth.mail_webhook(customer_id,connection_id,url,events,encrypted_secret) VALUES($1,$2,'https://example.com/hook','[\"email.complained\"]','unused-fixture-secret')",
      [f.actorA.customerID, f.actorA.connectionID],
    );
    const event = {
      customerID: f.actorA.customerID,
      connectionID: f.actorA.connectionID,
      resourceID: m.id,
      type: "email.complained" as const,
      recipient: "hidden@example.com",
      occurredAt: new Date().toISOString(),
      source: "provider" as const,
      dedupeKey: "complaint-one",
      details: { body: "do-not-persist", token: "do-not-persist" },
    };
    const results = await Promise.all([
      recordMailEvent(event),
      recordMailEvent(event),
    ]);
    assert.equal(results.filter((r) => r.inserted).length, 1);
    const outbox = (
      await f.db.query(
        "SELECT body FROM webdock_auth.mail_webhook_delivery WHERE customer_id=$1",
        [f.actorA.customerID],
      )
    ).rows;
    assert.equal(outbox.length, 1);
    assert.ok(!outbox[0].body.includes("hidden@example.com"));
    assert.ok(!outbox[0].body.includes("do-not-persist"));
    assert.equal(
      (
        await f.db.query(
          "SELECT reason FROM webdock_auth.mail_suppression WHERE customer_id=$1",
          [f.actorA.customerID],
        )
      ).rows[0].reason,
      "complained",
    );
    await assert.rejects(
      recordMailEvent({
        ...event,
        customerID: f.actorB.customerID,
        dedupeKey: "foreign",
      }),
    );
    await assert.rejects(
      recordMailEvent({
        ...event,
        recipient: "not-envelope@example.com",
        dedupeKey: "not-envelope",
      }),
    );
  });
});
