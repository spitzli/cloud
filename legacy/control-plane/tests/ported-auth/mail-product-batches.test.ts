import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { submitMailBatch, getMailBatch, advanceMailBatch } =
  await import("../../src/modules/mail/product/batches");
const { encryptMailSecret } =
  await import("../../src/modules/platform/platform");
test.after(() => database.end());
test("batch preflight rejects every invalid item before any handoff; unknown second item stops the prefix", async () => {
  await withMailProductFixture(async (f) => {
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
    const body = {
      from: "sender@" + f.domainA.name,
      to: "recipient@example.com",
      subject: "test",
      text: "hello",
    };
    let calls = 0;
    const send = async () => {
      calls++;
      return calls === 1
        ? { kind: "accepted" as const, mid: "9007199254740993123" }
        : { kind: "unknown" as const, code: "timeout" };
    };
    try {
      const invalid = await submitMailBatch(
        f.actorA,
        [body, { ...body, to: "invalid" }],
        { key: "invalid", send },
      );
      assert.equal(invalid.status, 422);
      assert.equal(calls, 0);
      const bodies = [
        body,
        { ...body, subject: "two" },
        { ...body, subject: "three" },
      ];
      const first = await submitMailBatch(f.actorA, bodies, {
        key: "partial",
        send,
      });
      assert.equal(first.status, 502);
      const result = first.body as {
        id: string;
        data: unknown[];
        failed_index: number;
      };
      assert.equal(result.failed_index, 1);
      assert.equal(result.data.length, 1);
      assert.equal(calls, 2);
      assert.deepEqual(
        await submitMailBatch(f.actorA, bodies, { key: "partial", send }),
        first,
      );
      assert.equal(calls, 2);
      await advanceMailBatch(result.id, new Date(Date.now() + 20000), { send });
      assert.equal(calls, 2);
      await assert.rejects(getMailBatch(f.actorB, result.id), { status: 404 });
      assert.equal(
        (
          await f.db.query(
            "SELECT status FROM webdock_auth.mail_idempotency WHERE customer_id=$1 AND key='partial'",
            [f.actorA.customerID],
          )
        ).rows[0].status,
        "unknown",
      );
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
        [f.actorA.customerID],
      );
    }
  });
});
test("deferred batch persists every item and polling advances without changing the 202 replay", async () => {
  await withMailProductFixture(async (f) => {
    const bodies = Array.from({ length: 100 }, (_, i) => ({
      from: "sender@" + f.domainA.name,
      to: "delivered@test.mail.webdock.dev",
      subject: String(i),
      text: "test",
    }));
    const first = await submitMailBatch(f.actorA, bodies, {
      key: "defer",
      inlineDeadline: new Date(0),
    });
    assert.equal(first.status, 202);
    const id = (first.body as { id: string }).id;
    await Promise.all([
      advanceMailBatch(id, new Date(Date.now() + 20000)),
      advanceMailBatch(id, new Date(Date.now() + 20000)),
    ]);
    const result = await getMailBatch(f.actorA, id);
    assert.equal(result.status, "succeeded");
    assert.equal(result.data.length, 100);
    assert.deepEqual(
      await submitMailBatch(f.actorA, bodies, { key: "defer" }),
      first,
    );
  });
});
test("an asynchronous unknown pins the original 202 beyond the ordinary replay TTL", async () => {
  await withMailProductFixture(async (f) => {
    const bodies = [
      {
        from: "sender@" + f.domainA.name,
        to: "delivered@test.mail.webdock.dev",
        subject: "Async",
        text: "test",
      },
    ];
    const first = await submitMailBatch(f.actorA, bodies, {
      key: "async-unknown",
      inlineDeadline: new Date(0),
    });
    assert.equal(first.status, 202);
    const id = (first.body as { id: string }).id;
    await f.db.query(
      "UPDATE webdock_auth.mail_message SET dispatch_status='outcome_unknown' WHERE id IN (SELECT message_id FROM webdock_auth.mail_batch_item WHERE batch_id=$1)",
      [id],
    );
    await advanceMailBatch(id, new Date(Date.now() + 20000));
    // Covers a persisted pre-fix/partial-transition row, too: expiration must check the work itself.
    await f.db.query(
      "UPDATE webdock_auth.mail_idempotency SET status='completed',expires_at=now()-interval '1 day' WHERE batch_id=$1",
      [id],
    );
    assert.deepEqual(
      await submitMailBatch(f.actorA, bodies, {
        key: "async-unknown",
        inlineDeadline: new Date(0),
      }),
      first,
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_batch WHERE customer_id=$1",
          [f.actorA.customerID],
        )
      ).rows[0].count,
      "1",
    );
  });
});
