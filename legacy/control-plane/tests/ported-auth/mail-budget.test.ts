import test from "node:test";
import assert from "node:assert/strict";
import { database } from "../../src/db/runtime";
import { nativeMailSchemaSQL } from "@webdock/mail-core/schema";
import {
  mailBudgetSchemaSQL,
  reserveMailBudget,
  settleMailBudget,
} from "../../src/modules/mail/mail-budget";
import { transaction } from "../../src/modules/hosting/db";
const target = new URL(process.env.DATABASE_URL!);
if (
  target.hostname !== "127.0.0.1" ||
  target.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(() => database.end());

test("concurrent native sends reserve recipient units once and retain uncertain budget holds", async () => {
  await database.query(nativeMailSchemaSQL);
  await database.query(mailBudgetSchemaSQL);
  await database.query(mailBudgetSchemaSQL);
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Mail budget fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0].organization_id;
  const previous = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  try {
    // Outgoing-only customers must not require or allocate a Stalwart service.
    await database.query(
      `INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Fixture','Fixture','{"mailMessages":7}','{}',1,'fixture')`,
      [customerID],
    );
    await database.query(
      `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"mailEnabled":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings`,
    );
    const ids = (
      await database.query(
        "SELECT webdock_auth.next_snowflake() AS id FROM generate_series(1,20)",
      )
    ).rows.map((row) => row.id as string);
    const attempts = await Promise.allSettled(
      ids.map((id) =>
        transaction((db) => reserveMailBudget(db, customerID, id, 1)),
      ),
    );
    assert.equal(
      attempts.filter((result) => result.status === "fulfilled").length,
      7,
    );
    const held = ids.filter((_, i) => attempts[i].status === "fulfilled");
    const replay = await transaction((db) =>
      reserveMailBudget(db, customerID, held[0], 1),
    );
    assert.equal(replay.created, false);
    assert.deepEqual(replay.path, { kind: "direct" });
    await assert.rejects(
      transaction((db) => reserveMailBudget(db, customerID, held[0], 2)),
    );
    await transaction((db) =>
      settleMailBudget(db, customerID, held[0], "accepted"),
    );
    await transaction((db) =>
      settleMailBudget(db, customerID, held[0], "accepted"),
    );
    await assert.rejects(
      transaction((db) =>
        settleMailBudget(db, customerID, held[0], "released"),
      ),
    );
    await transaction((db) =>
      settleMailBudget(db, customerID, held[1], "released"),
    );
    await transaction((db) =>
      settleMailBudget(db, customerID, held[1], "released"),
    );
    const fresh = (
      await database.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    await transaction((db) => reserveMailBudget(db, customerID, fresh, 1));
    assert.equal(
      (
        await database.query(
          "SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1",
          [customerID],
        )
      ).rowCount,
      0,
    );
    const usage = (
      await database.query(
        "SELECT reserved,accepted FROM webdock_mail.month_usage WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0];
    assert.equal(String(usage.reserved), "6");
    assert.equal(String(usage.accepted), "1");
    await database.query(
      "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
      [customerID],
    );
    await assert.rejects(
      transaction((db) => reserveMailBudget(db, customerID, ids[19], 1)),
    );
    await database.query(
      "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
      [customerID],
    );
    await database.query(
      `UPDATE webdock_auth.tenant_subscription SET base='{"mailMessages":null}' WHERE customer_id=$1`,
      [customerID],
    );
    const unlimited = (
      await database.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    await transaction((db) =>
      reserveMailBudget(db, customerID, unlimited, 100),
    );
    assert.equal(
      String(
        (
          await database.query(
            "SELECT reserved FROM webdock_mail.month_usage WHERE customer_id=$1",
            [customerID],
          )
        ).rows[0].reserved,
      ),
      "106",
    );
    const currentPeriod = (
      await database.query(
        "SELECT date_trunc('month',now() AT TIME ZONE 'UTC')::date::text AS period",
      )
    ).rows[0].period;
    const oldPeriod = (
      await database.query(
        "SELECT (date_trunc('month',now() AT TIME ZONE 'UTC')-interval '1 month')::date::text AS period",
      )
    ).rows[0].period;
    await database.query(
      "INSERT INTO webdock_mail.month_usage(customer_id,period_start,reserved) VALUES($1,$2,100)",
      [customerID, oldPeriod],
    );
    await database.query(
      "UPDATE webdock_mail.month_usage SET reserved=reserved-100 WHERE customer_id=$1 AND period_start=$2",
      [customerID, currentPeriod],
    );
    await database.query(
      "UPDATE webdock_mail.budget_reservation SET period_start=$2 WHERE reference_id=$1",
      [unlimited, oldPeriod],
    );
    await transaction((db) =>
      reserveMailBudget(db, customerID, unlimited, 100),
    );
    await transaction((db) =>
      reserveMailBudget(db, customerID, unlimited, 100),
    );
    assert.equal(
      (
        await database.query(
          "SELECT reserved FROM webdock_mail.month_usage WHERE customer_id=$1 AND period_start=$2",
          [customerID, oldPeriod],
        )
      ).rows[0].reserved,
      "0",
    );
    assert.equal(
      (
        await database.query(
          "SELECT reserved FROM webdock_mail.month_usage WHERE customer_id=$1 AND period_start=$2",
          [customerID, currentPeriod],
        )
      ).rows[0].reserved,
      "106",
    );
    // Once incoming Mail was activated, unavailable instances must never bypass the native route.
    await database.query(
      `INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES($1,$2,'local-test',true,'pending',1,0)`,
      [customerID, "mail-" + customerID],
    );
    const routed = (
      await database.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    await assert.rejects(
      transaction((db) => reserveMailBudget(db, customerID, routed, 1)),
      /Incoming Mail/,
    );
    await database.query(
      "UPDATE webdock_mail.service SET state='ready',applied_revision=1 WHERE customer_id=$1",
      [customerID],
    );
    await assert.rejects(
      transaction((db) => reserveMailBudget(db, customerID, held[0], 1)),
    );
    assert.deepEqual(
      (await transaction((db) => reserveMailBudget(db, customerID, routed, 1)))
        .path,
      { kind: "stalwart", serviceRevision: "1" },
    );
    await database.query(
      "UPDATE webdock_mail.service SET desired_enabled=false,state='suspended' WHERE customer_id=$1",
      [customerID],
    );
    await assert.rejects(
      transaction((db) => reserveMailBudget(db, customerID, routed, 1)),
      /Incoming Mail/,
    );
  } finally {
    for (const table of ["budget_reservation", "month_usage", "service"])
      await database.query(
        `DELETE FROM webdock_mail.${table} WHERE customer_id=$1`,
        [customerID],
      );
    await database.query(
      "DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
      [customerID],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    );
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customerID,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      org,
    ]);
    if (previous)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [previous.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});
