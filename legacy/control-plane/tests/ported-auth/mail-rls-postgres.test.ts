import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomBytes, createHash } from "node:crypto";
import pg from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
test(
  "Mail server paths work as constrained session/key/worker roles without external sends",
  { skip: !fixtureFile },
  async () => {
    const ownerURL = process.env.DATABASE_URL!,
      url = new URL(ownerURL);
    assert.ok(["localhost", "127.0.0.1"].includes(url.hostname));
    assert.equal(url.pathname, "/webdock_rls_test");
    const fixture = JSON.parse(readFileSync(fixtureFile!, "utf8"));
    Object.assign(process.env, {
      DATABASE_URL: fixture.roles.webdock_auth_runtime,
      AUTH_DATABASE_URL: fixture.roles.webdock_auth_bootstrap,
      DATABASE_WORKER_URL: fixture.roles.webdock_worker_runtime,
      DATABASE_RLS_SECRET: fixture.contextSecret,
      BETTER_AUTH_SECRET: "synthetic-mail-rls-key-".repeat(3),
    });
    const { database, authDatabase, workerDatabase } =
      await import("../../src/db/runtime");
    const { encryptMailSecret, mailProvider, getMailConnectionStatus } =
      await import("../../src/modules/platform/platform");
    const { mailEndpoint } =
      await import("../../src/modules/mail/product/http");
    const { submitMail } =
      await import("../../src/modules/mail/product/sending");
    const { authorizeMailKey } =
      await import("../../src/modules/mail/product/api-keys");
    const { resolveSendingCredential } =
      await import("../../src/modules/mail/product/provider-credentials");
    const { withMailDatabase } = await import("../../src/db/database-context");
    const { assertQuotaCapacity } =
      await import("../../src/modules/mail/product/quota");
    const { withMailTransaction, mailDatabase } =
      await import("../../src/modules/mail/product/store");
    const { withMailDeadline } =
      await import("../../src/modules/mail/product/deadline");
    const { acceptProviderEvents } =
      await import("../../src/modules/mail/product/callback-inbox");
    const { getMailService, requestMailService } =
      await import("@webdock/mail-core");
    const owner = new pg.Pool({ connectionString: ownerURL, max: 1 });
    const base = BigInt(
        "93" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
      ),
      id = (i: number) => (base + BigInt(i)).toString();
    const customer = id(0),
      subject = id(1),
      session = id(2),
      connection = id(3),
      byok = id(4),
      keyID = id(5),
      byokKeyID = id(6),
      foreign = id(7),
      domainID = id(10),
      operator = id(11),
      operatorSession = id(12),
      source = id(13),
      webhook = id(14);
    const domain = id(15) + ".example.invalid",
      pair = {
        consumerKey: "synthetic-consumer",
        consumerSecret: "synthetic-secret",
      };
    const key = "wdm_" + randomBytes(32).toString("base64url"),
      byokKey = "wdm_" + randomBytes(32).toString("base64url"),
      callbackSecret = randomBytes(32).toString("hex");
    const hash = (s: string) => createHash("sha256").update(s).digest("hex");
    const asUser = <T>(run: () => T) =>
      withDatabaseScope({ kind: "session", subject, sessionID: session }, run);
    const asOperator = <T>(run: () => T) =>
      withDatabaseScope(
        { kind: "session", subject: operator, sessionID: operatorSession },
        run,
      );
    const oldSettings = (
      await owner.query("SELECT * FROM webdock_auth.platform_settings WHERE id")
    ).rows[0];
    const oldCredential = (
      await owner.query(
        "SELECT * FROM webdock_auth.platform_mail_credential WHERE id",
      )
    ).rows[0];
    const orgs: string[] = [];
    let sendCalls = 0;
    try {
      for (const [user, sid, role] of [
        [subject, session, "user"],
        [operator, operatorSession, "operator"],
        [id(19), id(20), "user"],
      ]) {
        await owner.query(
          'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,$1,$2,$3,true,false,true)',
          [user, user + "@rls.invalid", role],
        );
        await owner.query(
          'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
          [sid, user],
        );
      }
      for (const cid of [customer, foreign]) {
        await owner.query(
          "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,$1,'active')",
          [cid],
        );
        orgs.push(
          (
            await owner.query(
              "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
              [cid],
            )
          ).rows[0].organization_id,
        );
      }
      await owner.query(
        'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'owner\',now())',
        [orgs[0], subject],
      );
      await owner.query(
        'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,$3,now())',
        [orgs[0], id(19), "member"],
      );
      await owner.query(
        "INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,$1) ON CONFLICT(id) DO UPDATE SET settings=excluded.settings",
        [
          {
            ...oldSettings?.settings,
            mailEnabled: true,
            mailGatewayEnabled: true,
            mailDispatchEnabled: true,
            mailWebhooksEnabled: true,
            mailDefaultLimit: 1000,
          },
        ],
      );
      await owner.query(
        "INSERT INTO webdock_auth.platform_mail_credential(id,encrypted_secret,key_suffix,updated_by) VALUES(true,$1,'test',$2) ON CONFLICT(id) DO UPDATE SET encrypted_secret=excluded.encrypted_secret,key_suffix=excluded.key_suffix,updated_by=excluded.updated_by",
        [encryptMailSecret(pair, "platform-master"), operator],
      );
      await owner.query(
        "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready','{\"active\":true}')",
        [customer, connection, subject + "@rls.invalid"],
      );
      for (const [cid, kind] of [
        [connection, "platform"],
        [byok, "byok"],
      ])
        await owner.query(
          "INSERT INTO webdock_auth.mail_connection(id,customer_id,kind,provider_account_id,state) VALUES($1,$2,$3,$1,'ready')",
          [cid, customer, kind],
        );
      for (const [kid, cid, value] of [
        [keyID, connection, key],
        [byokKeyID, byok, byokKey],
      ])
        await owner.query(
          "INSERT INTO webdock_auth.mail_api_key(id,customer_id,connection_id,name,prefix,key_hash,permission) VALUES($1,$2,$3,'synthetic','wdm_test',$4,'sending_access')",
          [kid, customer, cid, hash(value)],
        );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_byok_policy(customer_id,policy) VALUES($1,$2)",
        [
          customer,
          {
            kubernetes: false,
            vercel: false,
            turbosmtp: true,
            maxClusters: 0,
            manageExisting: false,
            namespaces: [],
          },
        ],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_turbosmtp(customer_id,label,encrypted_secret) VALUES($1,'synthetic',$2)",
        [customer, encryptMailSecret(pair, "byok-mail:" + customer)],
      );
      await owner.query(
        "INSERT INTO webdock_auth.mail_connection_secret(customer_id,connection_id,version,encrypted_secret,key_suffix,active) VALUES($1,$2,1,$3,'test',true)",
        [
          customer,
          byok,
          encryptMailSecret(pair, `mail-send:${customer}:${byok}:1`),
        ],
      );
      const snapshot = {
        id: "synthetic-domain",
        domain,
        spf_verified: true,
        dkim_verified: true,
        dmarc_verified: true,
      };
      await owner.query(
        "INSERT INTO webdock_auth.mail_sender_domain(id,customer_id,domain,token,status,provider_id) VALUES($1,$2,$3,'synthetic','ownership_verified','synthetic-domain')",
        [domainID, customer, domain],
      );
      await owner.query(
        "INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id,provider_id,provider_snapshot,provider_checked_at) VALUES($1,$2,$3,'synthetic-domain',$4,now())",
        [customer, connection, domainID, snapshot],
      );
      await owner.query(
        "INSERT INTO webdock_auth.mail_webhook(id,customer_id,connection_id,url,events,encrypted_secret) VALUES($1,$2,$3,'https://example.invalid/callback',$4,'synthetic')",
        [
          webhook,
          customer,
          connection,
          JSON.stringify(["email.sent", "email.delivered", "email.scheduled"]),
        ],
      );
      await owner.query(
        "INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'test','test',$2,$3,1,$4)",
        [customer, { mailMessages: 10 }, { mailMessages: 0 }, operator],
      );
      const period = new Date().toISOString().slice(0, 7) + "-01";
      await owner.query(
        "INSERT INTO webdock_auth.mail_usage_period(customer_id,connection_id,period,budget,consumed) VALUES($1,$2,$3,10,2),($1,$4,$3,10,8)",
        [customer, connection, period, byok],
      );
      assert.deepEqual(await asUser(() => getMailConnectionStatus()), {
        configured: true,
        consumerKeySuffix: "test",
      });
      assert.equal(
        (
          await asUser(() =>
            database.query(
              "SELECT encrypted_secret FROM webdock_auth.platform_mail_credential",
            ),
          )
        ).rowCount,
        0,
        "master credential table stays private",
      );
      assert.ok(
        await asUser(() =>
          mailProvider(
            async () => {
              throw Error("External access forbidden");
            },
            customer,
            connection,
          ),
        ),
      );
      await assert.rejects(
        asUser(() => mailProvider(undefined, foreign)),
        /authorization required/,
      );
      await assert.rejects(
        database.query("SELECT webdock_rls.mail_usage_total($1,$2)", [
          customer,
          period,
        ]),
        /authorization required/,
      );
      const actor = await authorizeMailKey(
        new Request("https://auth.invalid/mail", {
          headers: { Authorization: "Bearer " + key },
        }),
        "sending_access",
      );
      await owner.query(
        "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
        [customer],
      );
      await withMailDatabase(actor, async () =>
        assert.equal(
          (await database.query("SELECT id FROM webdock_auth.mail_connection"))
            .rowCount,
          0,
          "archived customer invalidates signed key scope",
        ),
      );
      await owner.query(
        "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
        [customer],
      );
      await owner.query(
        "UPDATE webdock_auth.mail_connection SET deleted_at=now() WHERE id=$1",
        [connection],
      );
      await withMailDatabase(actor, async () =>
        assert.equal(
          (await database.query("SELECT id FROM webdock_auth.mail_connection"))
            .rowCount,
          0,
          "deleted connection invalidates signed key scope",
        ),
      );
      await owner.query(
        "UPDATE webdock_auth.mail_connection SET deleted_at=NULL WHERE id=$1",
        [connection],
      );
      await withMailDatabase(actor, async () => {
        assert.equal(
          (
            await database.query(
              "SELECT consumed FROM webdock_auth.mail_usage_period",
            )
          ).rowCount,
          1,
          "other connection rows remain hidden",
        );
        assert.equal(
          (
            await database.query(
              "SELECT webdock_rls.mail_usage_total($1,$2) AS used",
              [customer, period],
            )
          ).rows[0].used,
          "10",
        );
        await assert.rejects(
          withMailTransaction((db) =>
            assertQuotaCapacity(db, customer, 1, new Date()),
          ),
          /Monthly recipient allowance exhausted/,
        );
        await database.query(
          "SELECT webdock_rls.mail_refresh_domain($1,$2,$3,$4)",
          [customer, connection, domainID, snapshot],
        );
        await assert.rejects(
          database.query(
            "UPDATE webdock_auth.mail_connection_domain SET provider_id='forged' WHERE connection_id=$1",
            [connection],
          ),
          /row-level security/,
        );
        await assert.rejects(
          database.query(
            "SELECT webdock_rls.mail_refresh_domain($1,$2,$3,$4)",
            [foreign, connection, domainID, snapshot],
          ),
          /authorization required/,
        );
      });
      const byokActor = await authorizeMailKey(
        new Request("https://auth.invalid/mail", {
          headers: { Authorization: "Bearer " + byokKey },
        }),
        "sending_access",
      );
      await withMailDatabase(byokActor, async () =>
        assert.deepEqual(await resolveSendingCredential(byokActor), {
          ...pair,
          region: "eu",
        }),
      );
      const endpoint = mailEndpoint(
        "sending_access",
        async (_request, who, { body }) =>
          submitMail(who, body as any, {
            key: randomBytes(20).toString("hex"),
            send: async () => {
              sendCalls++;
              return { kind: "accepted", mid: "synthetic" };
            },
          }),
        "/emails",
      );
      const response = await endpoint(
        new Request("https://auth.invalid/api/mail/v1/emails", {
          method: "POST",
          headers: {
            Authorization: "Bearer " + key,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            from: "sender@" + domain,
            to: "delivered@test.mail.webdock.dev",
            subject: "RLS simulator",
            text: "Synthetic",
          }),
        }),
      );
      const result = await response.json();
      assert.equal(response.status, 200, JSON.stringify(result));
      assert.equal(sendCalls, 0);
      assert.ok(
        Number(
          (
            await owner.query(
              "SELECT count(*) FROM webdock_auth.mail_event WHERE customer_id=$1",
              [customer],
            )
          ).rows[0].count,
        ) >= 2,
      );
      assert.ok(
        Number(
          (
            await owner.query(
              "SELECT count(*) FROM webdock_auth.mail_webhook_delivery WHERE customer_id=$1",
              [customer],
            )
          ).rows[0].count,
        ) >= 2,
      );
      await withDatabaseScope(
        { kind: "session", subject: id(19), sessionID: id(20) },
        async () => {
          const sent = await submitMail(
            {
              customerID: customer,
              connectionID: connection,
              principal: { kind: "session", userID: id(19), sessionID: id(20) },
            },
            {
              from: "sender@" + domain,
              to: "delivered@test.mail.webdock.dev",
              subject: "Member simulation",
              text: "Synthetic",
            },
            {
              key: randomBytes(20).toString("hex"),
              send: async () => {
                throw Error("External send forbidden");
              },
            },
          );
          assert.equal(sent.status, 200, JSON.stringify(sent.body));
          await assert.rejects(
            database.query(
              "UPDATE webdock_auth.mail_connection SET state='paused' WHERE id=$1",
              [connection],
            ),
            /row-level security/,
          );
        },
      );
      await withDatabaseScope(
        { kind: "system", purpose: "mail-cron" },
        async () => {
          const revision = (
            await owner.query(
              "SELECT revision FROM webdock_auth.mail_connection WHERE id=$1",
              [connection],
            )
          ).rows[0].revision;
          const baseline = (await mailDatabase.query("SHOW statement_timeout"))
            .rows[0].statement_timeout;
          const local = (
            await withMailDeadline(new Date(Date.now() + 500), () =>
              mailDatabase.query("SHOW statement_timeout"),
            )
          ).rows[0].statement_timeout;
          assert.notEqual(local, baseline);
          assert.equal(
            (await mailDatabase.query("SHOW statement_timeout")).rows[0]
              .statement_timeout,
            baseline,
            "deadline timeout cannot leak to pooled sessions",
          );
          await assert.rejects(
            withMailDeadline(new Date(Date.now() + 60), () =>
              mailDatabase.query(
                "WITH pause AS MATERIALIZED(SELECT pg_sleep(0.25)) UPDATE webdock_auth.mail_connection SET revision=revision+1 FROM pause WHERE id=$1",
                [connection],
              ),
            ),
          );
          await assert.rejects(
            withMailDeadline(new Date(Date.now() + 60), () =>
              withMailTransaction(async (db) => {
                await db.query(
                  "UPDATE webdock_auth.mail_connection SET revision=revision+1 WHERE id=$1",
                  [connection],
                );
                await db.query("SELECT pg_sleep(0.25)");
              }),
            ),
          );
          await new Promise((resolve) => setTimeout(resolve, 350));
          assert.equal(
            (
              await owner.query(
                "SELECT revision FROM webdock_auth.mail_connection WHERE id=$1",
                [connection],
              )
            ).rows[0].revision,
            revision,
            "no late standalone commit or unrolled-back explicit write",
          );
        },
      );
      await owner.query(
        "INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,applied_revision) VALUES($1,$1,'fixture',true,'ready',1)",
        [customer],
      );
      await owner.query(
        "INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at) VALUES($1,'synthetic-retained-credentials',now())",
        [customer],
      );
      for (const clusterID of [id(17), id(18)])
        await owner.query(
          "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES($1::text,$1::text,'local','test','synthetic')",
          [clusterID],
        );
      await owner.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,$1,$2,'active')",
        [id(16), customer],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode) VALUES($1,$2,'k3s',$3,$4,'managed')",
        [id(16), customer, id(17), "rls-" + customer],
      );
      await owner.query(
        "INSERT INTO webdock_mail.cluster_reservation(customer_id,cluster_id,demand,status) VALUES($1,$2,'{}','active')",
        [customer, id(18)],
      );
      for (const [clusterID, count] of [
        [id(17), 0],
        [id(18), 1],
      ] as const)
        await withDatabaseScope(
          { kind: "system", purpose: "agent", clusterID },
          async () =>
            assert.equal(
              (
                await database.query(
                  "SELECT customer_id FROM webdock_mail.instance WHERE customer_id=$1",
                  [customer],
                )
              ).rowCount,
              count,
            ),
        );
      assert.equal(
        (await asUser(() => getMailService(database, customer))).state,
        "ready",
      );
      await withMailDatabase(actor, async () =>
        assert.equal(
          (
            await database.query(
              "SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1 FOR SHARE",
              [customer],
            )
          ).rowCount,
          1,
        ),
      );
      await withDatabaseScope(
        { kind: "system", purpose: "mail-cron" },
        async () =>
          assert.equal(
            (
              await database.query(
                "SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1",
                [customer],
              )
            ).rowCount,
            1,
          ),
      );
      const blocked = await endpoint(
        new Request("https://auth.invalid/api/mail/v1/emails", {
          method: "POST",
          headers: {
            Authorization: "Bearer " + key,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            from: "sender@" + domain,
            to: "recipient@example.invalid",
            subject: "Must use native",
            text: "Synthetic",
          }),
        }),
      );
      assert.notEqual(blocked.status, 200);
      assert.equal(sendCalls, 0);
      assert.equal(
        (
          await owner.query(
            "SELECT count(*)::int AS n FROM webdock_auth.mail_message WHERE customer_id=$1 AND error_code='native_submission_required'",
            [customer],
          )
        ).rows[0].n,
        1,
      );
      await assert.rejects(
        asUser(() =>
          requestMailService(database, {
            customerID: customer,
            enabled: false,
            actorID: subject,
            expectedRevision: "1",
            hostID: "fixture",
          }),
        ),
        /operator/,
      );
      const suspended = await asOperator(() =>
        requestMailService(database, {
          customerID: customer,
          enabled: false,
          actorID: operator,
          expectedRevision: "1",
          hostID: "fixture",
        }),
      );
      assert.equal(suspended.enabled, false);
      assert.equal(
        (
          await owner.query(
            "SELECT encrypted_credentials FROM webdock_mail.instance WHERE customer_id=$1",
            [customer],
          )
        ).rows[0].encrypted_credentials,
        "synthetic-retained-credentials",
      );
      await owner.query(
        "INSERT INTO webdock_auth.mail_callback_source(id,secret_hash,kind,enabled,verified_at) VALUES($1,$2,'account',true,now())",
        [source, hash(callbackSecret)],
      );
      const callback = await acceptProviderEvents(
        source,
        new Request("https://auth.invalid/api/mail/provider-events/" + source, {
          method: "POST",
          headers: {
            Authorization:
              "Basic " +
              Buffer.from("webdock:" + callbackSecret).toString("base64"),
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            mid: "12345",
            status: "DELIVERED",
            email: "recipient@example.invalid",
            timestamp: Math.floor(Date.now() / 1000),
          }),
        }),
      );
      assert.equal(callback.status, 200);
      await withDatabaseScope(
        { kind: "system", purpose: "mail-cron" },
        async () =>
          assert.equal(
            (
              await database.query(
                "SELECT id FROM webdock_auth.mail_callback_inbox WHERE source_id=$1 FOR UPDATE",
                [source],
              )
            ).rowCount,
            1,
          ),
      );
      assert.equal(sendCalls, 0);
    } finally {
      const cleanup = await owner.connect();
      try {
        await cleanup.query("BEGIN");
        await cleanup.query("SET LOCAL session_replication_role='replica'");
        const tables = (
          await cleanup.query(
            "SELECT table_schema,table_name FROM information_schema.columns WHERE column_name='customer_id' AND table_schema IN ('webdock_auth','webdock_mail')",
          )
        ).rows;
        for (const t of tables)
          await cleanup.query(
            `DELETE FROM "${t.table_schema}"."${t.table_name}" WHERE customer_id=ANY($1)`,
            [[customer, foreign]],
          );
        await cleanup.query(
          "DELETE FROM webdock_auth.mail_callback_inbox WHERE source_id=$1",
          [source],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.mail_callback_source WHERE id=$1",
          [source],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.access_event WHERE actor_id=ANY($1)",
          [[subject, operator, id(19)]],
        );
        await cleanup.query(
          'DELETE FROM webdock_auth.member WHERE "userId"=ANY($1)',
          [[subject, operator, id(19)]],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
          [orgs],
        );
        await cleanup.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
          id(16),
        ]);
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_cluster WHERE id=ANY($1)",
          [[id(17), id(18)]],
        );
        await cleanup.query(
          "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
          [[customer, foreign]],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.session WHERE id=ANY($1)",
          [[session, operatorSession, id(20)]],
        );
        await cleanup.query(
          'DELETE FROM webdock_auth."user" WHERE id=ANY($1)',
          [[subject, operator, id(19)]],
        );
        if (oldSettings)
          await cleanup.query(
            "UPDATE webdock_auth.platform_settings SET settings=$1,updated_by=$2,updated_at=$3 WHERE id",
            [
              oldSettings.settings,
              oldSettings.updated_by,
              oldSettings.updated_at,
            ],
          );
        else
          await cleanup.query(
            "DELETE FROM webdock_auth.platform_settings WHERE id",
          );
        if (oldCredential)
          await cleanup.query(
            "UPDATE webdock_auth.platform_mail_credential SET encrypted_secret=$1,key_suffix=$2,updated_by=$3,updated_at=$4 WHERE id",
            [
              oldCredential.encrypted_secret,
              oldCredential.key_suffix,
              oldCredential.updated_by,
              oldCredential.updated_at,
            ],
          );
        else
          await cleanup.query(
            "DELETE FROM webdock_auth.platform_mail_credential WHERE id",
          );
        await cleanup.query("COMMIT");
      } catch (error) {
        await cleanup.query("ROLLBACK");
        throw error;
      } finally {
        cleanup.release();
        await Promise.all([
          owner.end(),
          database.end(),
          authDatabase.end(),
          workerDatabase.end(),
        ]);
      }
    }
  },
);
