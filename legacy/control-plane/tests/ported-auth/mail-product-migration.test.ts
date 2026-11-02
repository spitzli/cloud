import test from "node:test";
import assert from "node:assert/strict";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { backfillMailConnections } =
  await import("../../src/modules/mail/product/backfill");
test.after(() => database.end());
test("backfill preserves source tables, is idempotent and requires gateway off", async () =>
  withMailProductFixture(async (f) => {
    await assert.rejects(backfillMailConnections({ dryRun: true }), /Disable/);
    await f.db.query(
      `UPDATE webdock_auth.platform_settings SET settings=jsonb_set(settings,'{mailGatewayEnabled}','false') WHERE id=true`,
    );
    await f.db.query(
      `INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state) VALUES($1,$2,$3,'ready')`,
      [
        f.customers[0],
        "fixture-" + f.customers[0],
        f.suffix + "@example.invalid",
      ],
    );
    try {
      const before = (
        await f.db.query("SELECT count(*) FROM webdock_auth.mail_connection")
      ).rows[0].count;
      const preview = await backfillMailConnections({ dryRun: true });
      assert.equal(preview.dryRun, true);
      assert.equal(
        (await f.db.query("SELECT count(*) FROM webdock_auth.mail_connection"))
          .rows[0].count,
        before,
      );
      await backfillMailConnections({ dryRun: false });
      const repeated = await backfillMailConnections({ dryRun: false });
      assert.equal(repeated.created, 0);
      assert.equal(repeated.domains, 0);
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
            [f.customers[0]],
          )
        ).rows[0].count,
        "1",
      );
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
        [f.customers[0]],
      );
    }
  }));
test("Relaykit import validates identity, keeps unknown and scheduled work, seals secrets and resumes without duplicates", async () =>
  withMailProductFixture(async (f) => {
    const { importRelaykit } =
      await import("../../src/modules/mail/product/import");
    const { decryptMailSecret } =
      await import("../../src/modules/platform/platform");
    const schema = "relaykit_" + f.suffix;
    await f.db.query(`CREATE SCHEMA "${schema}"`);
    try {
      await f.db
        .query(`CREATE TABLE "${schema}".domains(turbo_id text,user_id text,name text,created_at timestamptz);
   CREATE TABLE "${schema}".emails(id text,user_id text,domain text,"from" text,"to" text[],subject text,payload jsonb,mid text,last_event text,last_event_at timestamptz,scheduled_at timestamptz,created_at timestamptz);
   CREATE TABLE "${schema}".email_events(id text,email_id text,type text,recipient text,created_at timestamptz);
   CREATE TABLE "${schema}".webhooks(id text,user_id text,url text,secret text,events text[],created_at timestamptz);
   CREATE TABLE "${schema}".webhook_deliveries(id text,webhook_id text,type text,payload jsonb,status text,attempts integer,response_status integer,response text,next_attempt_at timestamptz,lease_until timestamptz,created_at timestamptz);
   CREATE TABLE "${schema}".suppressions(user_id text,email text,reason text,created_at timestamptz);
   CREATE TABLE "${schema}".request_logs(id text,user_id text,method text,status integer,duration_ms integer,request_body jsonb,response_body jsonb,created_at timestamptz);
   CREATE TABLE "${schema}".api_keys(user_id text);
   CREATE TABLE "${schema}".idempotency_keys(user_id text);
   CREATE TABLE "${schema}".organization_accounts(account_id text);`);
      await f.db.query(
        `INSERT INTO "${schema}".domains VALUES('fixture-domain','source-account',$1,now())`,
        [f.domainA.name],
      );
      for (const [id, state, mid] of [
        ["email1", "delivered", "2220893013169782784"],
        ["email2", "queued", null],
        ["email3", "scheduled", null],
      ]) {
        const from = "sender@" + f.domainA.name;
        await f.db.query(
          `INSERT INTO "${schema}".emails VALUES($1,'source-account',$2,$3,ARRAY['recipient@example.invalid'],'Imported',$4,$5,$6,now(),CASE WHEN $6='scheduled' THEN now()+interval '1 day' ELSE NULL END,now())`,
          [
            id,
            f.domainA.name,
            from,
            JSON.stringify({
              from,
              to: ["recipient@example.invalid"],
              subject: "Imported",
              text: "source-private-body",
              attachments:
                state === "scheduled"
                  ? [{ filename: "test.txt", content: "Zg==" }]
                  : [{ filename: "test.txt" }],
            }),
            mid,
            state,
          ],
        );
      }
      await f.db.query(
        `INSERT INTO "${schema}".email_events VALUES('event1','email1','delivered','recipient@example.invalid',now())`,
      );
      const secret = "whsec_" + Buffer.alloc(24, 1).toString("base64");
      await f.db.query(
        `INSERT INTO "${schema}".webhooks VALUES('hook1','source-account','https://example.com/hook',$1,ARRAY['email.delivered'],now())`,
        [secret],
      );
      await f.db.query(
        `INSERT INTO "${schema}".webhook_deliveries VALUES('delivery1','hook1','email.delivered','{"data":{"email_id":"email1"}}','pending',0,NULL,NULL,now(),NULL,now())`,
      );
      await f.db.query(
        `INSERT INTO "${schema}".suppressions VALUES('source-account','blocked@example.invalid','manual',now())`,
      );
      await f.db.query(
        `INSERT INTO "${schema}".request_logs VALUES('log1','source-account','POST',200,12,'{"text":"private-log-body"}','{}',now()),('log2','source-account','POST',200,12,'{}','{}',now()-interval '31 days')`,
      );
      await f.db.query(
        `INSERT INTO "${schema}".organization_accounts VALUES('source-account')`,
      );
      // Retention starts at handoff, never at a later open; suppressed original recipients never count as accepted.
      await f.db.query(
        `UPDATE "${schema}".emails SET "to"=ARRAY['recipient@example.invalid','blocked@example.invalid'],last_event='opened',created_at=now()-interval '60 days' WHERE id='email1'`,
      );
      await f.db.query(
        `UPDATE "${schema}".email_events SET created_at=now()-interval '60 days' WHERE id='event1'`,
      );
      await f.db.query(
        `INSERT INTO "${schema}".email_events VALUES('sent1','email1','sent',NULL,now()-interval '60 days'),('open1','email1','opened','recipient@example.invalid',now()),('suppressed1','email1','suppressed','blocked@example.invalid',now()-interval '60 days')`,
      );
      await f.db.query(
        `INSERT INTO "${schema}".emails SELECT 'email4',user_id,domain,"from",ARRAY['recipient@example.invalid'],subject,payload,'2220893013169782785','sent',now()-interval '100 days',NULL,now()-interval '100 days' FROM "${schema}".emails WHERE id='email1'`,
      );
      await f.db.query(
        `INSERT INTO "${schema}".webhook_deliveries VALUES('delivery2','hook1','email.sent','{"data":{"email_id":"email4"}}','pending',0,NULL,NULL,now(),NULL,now()-interval '100 days')`,
      );
      const historical = f.suffix + ".deleted.example.invalid";
      await f.db.query(
        `INSERT INTO "${schema}".emails SELECT 'email5',user_id,$1,$2,ARRAY['recipient@example.invalid'],subject,payload,'2220893013169782786','sent',now(),NULL,now() FROM "${schema}".emails WHERE id='email1'`,
        [historical, "sender@" + historical],
      );
      await f.db.query(
        `INSERT INTO "${schema}".webhook_deliveries VALUES('delivery3','hook1','domain.deleted','{"data":{"id":"deleted-provider"}}','pending',0,NULL,NULL,now(),NULL,now())`,
      );
      await f.db.query(
        `UPDATE webdock_auth.platform_settings SET settings=settings||'{"mailGatewayEnabled":false,"mailWebhooksEnabled":false}' WHERE id=true`,
      );
      await f.db.query(
        `UPDATE webdock_auth.mail_connection SET state='paused',provider_account_id=$2 WHERE id=$1`,
        [f.actorA.connectionID, f.customers[0]],
      );
      const mapping = {
        version: 1,
        sourceID: schema,
        providerAccountID: f.customers[0],
        executorStopped: true,
        accounts: [
          {
            legacyAccountID: "source-account",
            customerID: f.customers[0],
            historicalConnectionID: f.actorA.connectionID,
          },
        ],
      };
      const run = (dryRun: boolean) =>
        importRelaykit({
          sourceReadOnlyURL: process.env.DATABASE_URL!,
          sourceSchema: schema,
          mapping,
          dryRun,
        });
      const dry = await run(true);
      assert.equal(dry.expired, 1);
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_auth.mail_message WHERE connection_id=$1",
            [f.actorA.connectionID],
          )
        ).rows[0].count,
        "0",
      );
      const first = await run(false),
        second = await run(false);
      assert.equal(second.imported, 0);
      assert.equal(second.existing, first.imported);
      assert.equal(first.checksum, second.checksum);
      const messages = (
        await f.db.query(
          `SELECT id,dispatch_status,provider_mid FROM webdock_auth.mail_message WHERE connection_id=$1 ORDER BY principal->>'legacyID'`,
          [f.actorA.connectionID],
        )
      ).rows;
      assert.deepEqual(
        messages.map((m) => m.dispatch_status),
        ["accepted", "outcome_unknown", "scheduled", "accepted", "accepted"],
      );
      assert.equal(messages[0].provider_mid, "2220893013169782784");
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_auth.mail_message_content WHERE message_id=ANY($1)",
            [[messages[0].id, messages[3].id]],
          )
        ).rows[0].count,
        "0",
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT recipients FROM webdock_auth.mail_usage_reservation WHERE message_id=$1",
            [messages[0].id],
          )
        ).rows[0].recipients,
        1,
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT included_in_handoff,suppression_reason FROM webdock_auth.mail_recipient WHERE message_id=$1 AND email='blocked@example.invalid'",
            [messages[0].id],
          )
        ).rows[0].included_in_handoff,
        false,
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT deleted_at IS NOT NULL AS deleted FROM webdock_auth.mail_sender_domain WHERE customer_id=$1 AND domain=$2",
            [f.customers[0], historical],
          )
        ).rows[0].deleted,
        true,
      );
      const hook = (
        await f.db.query(
          "SELECT * FROM webdock_auth.mail_webhook WHERE connection_id=$1",
          [f.actorA.connectionID],
        )
      ).rows[0];
      assert.equal(hook.enabled, false);
      assert.ok(!hook.encrypted_secret.includes(secret));
      assert.equal(
        decryptMailSecret<{ secret: string }>(
          hook.encrypted_secret,
          `mail-webhook:${f.customers[0]}:${f.actorA.connectionID}:${hook.id}:1`,
        ).secret,
        secret,
      );
      assert.ok(
        !JSON.stringify(
          (
            await f.db.query(
              "SELECT request_body FROM webdock_auth.mail_request_log WHERE connection_id=$1",
              [f.actorA.connectionID],
            )
          ).rows,
        ).includes("private-log-body"),
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_auth.mail_webhook_delivery WHERE connection_id=$1",
            [f.actorA.connectionID],
          )
        ).rows[0].count,
        "3",
      );
      await f.db.query(
        `UPDATE "${schema}".emails SET subject='changed' WHERE id='email1'`,
      );
      await assert.rejects(run(false), { code: "source_changed" });
      await assert.rejects(
        importRelaykit({
          sourceReadOnlyURL: process.env.DATABASE_URL!,
          sourceSchema: schema,
          mapping: {
            ...mapping,
            accounts: [
              ...mapping.accounts,
              {
                legacyAccountID: "other",
                customerID: f.customers[1],
                historicalConnectionID: f.actorB.connectionID,
              },
            ],
          },
          dryRun: true,
        }),
        { code: "shared_provider_split_required" },
      );
    } finally {
      await f.db.query(`DROP SCHEMA "${schema}" CASCADE`);
    }
  }));
