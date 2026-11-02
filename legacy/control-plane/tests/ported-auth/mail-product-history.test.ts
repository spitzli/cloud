import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { listMailMessages, getMailMessage } =
  await import("../../src/modules/mail/product/messages");
const { getMailAnalytics } =
  await import("../../src/modules/mail/product/analytics");
test.after(() => database.end());
test("local history and analytics share recipient truth, private lists and stable cursors", async () => {
  await withMailProductFixture(async (f) => {
    const ids: string[] = [];
    for (let i = 0; i < 3; i++) {
      const row = (
        await f.db.query(
          "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference,accepted_at,created_at,simulated) VALUES($1,$2,$3,'sender@example.com',$4,'accepted',$5,now(),date_trunc('second',now()),$6) RETURNING id",
          [
            f.actorA.customerID,
            f.actorA.connectionID,
            f.domainA.id,
            "Subject " + i,
            randomUUID(),
            i === 2,
          ],
        )
      ).rows[0];
      ids.push(row.id);
    }
    for (const [i, email, status, reason] of [
      [0, "one@example.com", "delivered", null],
      [0, "two@example.com", "delivered", null],
      [1, "three@example.com", "bounced", null],
      [0, "hidden@example.com", "suppressed", "manual"],
      [1, "hidden2@example.com", "suppressed", "manual"],
      [2, "simulator@example.com", "delivered", null],
    ] as const)
      await f.db.query(
        "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category,delivery_status,suppression_reason) VALUES($1,$2,$3,$4,$5,$6,$7)",
        [
          f.actorA.customerID,
          f.actorA.connectionID,
          ids[i],
          email,
          email.startsWith("hidden") ? "bcc" : "to",
          status,
          reason,
        ],
      );
    const stats = await getMailAnalytics(f.actorA, { days: 7 });
    assert.equal(stats.messagesAccepted, 2);
    assert.equal(stats.recipientsAccepted, 3);
    assert.equal(stats.delivered, 2);
    assert.equal(stats.bounced, 1);
    assert.equal(stats.suppressed, 2);
    assert.equal(stats.deliveryRate, 2 / 3);
    assert.equal(stats.days.length, 7);
    const page = await listMailMessages(f.actorA, { limit: 1 });
    assert.equal(page.data.length, 1);
    assert.ok(page.nextCursor);
    assert.ok(!JSON.stringify(page).includes("hidden@example.com"));
    const second = await listMailMessages(f.actorA, {
      limit: 1,
      after: page.nextCursor!,
    });
    assert.notEqual(second.data[0].id, page.data[0].id);
    await assert.rejects(
      listMailMessages(f.actorB, { limit: 1, after: page.nextCursor! }),
      { status: 422 },
    );
    await assert.rejects(getMailMessage(f.actorB, ids[0]), { status: 404 });
    assert.equal(
      (await getMailMessage(f.actorA, ids[0])).content_available,
      false,
    );
    const empty = await getMailAnalytics(f.actorB, { days: 7 });
    assert.equal(empty.deliveryRate, null);
  });
});
test("expired content is hidden at read time even before the maintenance tick", async () =>
  withMailProductFixture(async (f) => {
    const { submitMail } =
      await import("../../src/modules/mail/product/sending");
    const { getMailMessage } =
      await import("../../src/modules/mail/product/messages");
    const response = await submitMail(
        f.actorA,
        {
          from: "sender@" + f.domainA.name,
          to: "delivered@test.mail.webdock.dev",
          subject: "Expired",
          text: "expired-private-content",
        },
        { key: "expired-read" },
      ),
      id = (response.body as { id: string }).id;
    await f.db.query(
      "UPDATE webdock_auth.mail_message SET dispatch_status='outcome_unknown',terminal_at=NULL,handoff_started_at=now()-interval '8 days' WHERE id=$1",
      [id],
    );
    let message = await getMailMessage(f.actorA, id);
    assert.equal(message.content_available, false);
    assert.equal(message.content, null);
    await f.db.query(
      "UPDATE webdock_auth.mail_message SET dispatch_status='accepted',terminal_at=now()-interval '31 days' WHERE id=$1",
      [id],
    );
    message = await getMailMessage(f.actorA, id);
    assert.equal(message.content_available, false);
    assert.equal(message.content, null);
  }));
