import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { reserveMailQuota, settleMailQuota } =
  await import("../../src/modules/mail/product/quota");
test.after(() => database.end());
test("last available recipient is reserved once, unknown retains it and settlement is idempotent", async () => {
  await withMailProductFixture(async (f) => {
    await f.db.query(
      "UPDATE webdock_auth.platform_settings SET settings=settings || '{\"mailDefaultLimit\":1}'::jsonb WHERE id=true",
    );
    const ids: string[] = [];
    for (let i = 0; i < 2; i++) {
      const row = (
        await f.db.query(
          "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference) VALUES($1,$2,$3,'sender@example.com','quota','scheduled',$4) RETURNING id",
          [
            f.actorA.customerID,
            f.actorA.connectionID,
            f.domainA.id,
            randomUUID(),
          ],
        )
      ).rows[0];
      ids.push(row.id);
      await f.db.query(
        "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category) VALUES($1,$2,$3,'to@example.com','to')",
        [f.actorA.customerID, f.actorA.connectionID, row.id],
      );
    }
    const results = await Promise.allSettled(
      ids.map((id) => reserveMailQuota(f.actorA, id, 1, f.now)),
    );
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    const winner = ids[results.findIndex((r) => r.status === "fulfilled")],
      loser = ids.find((id) => id !== winner)!;
    await settleMailQuota(winner, "unknown");
    await assert.rejects(reserveMailQuota(f.actorA, loser, 1, f.now), {
      code: "quota_exceeded",
    });
    await settleMailQuota(winner, "accepted");
    await settleMailQuota(winner, "accepted");
    const usage = (
      await f.db.query(
        "SELECT reserved,consumed FROM webdock_auth.mail_usage_period WHERE customer_id=$1",
        [f.actorA.customerID],
      )
    ).rows[0];
    assert.deepEqual(usage, { reserved: "0", consumed: "1" });
    await assert.rejects(reserveMailQuota(f.actorB, loser, 1, f.now));
    await reserveMailQuota(
      f.actorA,
      loser,
      1,
      new Date("2026-11-01T00:00:00Z"),
    );
    await settleMailQuota(loser, "rejected");
  });
});
test("native and product gateways share the last tenant recipient allowance", async () =>
  withMailProductFixture(async (f) => {
    const { nativeMailSchemaSQL } = await import("@webdock/mail-core/schema");
    const { mailBudgetSchemaSQL, reserveMailBudget } =
      await import("../../src/modules/mail/mail-budget");
    const { withMailTransaction } =
      await import("../../src/modules/mail/product/store");
    await f.db.query(nativeMailSchemaSQL);
    await f.db.query(mailBudgetSchemaSQL);
    const customer = f.actorA.customerID;
    await f.db.query(
      `INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Quota fixture','Quota fixture','{"mailMessages":1}','{}',1,'fixture')`,
      [customer],
    );
    try {
      const id = (
        await f.db.query(
          "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference) VALUES($1,$2,$3,'sender@example.com','shared quota','scheduled',$4) RETURNING id",
          [customer, f.actorA.connectionID, f.domainA.id, randomUUID()],
        )
      ).rows[0].id;
      await f.db.query(
        "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category) VALUES($1,$2,$3,'to@example.com','to')",
        [customer, f.actorA.connectionID, id],
      );
      const native = (
        await f.db.query("SELECT webdock_auth.next_snowflake() AS id")
      ).rows[0].id;
      const outcomes = await Promise.allSettled([
        reserveMailQuota(f.actorA, id, 1, new Date()),
        withMailTransaction((db) => reserveMailBudget(db, customer, native, 1)),
      ]);
      assert.equal(outcomes.filter((r) => r.status === "fulfilled").length, 1);
      const product = Number(
          (
            await f.db.query(
              "SELECT coalesce(sum(reserved+consumed),0) AS used FROM webdock_auth.mail_usage_period WHERE customer_id=$1",
              [customer],
            )
          ).rows[0].used,
        ),
        legacy = Number(
          (
            await f.db.query(
              "SELECT coalesce(sum(reserved+accepted),0) AS used FROM webdock_mail.month_usage WHERE customer_id=$1",
              [customer],
            )
          ).rows[0].used,
        );
      assert.equal(product + legacy, 1);
    } finally {
      await f.db.query(
        "DELETE FROM webdock_mail.budget_reservation WHERE customer_id=$1",
        [customer],
      );
      await f.db.query(
        "DELETE FROM webdock_mail.month_usage WHERE customer_id=$1",
        [customer],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
        [customer],
      );
    }
  }));
test("incoming activation cannot change the route of pending product work", async () =>
  withMailProductFixture(async (f) => {
    const { nativeMailSchemaSQL } = await import("@webdock/mail-core/schema");
    const { requestMailService } = await import("@webdock/mail-core");
    await f.db.query(nativeMailSchemaSQL);
    const customer = f.actorA.customerID;
    const id = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference) VALUES($1,$2,$3,'sender@example.com','pending route','scheduled',$4) RETURNING id",
        [customer, f.actorA.connectionID, f.domainA.id, randomUUID()],
      )
    ).rows[0].id;
    const request = {
      customerID: customer,
      enabled: true,
      actorID: f.operator.id,
      expectedRevision: "0",
      hostID: "local-test",
    };
    try {
      await assert.rejects(
        requestMailService(f.db, request),
        /Reconcile pending sends/,
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_mail.service WHERE customer_id=$1",
            [customer],
          )
        ).rows[0].count,
        "0",
      );
      await f.db.query(
        "UPDATE webdock_auth.mail_message SET dispatch_status='failed' WHERE id=$1",
        [id],
      );
      assert.equal((await requestMailService(f.db, request)).state, "pending");
    } finally {
      await f.db.query(
        "DELETE FROM webdock_mail.operation WHERE customer_id=$1",
        [customer],
      );
      await f.db.query(
        "DELETE FROM webdock_mail.service WHERE customer_id=$1",
        [customer],
      );
    }
  }));
