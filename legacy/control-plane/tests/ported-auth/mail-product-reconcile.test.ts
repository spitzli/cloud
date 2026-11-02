import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { reconcileMail } =
  await import("../../src/modules/mail/product/reconcile");
test.after(() => database.end());
test("reconciliation reads only the exact bound account/MID and never resends", async () => {
  await withMailProductFixture(async (f) => {
    const id = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference,provider_mid,handoff_started_at) VALUES($1,$2,$3,'sender@example.com','unknown','outcome_unknown',$4,'9007199254740993123',now()) RETURNING id",
        [
          f.actorA.customerID,
          f.actorA.connectionID,
          f.domainA.id,
          randomUUID(),
        ],
      )
    ).rows[0].id;
    await f.db.query(
      "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category,included_in_handoff) VALUES($1,$2,$3,'recipient@example.com','to',true)",
      [f.actorA.customerID, f.actorA.connectionID, id],
    );
    const operator = { ...f.actorA, principal: f.operator.principal };
    let calls = 0;
    const deps = {
      headers: async () => ({ Authorization: "fixture-child-only" }),
      fetcher: async (url: URL | RequestInfo, init?: RequestInit) => {
        calls++;
        assert.equal(
          String(url),
          "https://pro.api.serversmtp.com/api/v2/analytics/9007199254740993123",
        );
        assert.equal(init?.method ?? "GET", "GET");
        return new Response(
          '{"id":9007199254740993123,"recipient":"recipient@example.com","status":"SUCCESS"}',
        );
      },
    };
    await assert.rejects(reconcileMail(f.actorA, id, deps));
    assert.equal(calls, 0);
    await reconcileMail(operator, id, deps);
    assert.equal(calls, 1);
    const state = (
      await f.db.query(
        "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1",
        [id],
      )
    ).rows[0].dispatch_status;
    assert.equal(state, "accepted");
    assert.equal(
      (
        await f.db.query(
          "SELECT count(*) FROM webdock_auth.mail_event WHERE message_id=$1 AND source='reconciliation' AND type='email.delivered'",
          [id],
        )
      ).rows[0].count,
      "1",
    );
    await assert.rejects(
      reconcileMail(operator, id, {
        ...deps,
        fetcher: async () =>
          new Response(
            '{"id":"1","recipient":"recipient@example.com","status":"SUCCESS"}',
          ),
      }),
      { code: "provider_identity_mismatch" },
    );
  });
});
test("late callback or reconciliation cannot revive expired unknown content before maintenance", async () =>
  withMailProductFixture(async (f) => {
    const { applyProviderEvent } =
      await import("../../src/modules/mail/product/callback-inbox");
    const { withMailTransaction } =
      await import("../../src/modules/mail/product/store");
    const { getMailMessage } =
      await import("../../src/modules/mail/product/messages");
    const source = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_callback_source(secret_hash,kind,verified_at) VALUES($1,'account',now()) RETURNING id",
        [randomUUID()],
      )
    ).rows[0].id;
    await f.db.query(
      "INSERT INTO webdock_auth.mail_callback_source_connection(source_id,connection_id) VALUES($1,$2)",
      [source, f.actorA.connectionID],
    );
    try {
      for (const [index, channel] of ["callback", "reconcile"].entries()) {
        const mid = String(9007199254740993123n + BigInt(index));
        const id = (
          await f.db.query(
            "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference,provider_mid,handoff_started_at) VALUES($1,$2,$3,'sender@example.com','expired unknown','outcome_unknown',$4,$5,now()-interval '8 days') RETURNING id",
            [
              f.customers[0],
              f.actorA.connectionID,
              f.domainA.id,
              randomUUID(),
              mid,
            ],
          )
        ).rows[0].id;
        await f.db.query(
          "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category,included_in_handoff) VALUES($1,$2,$3,'recipient@example.com','to',true)",
          [f.customers[0], f.actorA.connectionID, id],
        );
        await f.db.query(
          "INSERT INTO webdock_auth.mail_message_content(customer_id,connection_id,message_id,text_body) VALUES($1,$2,$3,'expired private body')",
          [f.customers[0], f.actorA.connectionID, id],
        );
        assert.equal(
          (await getMailMessage(f.actorA, id)).content_available,
          false,
        );
        if (channel === "callback")
          await withMailTransaction((db) =>
            applyProviderEvent(db, source, {
              mid,
              type: "email.delivered",
              recipient: "recipient@example.com",
              occurredAt: new Date().toISOString(),
              details: {},
            }),
          );
        else
          await reconcileMail(
            { ...f.actorA, principal: f.operator.principal },
            id,
            {
              headers: async () => ({ Authorization: "fixture" }),
              fetcher: async () =>
                Response.json({
                  id: mid,
                  recipient: "recipient@example.com",
                  status: "SUCCESS",
                }),
            },
          );
        assert.equal(
          (await getMailMessage(f.actorA, id)).dispatch_status,
          "accepted",
        );
        assert.equal(
          (await getMailMessage(f.actorA, id)).content_available,
          false,
        );
        assert.equal(
          (
            await f.db.query(
              "SELECT count(*) FROM webdock_auth.mail_message_content WHERE message_id=$1",
              [id],
            )
          ).rows[0].count,
          "0",
        );
      }
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_callback_source_connection WHERE source_id=$1",
        [source],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_callback_source WHERE id=$1",
        [source],
      );
    }
  }));
