import { mailboxSchemaSQL } from "../../src/modules/mail/mailbox-schema";
import {
  prepareSubmissionConfiguration,
  completeSubmissionConfiguration,
  reconcileSubmissionConfiguration,
} from "../../src/modules/mail/mail-submission-configuration";
import { encryptMailSecret } from "../../src/modules/platform/platform";
import test from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { database } from "../../src/db/runtime";
import { transaction } from "../../src/modules/hosting/db";
import { mailSendSchemaSQL } from "../../src/modules/mail/mail-send-schema";
import { outboxSchemaSQL, claimSend } from "@webdock/mail-core/outbox";
import { sealSecret } from "@webdock/mail-core/secrets";
import { mailBudgetSchemaSQL } from "../../src/modules/mail/mail-budget";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";
import { createMailAPI } from "../../../mail/src/api/server";
import { PrivatePayloadStore } from "../../../mail/src/sending/payload-store";
import { submitRelayAPI } from "../../../mail/src/sending/relay-api";
import { processQueuedSend } from "../../../mail/src/sending/queued-worker";
import type { SendLease } from "@webdock/mail-core/outbox";
import {
  authorizeSubmissionPolicy,
  preflightSubmission,
  admitSubmission as stageSubmission,
  commitSubmissionPayload,
  abortSubmissionPayload,
  claimSubmission,
  submissionCleanup,
  finishSubmissionCleanup,
  reviewSubmissionPreparation,
  beginSubmission,
  finishSubmission,
  submissionStatus,
} from "../../src/modules/mail/mail-submissions";
import { authorizeMailKey } from "../../src/modules/mail/mail-native-keys";
const target = new URL(process.env.DATABASE_URL!);
if (
  target.hostname !== "127.0.0.1" ||
  target.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(() => database.end());
async function admitSubmission(
  db: Parameters<typeof stageSubmission>[0],
  input: unknown,
) {
  const result = await stageSubmission(db, input);
  if ("uploadToken" in result && result.uploadToken && "customerID" in result)
    await commitSubmissionPayload(db, {
      id: result.id,
      customerID: result.customerID,
      payloadRef: result.payloadRef,
      uploadToken: result.uploadToken,
    });
  return result;
}

test("native sending isolates keys, stages private payloads and rechecks managed, BYOK and SMTP authorization", async () => {
  await database.query(mailboxSchemaSQL);
  await database.query(mailSendSchemaSQL);
  await database.query(mailBudgetSchemaSQL);
  await database.query(outboxSchemaSQL);
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Native key fixture') RETURNING id",
    )
  ).rows[0].id;
  const token = "wd_mail_" + randomBytes(32).toString("base64url");
  const before = (
    await database.query(
      "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
    )
  ).rows[0];
  const previous = process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED;
  process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED = "true";
  try {
    await database.query(
      `INSERT INTO webdock_auth.platform_settings(id,settings) VALUES(true,'{"mailEnabled":true}') ON CONFLICT(id) DO UPDATE SET settings=excluded.settings`,
    );
    process.env.MAIL_ENCRYPTION_KEY ??= randomBytes(32).toString("base64");
    const domain = (
      await database.query(
        "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status) VALUES($1,'native-key.example.test','fixture','ownership_verified') RETURNING id",
        [customer],
      )
    ).rows[0].id;
    const key = (
      await database.query(
        "INSERT INTO webdock_mail.api_key(customer_id,label,token_hash,key_suffix,domain_restricted,domain_id,created_by) VALUES($1,'Test',$2,'test',true,$3,'fixture') RETURNING id",
        [customer, createHash("sha256").update(token).digest("hex"), domain],
      )
    ).rows[0].id;
    const access = await transaction((db) =>
      authorizeMailKey(db, token, "sender@native-key.example.test"),
    );
    assert.equal(access.customerID, customer);
    assert.equal(access.keyID, key);
    await assert.rejects(
      transaction((db) =>
        authorizeMailKey(db, token, "sender@other.example.test"),
      ),
    );
    await assert.rejects(
      transaction((db) =>
        authorizeMailKey(db, token + "x", "sender@native-key.example.test"),
      ),
    );
    const providerKey = {
      consumerKey: "fixture-consumer",
      consumerSecret: "fixture-secret",
    };
    await database.query(
      `INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready','{"active":true}')`,
      [customer, customer, customer + "@example.test"],
    );
    await database.query(
      `UPDATE webdock_auth.mail_sender_domain SET provider_id=$2,provider_snapshot=$3 WHERE id=$1`,
      [
        domain,
        domain,
        {
          id: domain,
          domain: "native-key.example.test",
          spf_verified: true,
          dkim_verified: true,
          dmarc_verified: true,
        },
      ],
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
          providerKey,
          `send-transport:${customer}:1`,
          process.env.MAIL_ENCRYPTION_KEY!,
        ),
      ],
    );
    const command = {
      token,
      idempotencyKey: "fixture1",
      requestHash: "a".repeat(64),
      payloadRef: crypto.randomUUID(),
      payloadBytes: 10,
      envelope: {
        from: "sender@native-key.example.test",
        to: ["recipient@example.test"],
      },
    };
    const queued = await transaction((db) => admitSubmission(db, command));
    assert.equal(
      (await transaction((db) => admitSubmission(db, command))).id,
      queued.id,
    );
    assert.equal(
      (
        await database.query(
          "SELECT customer_id FROM webdock_mail.service WHERE customer_id=$1",
          [customer],
        )
      ).rowCount,
      0,
    );
    const lease = await transaction((db) => claimSend(db));
    assert.ok(lease);
    assert.equal(lease.customerID, customer);
    await database.query(
      "UPDATE webdock_mail.api_key SET revoked_at=now() WHERE id=$1",
      [key],
    );
    assert.equal(await transaction((db) => beginSubmission(db, lease)), null);
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.budget_reservation WHERE reference_id=$1",
          [queued.id],
        )
      ).rows[0].state,
      "released",
    );
    await database.query(
      "UPDATE webdock_mail.api_key SET revoked_at=NULL WHERE id=$1",
      [key],
    );
    const second = await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "fixture2",
        payloadRef: crypto.randomUUID(),
      }),
    );
    const next = await transaction((db) => claimSend(db));
    assert.ok(next);
    assert.equal(next.id, second.id);
    assert.deepEqual(await transaction((db) => beginSubmission(db, next)), {
      kind: "direct",
      account: { region: "eu", ...providerKey },
    });
    assert.equal(
      await transaction((db) =>
        finishSubmission(db, next, { state: "unknown" }),
      ),
      true,
    );
    assert.equal(
      (await transaction((db) => submissionStatus(db, token, second.id))).state,
      "unknown",
    );
    assert.equal(
      await transaction((db) =>
        finishSubmission(db, next, { state: "accepted", providerID: "123" }),
      ),
      true,
    );
    assert.equal(
      await transaction((db) =>
        finishSubmission(db, next, { state: "accepted", providerID: "123" }),
      ),
      true,
    );
    assert.equal(
      (
        await database.query(
          "SELECT accepted,reserved FROM webdock_mail.month_usage WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].accepted,
      "1",
    );
    const directory = await mkdtemp(join(tmpdir(), "webdock-send-e2e-"));
    const store = new PrivatePayloadStore(
      directory,
      process.env.MAIL_ENCRYPTION_KEY!,
    );
    const control = async (body: Record<string, unknown>): Promise<unknown> =>
      transaction(async (db) => {
        if (body.action === "send.authenticate")
          return authorizeMailKey(db, String(body.token));
        if (body.action === "send.commitPayload")
          return commitSubmissionPayload(
            db,
            body.lease as Parameters<typeof commitSubmissionPayload>[1],
          );
        if (body.action === "send.abortPayload")
          return abortSubmissionPayload(
            db,
            body.lease as Parameters<typeof abortSubmissionPayload>[1],
          );
        if (body.action === "send.preflight")
          return preflightSubmission(db, String(body.token), body.envelope);
        if (body.action === "send.admit")
          return stageSubmission(db, body.submission);
        if (body.action === "send.claim") return claimSubmission(db);
        if (body.action === "send.begin")
          return beginSubmission(
            db,
            body.lease as SendLease,
            body.digest as string | undefined,
          );
        if (body.action === "send.finish")
          return finishSubmission(
            db,
            body.lease as SendLease,
            body.result as {
              state: "accepted" | "rejected" | "unknown";
              providerID?: string;
            },
          );
        if (body.action === "send.status")
          return submissionStatus(
            db,
            String(body.token),
            String(body.id),
            body.cancel === true,
          );
        throw Error("Unexpected control command");
      });
    const api = createMailAPI(control, store);
    api.listen(0, "127.0.0.1");
    await once(api, "listening");
    const port = (api.address() as { port: number }).port,
      origin = `http://127.0.0.1:${port}`;
    try {
      const request = {
        from: "sender@native-key.example.test",
        to: ["recipient@example.test"],
        bcc: ["hidden@example.test"],
        subject: "Native API fixture",
        text: "Private body",
      };
      const options = {
        method: "POST",
        headers: {
          Authorization: "Bearer " + token,
          "Content-Type": "application/json",
          "Idempotency-Key": "api-e2e",
        },
        body: JSON.stringify(request),
      };
      const response = await fetch(origin + "/v1/emails", options);
      assert.equal(response.status, 202);
      const receipt = (await response.json()) as { id: string };
      const replay = await fetch(origin + "/v1/emails", options);
      assert.equal(((await replay.json()) as { id: string }).id, receipt.id);
      let sent = 0;
      assert.equal(
        await processQueuedSend(control, store, {
          relay: async (account, message) => {
            sent++;
            assert.deepEqual(account, { region: "eu", ...providerKey });
            assert.deepEqual(message.bcc, ["hidden@example.test"]);
            assert.doesNotMatch(message.raw.toString(), /Bcc:/);
            return submitRelayAPI(account, message, async (url, init) => {
              assert.equal(
                url,
                "https://api.eu.turbo-smtp.com/api/v2/mail/send",
              );
              const payload = JSON.parse(String(init?.body));
              assert.equal(payload.bcc, "hidden@example.test");
              assert.equal(payload.reference_id, receipt.id);
              return new Response('{"mid":999}');
            });
          },
        }),
        true,
      );
      assert.equal(sent, 1);
      assert.equal(await processQueuedSend(control, store), false);
      const state = await fetch(origin + "/v1/emails/" + receipt.id, {
        headers: { Authorization: "Bearer " + token },
      });
      assert.equal(
        ((await state.json()) as { state: string }).state,
        "accepted",
      );
    } finally {
      api.closeAllConnections();
      await new Promise<void>((resolve) => api.close(() => resolve()));
      await rm(directory, { recursive: true, force: true });
    }
    const staged = await transaction((db) =>
      stageSubmission(db, {
        ...command,
        idempotencyKey: "lost-upload",
        payloadRef: crypto.randomUUID(),
      }),
    );
    assert.equal(staged.state, "staging");
    assert.equal(await transaction((db) => claimSubmission(db)), null);
    const repeat = await transaction((db) =>
      stageSubmission(db, {
        ...command,
        idempotencyKey: "lost-upload",
        payloadRef: crypto.randomUUID(),
      }),
    );
    assert.equal(repeat.created, false);
    assert.equal("uploadToken" in repeat, false);
    await database.query(
      "UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",
      [staged.id],
    );
    assert.equal(await transaction((db) => claimSubmission(db)), null);
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.budget_reservation WHERE reference_id=$1",
          [staged.id],
        )
      ).rows[0].state,
      "released",
    );
    await database.query(
      "UPDATE webdock_mail.send_job SET finished_at=now()-interval '2 minutes' WHERE id=$1",
      [staged.id],
    );
    const garbage = await transaction((db) => submissionCleanup(db));
    const entry = garbage.find((item) => item.id === staged.id);
    assert.ok(entry);
    await transaction((db) => finishSubmissionCleanup(db, [entry]));
    assert.equal(
      (await transaction((db) => submissionCleanup(db))).some(
        (item) => item.id === staged.id,
      ),
      false,
    );
    const aborted = await transaction((db) =>
      stageSubmission(db, {
        ...command,
        idempotencyKey: "failed-upload",
        payloadRef: crypto.randomUUID(),
      }),
    );
    assert.ok(
      "uploadToken" in aborted &&
        aborted.uploadToken &&
        "customerID" in aborted,
    );
    const upload = {
      id: aborted.id,
      customerID: aborted.customerID,
      payloadRef: aborted.payloadRef,
      uploadToken: aborted.uploadToken,
    };
    assert.deepEqual(
      await transaction((db) =>
        abortSubmissionPayload(db, {
          ...upload,
          uploadToken: crypto.randomUUID(),
        }),
      ),
      { aborted: false },
    );
    assert.deepEqual(
      await transaction((db) => abortSubmissionPayload(db, upload)),
      { aborted: true },
    );
    await assert.rejects(
      transaction((db) => commitSubmissionPayload(db, upload)),
    );
    const unreadable = await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "unreadable-payload",
        payloadRef: crypto.randomUUID(),
      }),
    );
    const unreadableLease = await transaction((db) => claimSubmission(db));
    assert.ok(unreadableLease);
    assert.equal(unreadableLease.id, unreadable.id);
    assert.equal(
      await transaction((db) =>
        reviewSubmissionPreparation(db, unreadableLease),
      ),
      true,
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.send_job WHERE id=$1",
          [unreadable.id],
        )
      ).rows[0].state,
      "preparation_review",
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.budget_reservation WHERE reference_id=$1",
          [unreadable.id],
        )
      ).rows[0].state,
      "reserved",
    );
    assert.equal(
      (await transaction((db) => submissionCleanup(db))).some(
        (item) => item.id === unreadable.id,
      ),
      false,
    );
    assert.equal(
      (
        await transaction((db) =>
          submissionStatus(db, token, unreadable.id, true),
        )
      ).state,
      "cancelled",
    );
    const downgrade = await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "downgrade",
        payloadRef: crypto.randomUUID(),
      }),
    );
    await database.query(
      `UPDATE webdock_auth.tenant_subscription SET base='{"mailMessages":0}' WHERE customer_id=$1`,
      [customer],
    );
    const downgraded = await transaction((db) => claimSend(db));
    assert.ok(downgraded);
    assert.equal(downgraded.id, downgrade.id);
    assert.equal(
      await transaction((db) => beginSubmission(db, downgraded)),
      null,
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.budget_reservation WHERE reference_id=$1",
          [downgrade.id],
        )
      ).rows[0].state,
      "released",
    );
    await database.query(
      `UPDATE webdock_auth.tenant_subscription SET base='{"mailMessages":10}' WHERE customer_id=$1`,
      [customer],
    );
    const policy = {
      kubernetes: false,
      vercel: false,
      turbosmtp: true,
      maxClusters: 0,
      maxMailDomains: 10,
      manageExisting: false,
      namespaces: [],
    };
    await database.query(
      "INSERT INTO webdock_auth.hosting_byok_policy(customer_id,policy) VALUES($1,$2)",
      [customer, policy],
    );
    await database.query(
      "INSERT INTO webdock_auth.hosting_turbosmtp(customer_id,label,encrypted_secret,snapshot) VALUES($1,'Own fixture',$2,$3)",
      [
        customer,
        encryptMailSecret(
          { consumerKey: "own-key", consumerSecret: "own-secret" },
          `byok-mail:${customer}`,
        ),
        JSON.stringify([
          {
            domain: "native-key.example.test",
            spf_verified: true,
            dkim_verified: true,
            dmarc_verified: true,
          },
        ]),
      ],
    );
    await database.query(
      "UPDATE webdock_mail.send_transport SET mode='byok',byok_revision=1,revision=2,region='eu' WHERE customer_id=$1",
      [customer],
    );
    const own = await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "own",
        payloadRef: crypto.randomUUID(),
      }),
    );
    const ownLease = await transaction((db) => claimSend(db));
    assert.ok(ownLease);
    assert.equal(ownLease.id, own.id);
    assert.deepEqual(await transaction((db) => beginSubmission(db, ownLease)), {
      kind: "direct",
      account: {
        region: "eu",
        consumerKey: "own-key",
        consumerSecret: "own-secret",
      },
    });
    await transaction((db) =>
      finishSubmission(db, ownLease, { state: "rejected" }),
    );
    await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "own-disabled",
        payloadRef: crypto.randomUUID(),
      }),
    );
    await database.query(
      `UPDATE webdock_auth.hosting_byok_policy SET policy=jsonb_set(policy,'{turbosmtp}','false') WHERE customer_id=$1`,
      [customer],
    );
    const revokedOwn = await transaction((db) => claimSend(db));
    assert.ok(revokedOwn);
    assert.equal(
      await transaction((db) => beginSubmission(db, revokedOwn)),
      null,
    );
    await database.query(
      "UPDATE webdock_auth.hosting_byok_policy SET policy=$2 WHERE customer_id=$1",
      [customer, policy],
    );
    await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "own-rotated",
        payloadRef: crypto.randomUUID(),
      }),
    );
    await database.query(
      "UPDATE webdock_auth.hosting_turbosmtp SET revision=revision+1 WHERE customer_id=$1",
      [customer],
    );
    const rotated = await transaction((db) => claimSend(db));
    assert.ok(rotated);
    assert.equal(await transaction((db) => beginSubmission(db, rotated)), null);
    // Incoming jobs require verified instance credentials and a one-queue, content-bound DATA authorization.
    process.env.MAIL_MANAGED_CLUSTER_ID = "local-fixture";
    process.env.MAIL_HOSTNAME_SUFFIX = "example.test";
    const hookToken = `wd_hook_${customer}_${randomBytes(32).toString("base64url")}`;
    const smtpCredentials = {
      username: `wd-submit@mail-${customer}.example.test`,
      password: randomBytes(32).toString("base64url"),
      hookToken,
    };
    await database.query(
      "UPDATE webdock_mail.send_transport SET mode='managed',revision=3,encrypted_credentials=$2 WHERE customer_id=$1",
      [
        customer,
        sealSecret(
          providerKey,
          `send-transport:${customer}:3`,
          process.env.MAIL_ENCRYPTION_KEY!,
        ),
      ],
    );
    await database.query(
      "INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,desired_enabled,state,revision,applied_revision) VALUES($1,$2,'local-fixture',true,'ready',1,1)",
      [customer, "mail-" + customer],
    );
    await assert.rejects(
      transaction((db) =>
        admitSubmission(db, {
          ...command,
          idempotencyKey: "smtp-not-ready",
          payloadRef: crypto.randomUUID(),
        }),
      ),
    );
    await database.query(
      `INSERT INTO webdock_mail.instance(customer_id,encrypted_credentials,verified_at,smtp_certificate_sha256,smtp_certificate_revision,encrypted_submission_credentials,submission_hook_token_hash,submission_transport_revision,submission_ready_revision,submission_state)
   VALUES($1,'fixture-admin',now(),$2,1,$3,$4,3,1,'ready')`,
      [
        customer,
        "c".repeat(64),
        sealSecret(
          smtpCredentials,
          `submission:${customer}`,
          process.env.MAIL_ENCRYPTION_KEY!,
        ),
        createHash("sha256").update(hookToken).digest("hex"),
      ],
    );
    const native = await transaction((db) =>
      admitSubmission(db, {
        ...command,
        idempotencyKey: "smtp-ready",
        payloadRef: crypto.randomUUID(),
      }),
    );
    const nativeLease = await transaction((db) => claimSend(db));
    assert.ok(nativeLease);
    assert.equal(nativeLease.id, native.id);
    const nativeAuthorization = await transaction((db) =>
      beginSubmission(db, nativeLease, "d".repeat(64)),
    );
    assert.equal(nativeAuthorization?.kind, "stalwart");
    assert.ok(nativeAuthorization && nativeAuthorization.kind === "stalwart");
    assert.equal(nativeAuthorization.smtp.username, smtpCredentials.username);
    const policyCommand = {
      customerID: customer,
      token: hookToken,
      id: native.id,
      proof: nativeAuthorization.proof,
      digest: "d".repeat(64),
      username: smtpCredentials.username,
      queueID: "abc123",
      envelope: command.envelope,
    };
    assert.deepEqual(
      await transaction((db) =>
        authorizeSubmissionPolicy(db, {
          ...policyCommand,
          digest: "e".repeat(64),
        }),
      ),
      { accept: false },
    );
    assert.deepEqual(
      await transaction((db) =>
        authorizeSubmissionPolicy(db, {
          ...policyCommand,
          envelope: { ...command.envelope, to: ["changed@example.test"] },
        }),
      ),
      { accept: false },
    );
    assert.deepEqual(
      await transaction((db) => authorizeSubmissionPolicy(db, policyCommand)),
      { accept: true },
    );
    assert.deepEqual(
      await transaction((db) => authorizeSubmissionPolicy(db, policyCommand)),
      { accept: true },
    );
    assert.deepEqual(
      await transaction((db) =>
        authorizeSubmissionPolicy(db, { ...policyCommand, queueID: "abc124" }),
      ),
      { accept: false },
    );
    await transaction((db) =>
      finishSubmission(db, nativeLease, {
        state: "accepted",
        providerID: native.id,
      }),
    );
    assert.deepEqual(
      await transaction((db) => authorizeSubmissionPolicy(db, policyCommand)),
      { accept: false },
    );
    const hostname = `mail-${customer}.example.test`;
    assert.equal(
      await transaction((db) =>
        prepareSubmissionConfiguration(db, customer, hostname, "1"),
      ),
      null,
    );
    const preparation = await transaction((db) =>
      prepareSubmissionConfiguration(db, customer, hostname, "1", true),
    );
    assert.ok(preparation);
    assert.equal(preparation.profile.password, smtpCredentials.password);
    assert.equal(
      JSON.stringify(preparation.desired).includes(smtpCredentials.password),
      false,
    );
    await transaction((db) =>
      completeSubmissionConfiguration(db, customer, "1", preparation.desired, {
        ready: false,
      }),
    );
    assert.equal(
      (
        await database.query(
          "SELECT state FROM webdock_mail.service WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].state,
      "ready",
    );
    assert.equal(
      (
        await database.query(
          "SELECT submission_state FROM webdock_mail.instance WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].submission_state,
      "needs_review",
    );
    const retry = await transaction((db) =>
      prepareSubmissionConfiguration(db, customer, hostname, "1", true),
    );
    assert.ok(retry);
    assert.equal(retry.profile.hookToken, hookToken);
    await transaction((db) =>
      completeSubmissionConfiguration(db, customer, "1", retry.desired, {
        ready: true,
        resources: {
          accountID: "submit-account",
          hookID: "hook",
          routeID: "route",
          tlsID: "tls",
          certificateID: "certificate",
        },
        certificateSha256: "c".repeat(64),
      }),
    );
    await database.query(
      "UPDATE webdock_mail.service SET revision=2,applied_revision=2 WHERE customer_id=$1",
      [customer],
    );
    await transaction((db) =>
      reconcileSubmissionConfiguration(db, "unassigned-cluster"),
    );
    assert.equal(
      (
        await database.query(
          "SELECT id FROM webdock_mail.configuration_operation WHERE customer_id=$1",
          [customer],
        )
      ).rowCount,
      0,
    );
    await transaction((db) =>
      reconcileSubmissionConfiguration(db, "local-fixture"),
    );
    await transaction((db) =>
      reconcileSubmissionConfiguration(db, "local-fixture"),
    );
    const reproof = (
      await database.query(
        "SELECT reason,state FROM webdock_mail.configuration_operation WHERE customer_id=$1",
        [customer],
      )
    ).rows;
    assert.deepEqual(reproof, [
      { reason: "submission_reconcile", state: "pending" },
    ]);
    await database.query(
      "DELETE FROM webdock_auth.mail_sender_domain WHERE id=$1",
      [domain],
    );
    await assert.rejects(
      transaction((db) =>
        authorizeMailKey(db, token, "sender@other.example.test"),
      ),
    );
    await database.query(
      "UPDATE webdock_mail.api_key SET domain_restricted=false WHERE id=$1",
      [key],
    );
    await database.query(
      "UPDATE webdock_mail.api_key SET revoked_at=now() WHERE id=$1",
      [key],
    );
    await assert.rejects(
      transaction((db) =>
        authorizeMailKey(db, token, "sender@other.example.test"),
      ),
    );
  } finally {
    if (previous === undefined)
      delete process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED;
    else process.env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED = previous;
    for (const table of [
      "send_job",
      "budget_reservation",
      "month_usage",
      "send_transport",
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
      "hosting_byok_policy",
      "hosting_turbosmtp",
      "mail_tenant_account",
      "tenant_subscription",
    ])
      await database.query(
        `DELETE FROM webdock_auth.${table} WHERE customer_id=$1`,
        [customer],
      );
    await database.query(
      "DELETE FROM webdock_mail.api_key WHERE customer_id=$1",
      [customer],
    );
    await database.query(
      "DELETE FROM webdock_auth.mail_sender_domain WHERE customer_id=$1",
      [customer],
    );
    const mapping = (
      await database.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id",
        [customer],
      )
    ).rows[0];
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customer,
    ]);
    await database.query("DELETE FROM webdock_auth.organization WHERE id=$1", [
      mapping.organization_id,
    ]);
    if (before)
      await database.query(
        "UPDATE webdock_auth.platform_settings SET settings=$1 WHERE id=true",
        [before.settings],
      );
    else
      await database.query(
        "DELETE FROM webdock_auth.platform_settings WHERE id=true",
      );
  }
});
