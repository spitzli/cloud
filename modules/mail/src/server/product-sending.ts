import type { MailProductStore } from "./provider-ports";
import { randomUUID } from "node:crypto";
import type { DatabaseReader } from "@spitzli/core/identity";
import type {
  ApiResult,
  DispatchOutcome,
  EmailRequest,
  MailActor,
  EventType,
} from "@spitzli/mail-contracts";
import {
  MailValidationError,
  parseEmail,
} from "@spitzli/mail-contracts/validation";
import { MailError } from "./product-access";
type Dependencies = {
  database: MailProductStore["database"];
  withMailTransaction: MailProductStore["withMailTransaction"];
  assertMailAccess: MailProductStore["assertMailAccess"];
  assertVerifiedSender: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["assertVerifiedSender"];
  runIdempotent: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["runIdempotent"];
  consumeMailRate: ReturnType<
    typeof import("./product-rate-limit").createMailRateLimit
  >["consumeMailRate"];
  getSuppressedRecipients: ReturnType<
    typeof import("./product-suppressions").createMailSuppressions
  >["getSuppressedRecipients"];
  reserveMailQuota: ReturnType<
    typeof import("./product-quota").createMailQuota
  >["reserveMailQuota"];
  settleMailQuota: ReturnType<
    typeof import("./product-quota").createMailQuota
  >["settleMailQuota"];
  recordMailEvent: ReturnType<
    typeof import("./product-events").createMailEvents
  >["recordMailEvent"];
  openMailAttachments: ReturnType<
    typeof import("./product-content").createMailContent
  >["openMailAttachments"];
  sealMailAttachments: ReturnType<
    typeof import("./product-content").createMailContent
  >["sealMailAttachments"];
  resolveSendingCredential: ReturnType<
    typeof import("./provider-credentials").createMailCredentials
  >["resolveSendingCredential"];
  sendTurboMail: import("./provider-ports").MailSend;
};
type Options = {
  key?: string;
  now?: () => Date;
  send?: Dependencies["sendTurboMail"];
};
export function createMailSending(dependencies: Dependencies) {
  const {
    database,
    withMailTransaction,
    assertMailAccess,
    assertVerifiedSender,
    runIdempotent,
    consumeMailRate,
    getSuppressedRecipients,
    reserveMailQuota,
    settleMailQuota,
    recordMailEvent,
    openMailAttachments,
    sealMailAttachments,
    resolveSendingCredential,
    sendTurboMail,
  } = dependencies;
  function failure(
    error: unknown,
    requestID: string,
    emailID?: string,
  ): ApiResult {
    const known = error instanceof MailError,
      validation = error instanceof MailValidationError;
    const status = known ? error.status : validation ? 422 : 503;
    return {
      status,
      body: {
        statusCode: status,
        name: known
          ? error.code
          : validation
            ? "validation_error"
            : "mail_unavailable",
        message:
          known || validation
            ? error.message
            : "Mail request could not be completed",
        request_id: requestID,
        ...(emailID ? { email_id: emailID } : {}),
      },
    };
  }
  async function submitMail(
    actor: MailActor,
    body: EmailRequest,
    options: Options = {},
  ): Promise<ApiResult> {
    const now = options.now ?? (() => new Date()),
      key = options.key ?? randomUUID(),
      requestID = randomUUID();
    try {
      await assertMailAccess(actor, "send", database, false);
      parseEmail(body, now(), { ignoreScheduleTime: true });
      return await runIdempotent(
        actor,
        { key, method: "POST", path: "/emails", body },
        async (token) => {
          let id: string | undefined;
          try {
            const parsed = parseEmail(body, now());
            const domain = await assertVerifiedSender(
              actor,
              parsed.fromEmail.split("@")[1],
              { simulation: parsed.simulated },
            );
            await consumeMailRate(actor, {
              requests: 0,
              recipients: parsed.simulated ? 0 : parsed.recipients.length,
              keys: 0,
            });
            id = await withMailTransaction(async (db) => {
              await assertMailAccess(actor, "send", db);
              if (parsed.request.scheduled_at) {
                await db.query(
                  "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR UPDATE",
                  [actor.customerID],
                );
                if (
                  Number(
                    (
                      await db.query(
                        process.env.DATABASE_RLS_SECRET
                          ? "SELECT webdock_rls.mail_admission_count($1,'scheduled') AS count"
                          : "SELECT count(*) FROM webdock_auth.mail_message WHERE customer_id=$1 AND dispatch_status='scheduled'",
                        [actor.customerID],
                      )
                    ).rows[0].count,
                  ) >= 1000
                )
                  throw new MailError(
                    429,
                    "schedule_limit_exceeded",
                    "Pending schedule limit reached",
                  );
              }
              const row = {
                id: await persistMailMessage(
                  db,
                  actor,
                  parsed,
                  domain.domainID,
                ),
              };
              if (
                !(
                  await db.query(
                    "UPDATE webdock_auth.mail_idempotency SET message_id=$4 WHERE customer_id=$1 AND connection_id=$2 AND key=$3 AND operation_token=$5 AND status='pending'",
                    [actor.customerID, actor.connectionID, key, row.id, token],
                  )
                ).rowCount
              )
                throw new MailError(
                  409,
                  "idempotency_conflict",
                  "Request reservation changed",
                );
              if (parsed.request.scheduled_at)
                await recordMailEvent(
                  {
                    ...actor,
                    resourceID: row.id,
                    type: "email.scheduled",
                    occurredAt: now().toISOString(),
                    source: "application",
                    dedupeKey: "scheduled:" + row.id,
                  },
                  db,
                );
              return row.id as string;
            });
            if (parsed.request.scheduled_at)
              return { status: 200, body: { id } };
            const outcome = await dispatchMail(
              id,
              new Date(Date.now() + 20_000),
              { ...options, directActor: actor },
            );
            if (outcome.kind === "accepted")
              return { status: 200, body: { id } };
            if (outcome.kind === "unknown")
              return {
                status: 502,
                body: {
                  statusCode: 502,
                  name: "send_outcome_unknown",
                  message: "Provider acceptance is uncertain; do not resend",
                  request_id: requestID,
                  email_id: id,
                },
              };
            if (outcome.code === "suppressed")
              return { status: 200, body: { id } };
            return failure(
              outcome.code === "dispatch_disabled"
                ? new MailError(
                    503,
                    "dispatch_disabled",
                    "Mail dispatch is disabled",
                  )
                : new MailError(502, "send_failed", "Message was not accepted"),
              requestID,
              id,
            );
          } catch (error) {
            return failure(error, requestID, id);
          }
        },
        now(),
      );
    } catch (error) {
      return failure(error, requestID);
    }
  }
  async function dispatchMail(
    messageID: string,
    deadline: Date,
    options: Options & { directActor?: MailActor } = {},
  ): Promise<DispatchOutcome> {
    const now = options.now ?? (() => new Date()),
      claim = randomUUID();
    const message = (
      await database.query(
        `UPDATE webdock_auth.mail_message SET dispatch_status='dispatching',claim_token=$2,lease_until=now()+interval '2 minutes',updated_at=now()
  WHERE id=$1 AND dispatch_status='scheduled' AND (scheduled_at IS NULL OR scheduled_at<=$3) RETURNING *`,
        [messageID, claim, now()],
      )
    ).rows[0];
    if (!message) return { kind: "rejected", code: "not_dispatchable" };
    const job: MailActor = {
      customerID: message.customer_id,
      connectionID: message.connection_id,
      principal: { kind: "job", jobID: messageID },
    };
    const actor = options.directActor ?? job;
    let handedOff = false;
    try {
      await assertMailAccess(actor, "send");
      if (
        (
          await database.query(
            "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
          )
        ).rows[0]?.settings?.mailDispatchEnabled !== true
      )
        throw new MailError(
          503,
          "dispatch_disabled",
          "Mail dispatch is disabled",
        );
      if (
        !message.simulated &&
        (
          await database.query(
            "SELECT to_regclass('webdock_mail.service') AS present",
          )
        ).rows[0].present &&
        (
          await database.query(
            "SELECT 1 FROM webdock_mail.service WHERE customer_id=$1",
            [message.customer_id],
          )
        ).rowCount
      ) {
        throw new MailError(
          409,
          "native_submission_required",
          "Incoming Mail requires the native Stalwart submission route",
        );
      }
      const content = (
        await database.query(
          "SELECT * FROM webdock_auth.mail_message_content WHERE message_id=$1",
          [messageID],
        )
      ).rows[0];
      if (!content)
        throw new MailError(
          409,
          "content_unavailable",
          "Message content expired",
        );
      const recipients = (
        await database.query(
          "SELECT email,category FROM webdock_auth.mail_recipient WHERE message_id=$1",
          [messageID],
        )
      ).rows as { email: string; category: "to" | "cc" | "bcc" }[];
      const sender = parseEmail(
        {
          from: message.from_address,
          to: recipients.filter((r) => r.category === "to").map((r) => r.email),
          subject: message.subject,
          text: content.text_body ?? undefined,
          html: content.html_body ?? undefined,
        },
        now(),
      ).fromEmail;
      const domain = await assertVerifiedSender(actor, sender.split("@")[1], {
        simulation: message.simulated,
      });
      if (domain.domainID !== message.domain_id)
        throw new MailError(
          409,
          "sender_changed",
          "Original sender domain is no longer available",
        );
      const suppressed = message.simulated
        ? new Set<string>()
        : await getSuppressedRecipients(
            message.customer_id,
            recipients.map((r) => r.email),
          );
      const noPrimary = recipients
        .filter((r) => r.category === "to")
        .every((r) => suppressed.has(r.email));
      const envelope = recipients.filter(
        (r) => !noPrimary && !suppressed.has(r.email),
      );
      await withMailTransaction(async (db) => {
        for (const recipient of recipients.filter(
          (r) => noPrimary || suppressed.has(r.email),
        )) {
          await db.query(
            "UPDATE webdock_auth.mail_recipient SET suppression_reason=$3,delivery_status='suppressed' WHERE message_id=$1 AND email=$2",
            [
              messageID,
              recipient.email,
              suppressed.has(recipient.email)
                ? "suppressed"
                : "no_primary_recipient",
            ],
          );
          await recordMailEvent(
            {
              ...job,
              resourceID: messageID,
              recipient: recipient.email,
              type: "email.suppressed",
              source: "application",
              occurredAt: now().toISOString(),
              dedupeKey: "suppressed:" + messageID + ":" + recipient.email,
            },
            db,
          );
        }
        if (noPrimary) {
          await db.query(
            "UPDATE webdock_auth.mail_message SET dispatch_status='suppressed',terminal_at=now(),claim_token=NULL,lease_until=NULL WHERE id=$1 AND claim_token=$2",
            [messageID, claim],
          );
          await db.query(
            "UPDATE webdock_auth.mail_message_content SET encrypted_attachments=NULL,expires_at=now()+interval '30 days' WHERE message_id=$1",
            [messageID],
          );
        }
      });
      if (noPrimary) return { kind: "rejected", code: "suppressed" };
      await reserveMailQuota(actor, messageID, envelope.length, now());
      const attachments = content.encrypted_attachments
        ? openMailAttachments(
            { customerID: message.customer_id, messageID },
            content.encrypted_attachments,
          ).map((value, index) => ({
            filename: content.attachment_metadata[index].filename,
            content_type: content.attachment_metadata[index].content_type,
            content_id: content.attachment_metadata[index].content_id,
            content: value,
          }))
        : undefined;
      const request: EmailRequest = {
        from: message.from_address,
        to: envelope.filter((r) => r.category === "to").map((r) => r.email),
        cc: envelope.filter((r) => r.category === "cc").map((r) => r.email),
        bcc: envelope.filter((r) => r.category === "bcc").map((r) => r.email),
        subject: message.subject,
        text: content.text_body ?? undefined,
        html: content.html_body ?? undefined,
        headers: content.headers,
        tags: content.tags,
        reply_to: content.reply_to,
        attachments,
      };
      let outcome: DispatchOutcome;
      if (message.simulated)
        outcome = { kind: "accepted", mid: "simulator:" + messageID };
      else {
        const credential = await resolveSendingCredential(actor);
        await assertMailAccess(actor, "send");
        if (Date.now() >= deadline.getTime())
          throw new MailError(
            503,
            "dispatch_deadline",
            "Dispatch deadline exceeded",
          );
        await withMailTransaction(async (db) => {
          await db.query(
            "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
            [actor.customerID],
          );
          await assertMailAccess(actor, "send", db);
          if (
            (
              await db.query(
                "SELECT to_regclass('webdock_mail.service') AS present",
              )
            ).rows[0].present &&
            (
              await db.query(
                "SELECT 1 FROM webdock_mail.service WHERE customer_id=$1 FOR SHARE",
                [actor.customerID],
              )
            ).rowCount
          )
            throw new MailError(
              409,
              "native_submission_required",
              "Incoming Mail requires the native Stalwart submission route",
            );
          if (
            (
              await db.query(
                "SELECT settings FROM webdock_auth.platform_settings WHERE id=true FOR SHARE",
              )
            ).rows[0]?.settings?.mailDispatchEnabled !== true
          )
            throw new MailError(
              503,
              "dispatch_disabled",
              "Mail dispatch is disabled",
            );
          if (
            !(
              await db.query(
                "UPDATE webdock_auth.mail_message SET handoff_started_at=now() WHERE id=$1 AND claim_token=$2 AND dispatch_status='dispatching' RETURNING id",
                [messageID, claim],
              )
            ).rowCount
          )
            throw new MailError(
              409,
              "dispatch_changed",
              "Dispatch claim changed",
            );
          await db.query(
            "UPDATE webdock_auth.mail_recipient SET included_in_handoff=true WHERE message_id=$1 AND suppression_reason IS NULL",
            [messageID],
          );
        });
        handedOff = true;
        outcome = await (options.send ?? sendTurboMail)(
          credential,
          request,
          message.provider_reference,
          AbortSignal.timeout(Math.max(1, deadline.getTime() - Date.now())),
        );
      }
      const stored = await withMailTransaction(async (db) => {
        await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          "mail-budget:" + message.customer_id,
        ]);
        const final = (
          await db.query(
            `UPDATE webdock_auth.mail_message SET dispatch_status=$3,provider_mid=$4,error_code=$5,accepted_at=CASE WHEN $3='accepted' THEN now() ELSE accepted_at END,
    terminal_at=CASE WHEN $3 IN ('accepted','failed') THEN now() ELSE NULL END,lease_until=NULL,updated_at=now()
    WHERE id=$1 AND claim_token=$2 AND dispatch_status='dispatching' RETURNING id`,
            [
              messageID,
              claim,
              outcome.kind === "accepted"
                ? "accepted"
                : outcome.kind === "rejected"
                  ? "failed"
                  : "outcome_unknown",
              outcome.kind === "accepted" ? outcome.mid : null,
              outcome.kind === "accepted" ? null : outcome.code,
            ],
          )
        ).rows[0];
        if (!final) return false;
        if (outcome.kind !== "unknown")
          await db.query(
            "UPDATE webdock_auth.mail_message_content SET encrypted_attachments=NULL,expires_at=now()+interval '30 days' WHERE message_id=$1",
            [messageID],
          );
        for (const recipient of envelope) {
          if (outcome.kind !== "unknown")
            await recordMailEvent(
              {
                ...job,
                resourceID: messageID,
                recipient: recipient.email,
                type:
                  outcome.kind === "accepted" ? "email.sent" : "email.failed",
                source: message.simulated ? "simulator" : "application",
                occurredAt: now().toISOString(),
                dedupeKey:
                  outcome.kind + ":" + messageID + ":" + recipient.email,
              },
              db,
            );
          if (message.simulated)
            await recordMailEvent(
              {
                ...job,
                resourceID: messageID,
                recipient: recipient.email,
                type: ("email." + recipient.email.split("@")[0]) as EventType,
                source: "simulator",
                occurredAt: now().toISOString(),
                dedupeKey: "simulator:" + messageID + ":" + recipient.email,
              },
              db,
            );
        }
        await settleMailQuota(
          messageID,
          outcome.kind === "accepted"
            ? "accepted"
            : outcome.kind === "rejected"
              ? "rejected"
              : "unknown",
          db,
        );
        return true;
      });
      if (!stored) {
        const current = (
          await database.query(
            "SELECT dispatch_status,provider_mid FROM webdock_auth.mail_message WHERE id=$1",
            [messageID],
          )
        ).rows[0];
        return current?.dispatch_status === "accepted"
          ? { kind: "accepted", mid: current.provider_mid }
          : { kind: "unknown", code: "concurrent_dispatch_update" };
      }
      return outcome;
    } catch (error) {
      if (handedOff) {
        await database
          .query(
            "UPDATE webdock_auth.mail_message SET dispatch_status='outcome_unknown',error_code='handoff_persistence_failure',updated_at=now() WHERE id=$1 AND claim_token=$2 AND dispatch_status='dispatching'",
            [messageID, claim],
          )
          .catch(() => {});
        return { kind: "unknown", code: "handoff_persistence_failure" };
      }
      await withMailTransaction(async (db) => {
        await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          "mail-budget:" + message.customer_id,
        ]);
        const updated = await db.query(
          "UPDATE webdock_auth.mail_message SET dispatch_status='failed',error_code=$3,terminal_at=now(),lease_until=NULL WHERE id=$1 AND claim_token=$2 AND dispatch_status='dispatching'",
          [
            messageID,
            claim,
            error instanceof MailError ? error.code : "preflight_failed",
          ],
        );
        if (updated.rowCount) {
          await db.query(
            "UPDATE webdock_auth.mail_message_content SET encrypted_attachments=NULL,expires_at=now()+interval '30 days' WHERE message_id=$1",
            [messageID],
          );
          await settleMailQuota(messageID, "rejected", db);
        }
      });
      return {
        kind: "rejected",
        code: error instanceof MailError ? error.code : "preflight_failed",
      };
    }
  }
  /** Internal admission primitive; caller authorizes and owns the transaction. */
  async function persistMailMessage(
    db: DatabaseReader,
    actor: MailActor,
    parsed: ReturnType<typeof parseEmail>,
    domainID: string,
  ): Promise<string> {
    await db.query(
      "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
      [actor.customerID],
    );
    const row = (
      await db.query(
        `INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,principal,from_address,subject,dispatch_status,scheduled_at,provider_reference,simulated)
      VALUES($1,$2,$3,$4,$5,$6,'scheduled',$7,$8,$9) RETURNING id`,
        [
          actor.customerID,
          actor.connectionID,
          domainID,
          JSON.stringify(actor.principal),
          parsed.request.from,
          parsed.request.subject,
          parsed.request.scheduled_at ?? null,
          randomUUID(),
          parsed.simulated,
        ],
      )
    ).rows[0];
    const attachments = parsed.request.attachments ?? [];
    const metadata = attachments.map((a) => ({
      filename: a.filename,
      content_type: a.content_type,
      content_id: a.content_id,
      size: Buffer.from(a.content, "base64").length,
    }));
    await db.query(
      `INSERT INTO webdock_auth.mail_message_content(customer_id,connection_id,message_id,text_body,html_body,headers,tags,reply_to,attachment_metadata,encrypted_attachments)
      VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)`,
      [
        actor.customerID,
        actor.connectionID,
        row.id,
        parsed.request.text ?? null,
        parsed.request.html ?? null,
        JSON.stringify(parsed.request.headers),
        JSON.stringify(parsed.request.tags),
        JSON.stringify(parsed.request.reply_to),
        JSON.stringify(metadata),
        attachments.length
          ? sealMailAttachments(
              { customerID: actor.customerID, messageID: row.id },
              attachments,
            )
          : null,
      ],
    );
    for (const recipient of parsed.recipients)
      await db.query(
        "INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category) VALUES($1,$2,$3,$4,$5)",
        [
          actor.customerID,
          actor.connectionID,
          row.id,
          recipient.email,
          recipient.category,
        ],
      );
    return row.id;
  }
  return { submitMail, dispatchMail, persistMailMessage };
}
