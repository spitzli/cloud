import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { setMailSuppression } =
  await import("../../src/modules/mail/product/suppressions");
test.after(() => database.end());
test("manual or bounce updates preserve complaint and unsubscribe evidence", async () => {
  await withMailProductFixture(async (f) => {
    for (const reason of ["complained", "unsubscribed"]) {
      await f.db.query(
        "INSERT INTO webdock_auth.mail_suppression(customer_id,email,reason,source) VALUES($1,$2,$3,'provider')",
        [f.actorA.customerID, reason + "@example.com", reason],
      );
      for (const later of ["manual", "bounced"])
        await setMailSuppression(f.actorA, {
          email: reason + "@example.com",
          reason: later,
        });
      const row = (
        await f.db.query(
          "SELECT reason,source FROM webdock_auth.mail_suppression WHERE customer_id=$1 AND email=$2",
          [f.actorA.customerID, reason + "@example.com"],
        )
      ).rows[0];
      assert.deepEqual(row, { reason, source: "provider" });
    }
  });
});
