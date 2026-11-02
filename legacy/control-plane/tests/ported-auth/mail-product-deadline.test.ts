import test from "node:test";
import assert from "node:assert/strict";
import { assertMailTestDatabase } from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { mailDatabase, withMailTransaction } =
  await import("../../src/modules/mail/product/store");
const { withMailDeadline } =
  await import("../../src/modules/mail/product/deadline");
test.after(() => database.end());
test("absolute Mail deadline interrupts blocked SQL and rolls back an unfinished transaction", async () => {
  let started = Date.now();
  await assert.rejects(
    withMailDeadline(new Date(started + 150), () =>
      mailDatabase.query("SELECT pg_sleep(5)"),
    ),
  );
  assert.ok(Date.now() - started < 1000);
  started = Date.now();
  await assert.rejects(
    withMailDeadline(new Date(started + 180), () =>
      withMailTransaction(async (db) => {
        await db.query("CREATE TEMP TABLE deadline_probe(id int)");
        await db.query("SELECT pg_sleep(0.1)");
        await db.query("SELECT pg_sleep(0.15)");
        throw Error("deadline failed");
      }),
    ),
  );
  assert.ok(Date.now() - started < 1000);
  assert.equal(
    (await database.query("SELECT 1 AS healthy")).rows[0].healthy,
    1,
  );
});
