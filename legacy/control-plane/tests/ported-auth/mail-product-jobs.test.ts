import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { submitMail } = await import("../../src/modules/mail/product/sending");
const { runMailJob } = await import("../../src/modules/mail/product/jobs");
const { pruneMailData } =
  await import("../../src/modules/mail/product/retention");
test.after(() => database.end());
test("concurrent job ticks dispatch a due schedule once and preserve blocked or unknown work", async () => {
  await withMailProductFixture(async (f) => {
    const response = await submitMail(
      f.actorA,
      {
        from: "sender@" + f.domainA.name,
        to: "delivered@test.mail.webdock.dev",
        subject: "job",
        text: "test",
        scheduled_at: new Date(Date.now() + 60000).toISOString(),
      },
      { key: "job" },
    );
    assert.equal(response.status, 200);
    const id = (response.body as { id: string }).id;
    await f.db.query(
      "UPDATE webdock_auth.mail_message SET scheduled_at=now()-interval '1 minute' WHERE id=$1",
      [id],
    );
    await f.db.query(
      "UPDATE webdock_auth.mail_connection SET state='paused' WHERE id=$1",
      [f.actorA.connectionID],
    );
    await runMailJob("dispatch", new Date(Date.now() + 20000));
    assert.equal(
      (
        await f.db.query(
          "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1",
          [id],
        )
      ).rows[0].dispatch_status,
      "scheduled",
    );
    await f.db.query(
      "UPDATE webdock_auth.mail_connection SET state='ready' WHERE id=$1",
      [f.actorA.connectionID],
    );
    await Promise.all([
      runMailJob("dispatch", new Date(Date.now() + 20000)),
      runMailJob("dispatch", new Date(Date.now() + 20000)),
    ]);
    assert.equal(
      (
        await f.db.query(
          "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1",
          [id],
        )
      ).rows[0].dispatch_status,
      "accepted",
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_event WHERE message_id=$1 AND type='email.sent'",
          [id],
        )
      ).rows[0].count,
      "1",
    );
  });
});
test("retention removes aged unknown payload but cannot release its dedupe reservation", async () => {
  await withMailProductFixture(async (f) => {
    const response = await submitMail(
      f.actorA,
      {
        from: "sender@" + f.domainA.name,
        to: "delivered@test.mail.webdock.dev",
        subject: "retain",
        text: "private payload",
        scheduled_at: new Date(Date.now() + 60000).toISOString(),
      },
      { key: "retain" },
    );
    const id = (response.body as { id: string }).id;
    await f.db.query(
      "UPDATE webdock_auth.mail_message SET dispatch_status='outcome_unknown',handoff_started_at=now()-interval '8 days',created_at=now()-interval '8 days' WHERE id=$1",
      [id],
    );
    await f.db.query(
      "UPDATE webdock_auth.mail_idempotency SET status='unknown',expires_at=NULL WHERE message_id=$1",
      [id],
    );
    await pruneMailData(new Date());
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_message_content WHERE message_id=$1",
          [id],
        )
      ).rows[0].count,
      "0",
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT status FROM webdock_auth.mail_idempotency WHERE message_id=$1",
          [id],
        )
      ).rows[0].status,
      "unknown",
    );
  });
});
test("expired dispatch sweep cannot overwrite acceptance committed while waiting", async () =>
  withMailProductFixture(async (f) => {
    const response = await submitMail(
      f.actorA,
      {
        from: "sender@" + f.domainA.name,
        to: "delivered@test.mail.webdock.dev",
        subject: "race",
        text: "test",
        scheduled_at: new Date(Date.now() + 60000).toISOString(),
      },
      { key: "sweep-race" },
    );
    const id = (response.body as { id: string }).id;
    await f.db.query(
      "UPDATE webdock_auth.mail_message SET dispatch_status='dispatching',lease_until=now()-interval '1 minute' WHERE id=$1",
      [id],
    );
    const db = await f.db.connect();
    let tick: ReturnType<typeof runMailJob> | undefined;
    try {
      await db.query("BEGIN");
      await db.query(
        "UPDATE webdock_auth.mail_message SET dispatch_status='accepted',lease_until=NULL,accepted_at=now(),terminal_at=now() WHERE id=$1",
        [id],
      );
      tick = runMailJob("dispatch", new Date(Date.now() + 10000));
      let waiting = false;
      for (let i = 0; i < 100; i++) {
        if (
          (
            await f.db.query(
              "SELECT 1 FROM pg_stat_activity WHERE wait_event_type='Lock' AND position($1 in query)>0",
              ["dispatch_lease_expired"],
            )
          ).rowCount
        ) {
          waiting = true;
          break;
        }
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      assert.equal(waiting, true);
      await db.query("COMMIT");
      await tick;
      assert.equal(
        (
          await f.db.query(
            "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1",
            [id],
          )
        ).rows[0].dispatch_status,
        "accepted",
      );
    } finally {
      await db.query("ROLLBACK");
      db.release();
      await tick;
    }
  }));
test("removing a connection finalizes deferred batches without sending", async () =>
  withMailProductFixture(async (f) => {
    const { submitMailBatch } =
      await import("../../src/modules/mail/product/batches");
    const body = {
      from: "sender@" + f.domainA.name,
      to: "delivered@test.mail.webdock.dev",
      subject: "removed",
      text: "test",
    };
    const result = await submitMailBatch(f.actorA, [body, body], {
      key: "removed-batch",
      inlineDeadline: new Date(),
    });
    assert.equal(result.status, 202);
    const id = (result.body as { id: string }).id;
    await f.db.query(
      "UPDATE webdock_auth.mail_connection SET deleted_at=now(),state='paused' WHERE id=$1",
      [f.actorA.connectionID],
    );
    await runMailJob("dispatch", new Date(Date.now() + 10000));
    assert.equal(
      (
        await f.db.query(
          "SELECT status FROM webdock_auth.mail_batch WHERE id=$1",
          [id],
        )
      ).rows[0].status,
      "failed",
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT status FROM webdock_auth.mail_idempotency WHERE batch_id=$1",
          [id],
        )
      ).rows[0].status,
      "completed",
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_message WHERE connection_id=$1 AND dispatch_status='accepted'",
          [f.actorA.connectionID],
        )
      ).rows[0].count,
      "0",
    );
  }));
test("quota recovery releases definitive failure and keeps uncertain handoffs reserved", async () =>
  withMailProductFixture(async (f) => {
    const { reserveMailQuota, recoverTerminalQuotas } =
      await import("../../src/modules/mail/product/quota");
    const { persistMailMessage } =
      await import("../../src/modules/mail/product/sending");
    const { parseEmail } = await import("@webdock/mail-contracts/validation");
    const { withMailTransaction } =
      await import("../../src/modules/mail/product/store");
    const ids: string[] = [];
    for (const state of ["failed", "outcome_unknown"]) {
      const parsed = parseEmail(
        {
          from: "sender@" + f.domainA.name,
          to: "recipient@example.invalid",
          subject: "recovery",
          text: "test",
        },
        new Date(),
      );
      const id = await withMailTransaction((db) =>
        persistMailMessage(db, f.actorA, parsed, f.domainA.id),
      );
      ids.push(id);
      await reserveMailQuota(f.actorA, id, 1, new Date());
      await f.db.query(
        "UPDATE webdock_auth.mail_message SET dispatch_status=$2,terminal_at=now() WHERE id=$1",
        [id, state],
      );
    }
    await recoverTerminalQuotas();
    const rows = (
      await f.db.query(
        "SELECT message_id,status FROM webdock_auth.mail_usage_reservation WHERE message_id=ANY($1) ORDER BY message_id",
        [ids],
      )
    ).rows;
    assert.deepEqual(
      rows.map((r) => r.status),
      ["released", "reserved"],
    );
  }));
test("persistent dispatch cursor reaches tenants beyond the twenty-tenant tick limit", async () =>
  withMailProductFixture(async (f) => {
    const { persistMailMessage } =
      await import("../../src/modules/mail/product/sending");
    const { parseEmail } = await import("@webdock/mail-contracts/validation");
    const { withMailTransaction } =
      await import("../../src/modules/mail/product/store");
    const customerRows = (
      await f.db.query(
        "INSERT INTO webdock_admin.customers(id,name) SELECT webdock_auth.next_snowflake(),'Mail fairness fixture' FROM generate_series(1,21) RETURNING id",
      )
    ).rows;
    const customers = customerRows.map((r) => r.id);
    f.customers.push(...customers);
    f.organizations.push(
      ...(
        await f.db.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
          [customers],
        )
      ).rows.map((r) => r.organization_id),
    );
    for (const customerID of customers) {
      const c = (
        await f.db.query(
          "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id,state) VALUES($1,'platform',$2,'ready') RETURNING id",
          [customerID, "fairness-" + customerID],
        )
      ).rows[0];
      const name = "fairness-" + customerID + ".example.invalid",
        domain = (
          await f.db.query(
            "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status,provider_id) VALUES($1,$2,'fixture','ownership_verified',$3) RETURNING id",
            [customerID, name, "domain-" + customerID],
          )
        ).rows[0];
      await f.db.query(
        "INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id,provider_id,provider_snapshot,provider_checked_at) VALUES($1,$2,$3,$4,$5,now())",
        [
          customerID,
          c.id,
          domain.id,
          "domain-" + customerID,
          JSON.stringify({
            id: "domain-" + customerID,
            domain: name,
            spf_verified: true,
            dkim_verified: true,
            dmarc_verified: true,
          }),
        ],
      );
      const parsed = parseEmail(
        {
          from: "sender@" + name,
          to: "delivered@test.mail.webdock.dev",
          subject: "fairness",
          text: "fixture",
        },
        new Date(),
      );
      await withMailTransaction((db) =>
        persistMailMessage(
          db,
          {
            customerID,
            connectionID: c.id,
            principal: { kind: "job", jobID: "1" },
          },
          parsed,
          domain.id,
        ),
      );
    }
    await runMailJob("dispatch", new Date(Date.now() + 45000));
    const first = Number(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_message WHERE customer_id=ANY($1) AND dispatch_status='accepted'",
          [customers],
        )
      ).rows[0].count,
    );
    assert.ok(first > 0 && first <= 20);
    await runMailJob("dispatch", new Date(Date.now() + 45000));
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_message WHERE customer_id=ANY($1) AND dispatch_status='accepted'",
          [customers],
        )
      ).rows[0].count,
      "21",
    );
  }));
test("maintenance drains multiple bounded retention batches within one tick", async () =>
  withMailProductFixture(async (f) => {
    await f.db.query(
      `INSERT INTO webdock_auth.mail_request_log(customer_id,connection_id,route,method,status,duration_ms,request_id,created_at) SELECT $1,$2,'/fixture','GET',200,1,gen_random_uuid(),now()-interval '31 days' FROM generate_series(1,501)`,
      [f.customers[0], f.actorA.connectionID],
    );
    const result = await runMailJob(
      "maintenance",
      new Date(Date.now() + 15000),
    );
    assert.equal(result.failed, undefined);
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_request_log WHERE customer_id=$1",
          [f.customers[0]],
        )
      ).rows[0].count,
      "0",
    );
  }));
