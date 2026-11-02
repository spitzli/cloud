import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { submitMail, dispatchMail } =
  await import("../../src/modules/mail/product/sending");
const { rescheduleMail, cancelScheduledMail } =
  await import("../../src/modules/mail/product/schedules");
test.after(() => database.end());
test("schedule replay ignores elapsed requested time; revision fences cancel/reschedule and dispatch", async () => {
  await withMailProductFixture(async (f) => {
    const time = new Date(Date.now() + 60_000),
      body = {
        from: "sender@" + f.domainA.name,
        to: "delivered@test.mail.webdock.dev",
        subject: "Later",
        text: "Hello",
        scheduled_at: time.toISOString(),
      };
    const result = await submitMail(f.actorA, body, { key: "schedule" });
    assert.equal(result.status, 200);
    const id = (result.body as { id: string }).id;
    const replay = await submitMail(f.actorA, body, {
      key: "schedule",
      now: () => new Date(time.getTime() + 1000),
    });
    assert.deepEqual(replay, result);
    await rescheduleMail(
      f.actorA,
      id,
      new Date(time.getTime() + 60_000).toISOString(),
      1,
    );
    await assert.rejects(cancelScheduledMail(f.actorA, id, 1), { status: 409 });
    await cancelScheduledMail(f.actorA, id, 2);
    assert.equal(
      (await dispatchMail(id, new Date(Date.now() + 1000))).kind,
      "rejected",
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT dispatch_status,revision FROM webdock_auth.mail_message WHERE id=$1",
          [id],
        )
      ).rows[0].dispatch_status,
      "canceled",
    );
    await assert.rejects(rescheduleMail(f.actorB, id, time.toISOString(), 3), {
      status: 404,
    });
  });
});
