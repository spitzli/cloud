import test from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";
import { database } from "../../src/db/runtime";
import { transaction } from "../../src/modules/hosting/db";
import { mailboxSchemaSQL } from "../../src/modules/mail/mailbox-schema";
import { mailSendSchemaSQL } from "../../src/modules/mail/mail-send-schema";
import { mailBudgetSchemaSQL } from "../../src/modules/mail/mail-budget";
import { nativeMailSchemaSQL } from "@webdock/mail-core/schema";
import { outboxSchemaSQL } from "@webdock/mail-core/outbox";
import { sealSecret } from "@webdock/mail-core/secrets";
import {
  launchWebmail,
  exchangeWebmail,
  inspectWebmail,
} from "../../src/modules/mail/mail-sessions";
import { mailGateway } from "../../src/modules/mail/mail-gateway";
import { createMailAPI, MailControlError } from "../../../mail/src/api/server";
import { PrivatePayloadStore } from "../../../mail/src/sending/payload-store";
import { processQueuedSend } from "../../../mail/src/sending/queued-worker";
import type { WebmailClientFactory } from "../../../mail/src/api/webmail-sending";
const url = new URL(process.env.DATABASE_URL!);
if (
  url.hostname !== "127.0.0.1" ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(() => database.end());
test("Webmail queues once, survives logout, authorizes DATA and repairs Sent without resending", async () => {
  for (const sql of [
    nativeMailSchemaSQL,
    mailboxSchemaSQL,
    mailSendSchemaSQL,
    mailBudgetSchemaSQL,
    outboxSchemaSQL,
  ])
    await database.query(sql);
  const before = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const original = { ...process.env };
  const nonce = randomBytes(6).toString("hex"),
    key = randomBytes(32).toString("base64");
  Object.assign(process.env, {
    WEBDOCK_NATIVE_MAIL_SENDING_ENABLED: "true",
    WEBDOCK_NATIVE_MAILBOXES_ENABLED: "true",
    MAIL_ENCRYPTION_KEY: key,
    MAIL_GATEWAY_SECRET: "g".repeat(43),
    MAIL_SEND_WORKER_SECRET: "w".repeat(43),
    MAIL_GATEWAY_ORIGIN: "https://mail.example.test",
    MAIL_MANAGED_CLUSTER_ID: "local-test",
    MAIL_HOSTNAME_SUFFIX: "example.test",
  });
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Queued Webmail fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    )
  ).rows[0].organization_id;
  const user = (
    await database.query(
      'INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"mustChangePassword") VALUES(\'Mail user\',$1,true,\'user\',false) RETURNING id',
      [nonce + "@example.invalid"],
    )
  ).rows[0].id;
  const session = async () =>
    (
      await database.query(
        'INSERT INTO webdock_auth.session("userId",token,"expiresAt","createdAt","updatedAt") VALUES($1,$2,now()+interval \'1 hour\',now(),now()) RETURNING id',
        [user, randomBytes(32).toString("hex")],
      )
    ).rows[0].id as string;
  const authSession = await session();
  const member = (
    await database.query(
      'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now()) RETURNING id',
      [org, user],
    )
  ).rows[0].id;
  const directory = await mkdtemp(join(tmpdir(), "webdock-webmail-send-")),
    store = new PrivatePayloadStore(directory, key);
  let server: ReturnType<typeof createMailAPI> | undefined;
  try {
    await database.query(
      `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"mailEnabled":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings`,
    );
    await database.query(
      "INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES($1,$2,'local-test',true,'ready',1,1)",
      [customer, "mail-" + customer],
    );
    const domainName = nonce + ".example.test",
      from = "support@" + domainName;
    const sender = (
      await database.query(
        "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status,provider_id,provider_snapshot) VALUES($1,$2,'fixture','ownership_verified','provider-domain',$3) RETURNING id",
        [
          customer,
          domainName,
          {
            id: "provider-domain",
            domain: domainName,
            spf_verified: true,
            dkim_verified: true,
            dmarc_verified: true,
          },
        ],
      )
    ).rows[0].id;
    const domain = (
      await database.query(
        "INSERT INTO webdock_mail.domain(customer_id,sender_domain_id,name,provider_id,state) VALUES($1,$2,$3,'domain','ready') RETURNING id",
        [customer, sender, domainName],
      )
    ).rows[0].id;
    const mailbox = (
      await database.query(
        "INSERT INTO webdock_mail.mailbox(customer_id,domain_id,local_part,name,quota_bytes,provider_id,state) VALUES($1,$2,'support','Support',10485760,'group-account','ready') RETURNING id",
        [customer, domain],
      )
    ).rows[0].id;
    const identity = (
      await database.query(
        "INSERT INTO webdock_mail.identity(customer_id,subject,provider_id,state) VALUES($1,$2,'user-account','ready') RETURNING id",
        [customer, user],
      )
    ).rows[0].id;
    await database.query(
      "UPDATE webdock_mail.identity SET encrypted_credentials=$2 WHERE id=$1",
      [
        identity,
        sealSecret(
          { username: "private-user", password: "private-password" },
          `identity:${customer}:${identity}`,
          key,
        ),
      ],
    );
    await database.query(
      "INSERT INTO webdock_mail.mailbox_grant(customer_id,mailbox_id,subject,member_id,applied_revision) VALUES($1,$2,$3,$4,1)",
      [customer, mailbox, user, member],
    );
    await database.query(
      `INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$1,$2,'ready','{"active":true}')`,
      [customer, nonce + "-relay@example.test"],
    );
    await database.query(
      `INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Fixture','Fixture','{"mailMessages":10}','{}',1,'fixture')`,
      [customer],
    );
    await database.query(
      "INSERT INTO webdock_mail.send_transport(customer_id,mode,state,region,provider_account_id,encrypted_credentials) VALUES($1,'managed','ready','eu',$1,$2)",
      [
        customer,
        sealSecret(
          { consumerKey: "fixture-key", consumerSecret: "fixture-secret" },
          `send-transport:${customer}:1`,
          key,
        ),
      ],
    );
    const hookToken = `wd_hook_${customer}_${randomBytes(32).toString("base64url")}`,
      smtpUsername = `wd-submit@mail-${customer}.example.test`;
    await database.query(
      `INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at,smtp_certificate_sha256,smtp_certificate_revision,encrypted_submission_credentials,submission_hook_token_hash,submission_transport_revision,submission_ready_revision,submission_state)
   VALUES($1,'fixture',now(),$2,1,$3,$4,1,1,'ready')`,
      [
        customer,
        "a".repeat(64),
        sealSecret(
          {
            username: smtpUsername,
            password: randomBytes(32).toString("base64url"),
            hookToken,
          },
          `submission:${customer}`,
          key,
        ),
        createHash("sha256").update(hookToken).digest("hex"),
      ],
    );
    const launch = await transaction((db) =>
      launchWebmail(
        db,
        { subject: user, sessionID: authSession },
        customer,
        mailbox,
      ),
    );
    const webmail = await transaction((db) => exchangeWebmail(db, launch.code));
    const control = async (body: Record<string, unknown>) => {
      const response = await mailGateway(
        new Request("https://auth.example.test/api/mail/gateway", {
          method: "POST",
          headers: {
            Authorization: "Bearer " + process.env.MAIL_SEND_WORKER_SECRET,
            "Content-Type": "application/json",
          },
          body: JSON.stringify(body),
        }),
      );
      if (!response.ok) throw new MailControlError(response.status);
      return ((await response.json()) as { data: unknown }).data;
    };
    let exports = 0,
      sends = 0,
      filings = 0,
      filingUnavailable = true;
    const factory: WebmailClientFactory = (access) => {
      assert.equal(access.customerID, customer);
      assert.equal(access.mailboxID, mailbox);
      assert.equal(access.accountID, "group-account");
      return {
        exportDraft: async (emailID, identityID) => {
          exports++;
          assert.ok(["draft-one", "draft-two"].includes(emailID));
          assert.equal(identityID, "sender-identity");
          return {
            sender: { email: from, name: "Support" },
            to: ["recipient@example.test", "hidden@example.test"],
            blobId: "draft-blob",
            raw: Buffer.from(
              `From: ${from}\r\nTo: recipient@example.test\r\nBcc: hidden@example.test\r\nSubject: Queued Webmail\r\nMessage-ID: <${emailID}@example.test>\r\n\r\nPrivate content\r\n`,
            ),
          };
        },
        fileSent: async (emailID) => {
          filings++;
          assert.ok(["draft-one", "draft-two"].includes(emailID));
          if (filingUnavailable) throw Error("Filing temporarily unavailable");
          return { state: "filed" };
        },
      };
    };
    server = createMailAPI(control, store, {
      webmailSecret: process.env.MAIL_GATEWAY_SECRET,
      clientFactory: factory,
    });
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    const origin = `http://127.0.0.1:${(server.address() as { port: number }).port}`;
    const invoke = (body: unknown) =>
      fetch(origin + "/internal/webmail", {
        method: "POST",
        headers: {
          Authorization: "Bearer " + process.env.MAIL_GATEWAY_SECRET,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(body),
      });
    const input = {
      action: "submit",
      token: webmail.token,
      requestID: crypto.randomUUID(),
      emailID: "draft-one",
      identityID: "sender-identity",
    };
    const first = await invoke(input);
    assert.equal(first.status, 200);
    const receipt = (await first.json()) as { id: string; state: string };
    assert.equal(receipt.state, "queued");
    const repeat = await invoke({ ...input, requestID: crypto.randomUUID() });
    assert.equal(((await repeat.json()) as { id: string }).id, receipt.id);
    assert.equal(exports, 1);
    await database.query("DELETE FROM webdock_auth.session WHERE id=$1", [
      authSession,
    ]);
    await assert.rejects(
      transaction((db) => inspectWebmail(db, webmail.token)),
    );
    assert.equal(
      await processQueuedSend(
        control,
        store,
        {
          smtp: async (options, message) => {
            sends++;
            assert.equal(options.username, smtpUsername);
            assert.deepEqual(message.to, [
              "recipient@example.test",
              "hidden@example.test",
            ]);
            assert.doesNotMatch(message.raw.toString(), /Bcc:/);
            const split = message.raw.indexOf("\r\n\r\n"),
              headers: [string, string][] = [];
            for (const line of message.raw
              .subarray(0, split)
              .toString()
              .split("\r\n")) {
              if (/^[ \t]/.test(line))
                headers[headers.length - 1][1] += "\r\n" + line;
              else {
                const colon = line.indexOf(":");
                headers.push([line.slice(0, colon), line.slice(colon + 1)]);
              }
            }
            const proof = await fetch(origin + "/internal/smtp/" + customer, {
              method: "POST",
              headers: {
                Authorization: "Bearer " + hookToken,
                "Content-Type": "application/json",
              },
              body: JSON.stringify({
                context: {
                  stage: "data",
                  sasl: { login: options.username },
                  queue: { id: "abc123" },
                },
                envelope: {
                  from: { address: message.from },
                  to: message.to.map((address) => ({ address })),
                },
                message: {
                  headers,
                  contents: message.raw.subarray(split + 4).toString(),
                  size: message.raw.length,
                },
              }),
            });
            assert.equal(proof.status, 200);
            assert.equal(
              ((await proof.json()) as { action: string }).action,
              "accept",
            );
            return { accepted: true };
          },
        },
        factory,
      ),
      true,
    );
    assert.equal(sends, 1);
    assert.equal(filings, 1);
    const state = (
      await database.query(
        "SELECT state,filing_state,payload_ref FROM webdock_mail.send_job WHERE id=$1",
        [receipt.id],
      )
    ).rows[0];
    assert.equal(state.state, "accepted");
    assert.equal(state.filing_state, "needs_review");
    assert.ok((await store.get(customer, state.payload_ref)).length);
    const renewed = await session();
    const reconnect = await transaction((db) =>
      launchWebmail(
        db,
        { subject: user, sessionID: renewed },
        customer,
        mailbox,
      ),
    );
    const fresh = await transaction((db) =>
      exchangeWebmail(db, reconnect.code),
    );
    filingUnavailable = false;
    const repaired = await invoke({
      action: "file",
      token: fresh.token,
      id: receipt.id,
    });
    assert.equal(repaired.status, 200);
    assert.equal(((await repaired.json()) as { filed: boolean }).filed, true);
    assert.equal(sends, 1);
    assert.equal(filings, 2);
    await assert.rejects(store.get(customer, state.payload_ref));
    assert.equal(await processQueuedSend(control, store), false);
    const revoked = await invoke({
      ...input,
      token: fresh.token,
      requestID: crypto.randomUUID(),
      emailID: "draft-two",
    });
    assert.equal(revoked.status, 200);
    const revokedReceipt = (await revoked.json()) as { id: string };
    await database.query(
      "UPDATE webdock_mail.mailbox_grant SET enabled=false,revision=revision+1 WHERE mailbox_id=$1 AND subject=$2",
      [mailbox, user],
    );
    assert.equal(
      await processQueuedSend(
        control,
        store,
        {
          smtp: async () => {
            sends++;
            throw Error("Revoked sender must not reach SMTP");
          },
        },
        factory,
      ),
      true,
    );
    assert.equal(sends, 1);
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.send_job WHERE id=$1",
          [revokedReceipt.id],
        )
      ).rows[0].state,
      "cancelled",
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.budget_reservation WHERE reference_id=$1",
          [revokedReceipt.id],
        )
      ).rows[0].state,
      "released",
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.webmail_write WHERE id=$1",
          [revokedReceipt.id],
        )
      ).rows[0].state,
      "rejected",
    );
    await database.query(
      "UPDATE webdock_mail.mailbox_grant SET enabled=true,applied_revision=revision WHERE mailbox_id=$1 AND subject=$2",
      [mailbox, user],
    );
    const refreshedLaunch = await transaction((db) =>
      launchWebmail(
        db,
        { subject: user, sessionID: renewed },
        customer,
        mailbox,
      ),
    );
    const refreshed = await transaction((db) =>
      exchangeWebmail(db, refreshedLaunch.code),
    );
    const retry = await invoke({
      ...input,
      token: refreshed.token,
      requestID: crypto.randomUUID(),
      emailID: "draft-two",
    });
    assert.equal(retry.status, 200);
    const retried = (await retry.json()) as { id: string };
    assert.notEqual(retried.id, revokedReceipt.id);
    const cancelled = await invoke({
      action: "cancel",
      token: refreshed.token,
      id: retried.id,
    });
    assert.equal(
      ((await cancelled.json()) as { state: string }).state,
      "cancelled",
    );
  } finally {
    if (server) {
      server.closeAllConnections();
      await new Promise<void>((resolve) => server!.close(() => resolve()));
    }
    await rm(directory, { recursive: true, force: true });
    for (const table of [
      "send_job",
      "budget_reservation",
      "month_usage",
      "send_transport",
      "webmail_write",
      "webmail_session",
      "webmail_launch",
      "mailbox_grant",
      "identity",
      "mailbox",
      "domain",
      "configuration_operation",
      "configuration",
      "instance",
      "service",
    ])
      await database.query(
        `DELETE FROM webdock_mail.${table} WHERE customer_id=$1`,
        [customer],
      );
    for (const table of [
      "mail_sender_domain",
      "mail_tenant_account",
      "tenant_subscription",
      "tenant_customer",
    ])
      await database.query(
        `DELETE FROM webdock_auth.${table} WHERE customer_id=$1`,
        [customer],
      );
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customer,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      org,
    ]);
    await database.query('DELETE FROM webdock_auth."user" WHERE id=$1', [user]);
    if (before)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [before.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
    for (const name of [
      "WEBDOCK_NATIVE_MAIL_SENDING_ENABLED",
      "WEBDOCK_NATIVE_MAILBOXES_ENABLED",
      "MAIL_ENCRYPTION_KEY",
      "MAIL_GATEWAY_SECRET",
      "MAIL_SEND_WORKER_SECRET",
      "MAIL_GATEWAY_ORIGIN",
      "MAIL_MANAGED_CLUSTER_ID",
      "MAIL_HOSTNAME_SUFFIX",
    ]) {
      if (original[name] === undefined) delete process.env[name];
      else process.env[name] = original[name];
    }
  }
});
