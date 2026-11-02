import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { consumeMailRate } =
  await import("../../src/modules/mail/product/rate-limit");
test.after(() => database.end());
test("atomic tenant rate counters reject excess without partial writes or crossing tenants", async () => {
  await withMailProductFixture(async (f) => {
    await consumeMailRate(f.actorA, { requests: 119, recipients: 0, keys: 0 });
    const results = await Promise.allSettled([
      consumeMailRate(f.actorA, { requests: 1, recipients: 0, keys: 0 }),
      consumeMailRate(f.actorA, { requests: 1, recipients: 0, keys: 0 }),
    ]);
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    await assert.rejects(
      consumeMailRate(f.actorA, { requests: 1, recipients: 500, keys: 0 }),
      { status: 429 },
    );
    const rows = (
      await f.db.query(
        "SELECT kind,count FROM webdock_auth.mail_rate_window WHERE customer_id=$1",
        [f.actorA.customerID],
      )
    ).rows;
    assert.deepEqual(rows, [{ kind: "requests", count: 120 }]);
    await consumeMailRate(f.actorB, { requests: 1, recipients: 0, keys: 0 });
  });
});
