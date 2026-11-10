import type { MailProductStore } from "./provider-ports";
import { randomUUID } from "node:crypto";
import type {
  ApiResult,
  EmailRequest,
  MailActor,
  BatchResult,
} from "@spitzli/mail-contracts";
import {
  parseEmail,
  parseID,
  MailValidationError,
} from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
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
  refreshBatchIdempotency: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["refreshBatchIdempotency"];
  consumeMailRate: ReturnType<
    typeof import("./product-rate-limit").createMailRateLimit
  >["consumeMailRate"];
  persistMailMessage: ReturnType<
    typeof import("./product-sending").createMailSending
  >["persistMailMessage"];
  dispatchMail: ReturnType<
    typeof import("./product-sending").createMailSending
  >["dispatchMail"];
  sendTurboMail: import("./provider-ports").MailSend;
  getSuppressedRecipients: ReturnType<
    typeof import("./product-suppressions").createMailSuppressions
  >["getSuppressedRecipients"];
  assertQuotaCapacity: ReturnType<
    typeof import("./product-quota").createMailQuota
  >["assertQuotaCapacity"];
};
type Options = {
  key?: string;
  inlineDeadline?: Date;
  send?: Dependencies["sendTurboMail"];
  directActor?: MailActor;
  maxItems?: number;
};
export function createMailBatches(dependencies: Dependencies) {
  const {
    database,
    withMailTransaction,
    assertMailAccess,
    assertVerifiedSender,
    runIdempotent,
    refreshBatchIdempotency,
    consumeMailRate,
    persistMailMessage,
    dispatchMail,
    sendTurboMail,
    getSuppressedRecipients,
    assertQuotaCapacity,
  } = dependencies;
  async function batchView(id: string): Promise<BatchResult> {
    const batch = (
      await database.query(
        "SELECT id,status,failed_index,failed_email_id FROM webdock_auth.mail_batch WHERE id=$1",
        [id],
      )
    ).rows[0];
    if (!batch) mailUnavailable();
    const items = (
      await database.query(
        "SELECT message_id FROM webdock_auth.mail_batch_item WHERE batch_id=$1 AND status='accepted' ORDER BY item_index",
        [id],
      )
    ).rows;
    return {
      id: batch.id,
      status: batch.status,
      data: items.map((r) => ({ id: r.message_id })),
      ...(batch.failed_index !== null
        ? {
            failed_index: batch.failed_index,
            failed_email_id: batch.failed_email_id,
          }
        : {}),
    };
  }
  async function getMailBatch(
    actor: MailActor,
    id: string,
  ): Promise<BatchResult> {
    parseID(id);
    await assertMailAccess(actor, "read");
    if (
      !(
        await database.query(
          "SELECT 1 FROM webdock_auth.mail_batch WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
          [id, actor.customerID, actor.connectionID],
        )
      ).rowCount
    )
      mailUnavailable();
    return batchView(id);
  }
  async function submitMailBatch(
    actor: MailActor,
    bodies: EmailRequest[],
    options: Options = {},
  ): Promise<ApiResult> {
    const requestID = randomUUID();
    try {
      await assertMailAccess(actor, "send", database, false);
      if (!Array.isArray(bodies) || !bodies.length || bodies.length > 100)
        throw new MailValidationError("Batch size must be 1–100");
      const parsed = bodies.map((body) =>
        parseEmail(body, new Date(), { batch: true }),
      );
      return await runIdempotent(
        actor,
        {
          key: options.key ?? randomUUID(),
          method: "POST",
          path: "/emails/batch",
          body: bodies,
        },
        async (token) => {
          const domains = await Promise.all(
            [...new Set(parsed.map((p) => p.fromEmail.split("@")[1]))].map(
              async (name) =>
                [
                  name,
                  (
                    await assertVerifiedSender(actor, name, {
                      simulation: parsed
                        .filter((p) => p.fromEmail.split("@")[1] === name)
                        .every((p) => p.simulated),
                    })
                  ).domainID,
                ] as const,
            ),
          );
          const domainMap = new Map(domains);
          const suppressed = await getSuppressedRecipients(actor.customerID, [
            ...new Set(parsed.flatMap((p) => p.recipients.map((r) => r.email))),
          ]);
          const count = parsed.reduce(
            (n, p) =>
              n +
              (p.simulated ||
              p.recipients
                .filter((r) => r.category === "to")
                .every((r) => suppressed.has(r.email))
                ? 0
                : p.recipients.filter((r) => !suppressed.has(r.email)).length),
            0,
          );
          await consumeMailRate(actor, {
            requests: 0,
            recipients: count,
            keys: 0,
          });
          const id = await withMailTransaction(async (db) => {
            await db.query(
              "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
              [actor.customerID],
            );
            await assertMailAccess(actor, "send", db);
            if (count)
              await assertQuotaCapacity(
                db,
                actor.customerID,
                count,
                new Date(),
              );
            const batch = (
              await db.query(
                "INSERT INTO webdock_auth.mail_batch(customer_id,connection_id) VALUES($1,$2) RETURNING id",
                [actor.customerID, actor.connectionID],
              )
            ).rows[0];
            for (const [index, p] of parsed.entries()) {
              const messageID = await persistMailMessage(
                db,
                actor,
                p,
                domainMap.get(p.fromEmail.split("@")[1])!,
              );
              await db.query(
                "INSERT INTO webdock_auth.mail_batch_item(customer_id,connection_id,batch_id,item_index,message_id,status) VALUES($1,$2,$3,$4,$5,'pending')",
                [
                  actor.customerID,
                  actor.connectionID,
                  batch.id,
                  index,
                  messageID,
                ],
              );
            }
            if (
              !(
                await db.query(
                  "UPDATE webdock_auth.mail_idempotency SET batch_id=$4 WHERE customer_id=$1 AND connection_id=$2 AND operation_token=$3 AND status='pending'",
                  [actor.customerID, actor.connectionID, token, batch.id],
                )
              ).rowCount
            )
              throw new MailError(
                409,
                "idempotency_conflict",
                "Reservation changed",
              );
            return batch.id as string;
          });
          await advanceMailBatch(
            id,
            options.inlineDeadline ?? new Date(Date.now() + 20000),
            { ...options, directActor: actor },
          );
          const result = await batchView(id);
          return {
            status:
              result.status === "processing"
                ? 202
                : result.status === "failed"
                  ? 502
                  : 200,
            body: {
              ...result,
              ...(result.status === "failed"
                ? {
                    name: "batch_send_failed",
                    statusCode: 502,
                    message: "Batch stopped at the first unsuccessful message",
                    request_id: requestID,
                  }
                : {}),
            },
          };
        },
      );
    } catch (error) {
      const known = error instanceof MailError;
      const status = known
        ? error.status
        : error instanceof MailValidationError
          ? 422
          : 503;
      return {
        status,
        body: {
          statusCode: status,
          name: known
            ? error.code
            : status === 422
              ? "validation_error"
              : "service_unavailable",
          message:
            known || error instanceof MailValidationError
              ? error.message
              : "Batch could not be completed",
          request_id: requestID,
        },
      };
    }
  }
  async function advanceMailBatch(
    id: string,
    deadline: Date,
    options: Options = {},
  ): Promise<void> {
    parseID(id);
    const token = randomUUID();
    const leased = (
      await database.query(
        `UPDATE webdock_auth.mail_batch b SET claim_token=$2,lease_until=now()+interval '2 minutes'
 WHERE b.id=$1 AND b.status='processing' AND (b.lease_until IS NULL OR b.lease_until<now())
 AND EXISTS(SELECT 1 FROM webdock_auth.mail_connection c WHERE c.id=b.connection_id AND c.state='ready') RETURNING b.id`,
        [id, token],
      )
    ).rowCount;
    if (!leased) return;
    try {
      let handled = 0;
      while (
        Date.now() + 15000 <= deadline.getTime() &&
        handled < (options.maxItems ?? 100)
      ) {
        const item = (
          await database.query(
            `SELECT i.item_index,i.message_id,m.dispatch_status FROM webdock_auth.mail_batch_item i
    JOIN webdock_auth.mail_message m ON m.id=i.message_id JOIN webdock_auth.mail_batch b ON b.id=i.batch_id JOIN webdock_auth.mail_connection c ON c.id=b.connection_id
    WHERE b.id=$1 AND b.claim_token=$2 AND b.lease_until>now() AND c.state='ready' AND i.status='pending' ORDER BY i.item_index LIMIT 1`,
            [id, token],
          )
        ).rows[0];
        if (!item) break;
        const outcome =
          item.dispatch_status === "scheduled"
            ? await dispatchMail(item.message_id, deadline, options)
            : null;
        const state = (
          await database.query(
            "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1",
            [item.message_id],
          )
        ).rows[0].dispatch_status;
        const accepted = ["accepted", "suppressed"].includes(state);
        if (
          outcome?.kind === "rejected" &&
          outcome.code === "not_dispatchable" &&
          state === "scheduled"
        )
          break;
        const continued = await withMailTransaction(async (db) => {
          if (
            !(
              await db.query(
                "SELECT id FROM webdock_auth.mail_batch WHERE id=$1 AND claim_token=$2 AND lease_until>now() FOR UPDATE",
                [id, token],
              )
            ).rowCount
          )
            return false;
          await db.query(
            "UPDATE webdock_auth.mail_batch_item SET status=$3 WHERE batch_id=$1 AND item_index=$2",
            [id, item.item_index, accepted ? "accepted" : "failed"],
          );
          if (!accepted) {
            await db.query(
              "UPDATE webdock_auth.mail_batch SET status='failed',failed_index=$2,failed_email_id=$3,finished_at=now() WHERE id=$1",
              [id, item.item_index, item.message_id],
            );
            await db.query(
              "UPDATE webdock_auth.mail_batch_item SET status='not_sent' WHERE batch_id=$1 AND status='pending'",
              [id],
            );
            await db.query(
              "UPDATE webdock_auth.mail_message m SET dispatch_status='failed',error_code='batch_not_sent',terminal_at=now() FROM webdock_auth.mail_batch_item i WHERE i.batch_id=$1 AND i.status='not_sent' AND m.id=i.message_id AND m.dispatch_status='scheduled'",
              [id],
            );
          }
          return accepted;
        });
        handled++;
        if (!continued) break;
      }
      await database.query(
        "UPDATE webdock_auth.mail_batch b SET status='succeeded',finished_at=now() WHERE id=$1 AND claim_token=$2 AND status='processing' AND NOT EXISTS(SELECT 1 FROM webdock_auth.mail_batch_item i WHERE i.batch_id=b.id AND i.status<>'accepted')",
        [id, token],
      );
      await refreshBatchIdempotency(database, id);
    } finally {
      await database.query(
        "UPDATE webdock_auth.mail_batch SET claim_token=NULL,lease_until=NULL WHERE id=$1 AND claim_token=$2",
        [id, token],
      );
    }
  }
  return { getMailBatch, submitMailBatch, advanceMailBatch };
}
