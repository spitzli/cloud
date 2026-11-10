import type { MailProductStore } from "./provider-ports";
import type { DatabaseReader } from "@spitzli/core/identity";
import { isMailEvent, type MailEventInput } from "@spitzli/mail-contracts";
import { parseAddress, parseID } from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
import { emptyRecipientState, projectRecipient } from "./product-projections";
/** Internal event ingestion; callback sources are authenticated before this boundary. */
export function createMailEvents(
  dependencies: Pick<MailProductStore, "withMailTransaction">,
) {
  const { withMailTransaction } = dependencies;
  async function recordMailEvent(
    event: MailEventInput,
    transaction?: DatabaseReader,
    options: { outbox?: boolean } = {},
  ): Promise<{ inserted: boolean }> {
    parseID(event.customerID);
    parseID(event.connectionID);
    parseID(event.resourceID);
    const at = new Date(event.occurredAt);
    if (
      !isMailEvent(event.type) ||
      !["application", "provider", "simulator", "reconciliation"].includes(
        event.source,
      ) ||
      !Number.isFinite(at.getTime()) ||
      at.getTime() > Date.now() + 300_000 ||
      !event.dedupeKey ||
      event.dedupeKey.length > 512
    )
      throw new MailError(422, "invalid_event", "Invalid canonical event");
    const recipient =
      event.recipient === undefined ? null : parseAddress(event.recipient);
    const run = async (db: DatabaseReader) => {
      const message = event.type.startsWith("email.")
        ? (
            await db.query(
              "SELECT id,simulated FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3 FOR UPDATE",
              [event.resourceID, event.customerID, event.connectionID],
            )
          ).rows[0]
        : null;
      if (event.type.startsWith("email.") && !message) mailUnavailable();
      if (
        !message &&
        !(
          await db.query(
            "SELECT 1 FROM webdock_auth.mail_connection_domain WHERE domain_id=$1 AND customer_id=$2 AND connection_id=$3",
            [event.resourceID, event.customerID, event.connectionID],
          )
        ).rowCount
      )
        mailUnavailable();
      const target = recipient
        ? (
            await db.query(
              "SELECT * FROM webdock_auth.mail_recipient WHERE message_id=$1 AND email=$2 AND customer_id=$3 AND connection_id=$4 FOR UPDATE",
              [
                event.resourceID,
                recipient,
                event.customerID,
                event.connectionID,
              ],
            )
          ).rows[0]
        : null;
      if (recipient && !target)
        throw new MailError(
          422,
          "invalid_recipient",
          "Event recipient is not in the message envelope",
        );
      const details =
        typeof event.details?.code === "string" &&
        /^[a-zA-Z0-9_.-]{1,64}$/.test(event.details.code)
          ? { code: event.details.code }
          : {};
      const saved = (
        await db.query(
          `INSERT INTO webdock_auth.mail_event(customer_id,connection_id,resource_id,message_id,recipient,type,source,dedupe_key,details,occurred_at)
   VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(connection_id,dedupe_key) DO NOTHING RETURNING id`,
          [
            event.customerID,
            event.connectionID,
            event.resourceID,
            message?.id ?? null,
            recipient,
            event.type,
            event.source,
            event.dedupeKey,
            JSON.stringify(details),
            at,
          ],
        )
      ).rows[0];
      if (!saved) return { inserted: false };
      if (target) {
        const iso = (value: Date | string | null) =>
          value ? new Date(value).toISOString() : null;
        const previous = {
          ...emptyRecipientState(),
          delivery: target.delivery_status,
          firstDeliveredAt: iso(target.first_delivered_at),
          firstOpenedAt: iso(target.first_open_at),
          lastOpenedAt: iso(target.last_open_at),
          firstClickedAt: iso(target.first_click_at),
          lastClickedAt: iso(target.last_click_at),
          complainedAt: iso(target.complained_at),
          unsubscribedAt: iso(target.unsubscribed_at),
        };
        const projected = projectRecipient(previous, event);
        await db.query(
          `UPDATE webdock_auth.mail_recipient SET delivery_status=$3,first_delivered_at=$4,first_open_at=$5,last_open_at=$6,first_click_at=$7,last_click_at=$8,complained_at=$9,unsubscribed_at=$10 WHERE message_id=$1 AND email=$2`,
          [
            event.resourceID,
            recipient,
            projected.delivery,
            projected.firstDeliveredAt,
            projected.firstOpenedAt,
            projected.lastOpenedAt,
            projected.firstClickedAt,
            projected.lastClickedAt,
            projected.complainedAt,
            projected.unsubscribedAt,
          ],
        );
        if (
          !message.simulated &&
          event.source !== "simulator" &&
          ["email.bounced", "email.complained", "email.unsubscribed"].includes(
            event.type,
          )
        ) {
          await db.query(
            `INSERT INTO webdock_auth.mail_suppression(customer_id,email,reason,source) VALUES($1,$2,$3,$4)
     ON CONFLICT(customer_id,email) DO UPDATE SET reason=excluded.reason,source=excluded.source
     WHERE mail_suppression.reason NOT IN ('complained','unsubscribed') OR excluded.reason IN ('complained','unsubscribed')`,
            [event.customerID, recipient, event.type.slice(6), event.source],
          );
        }
      }
      const body = JSON.stringify({
        version: 1,
        id: saved.id,
        type: event.type,
        created_at: at.toISOString(),
        data: {
          id: event.resourceID,
          simulated: !!message?.simulated,
          ...(recipient && target?.category !== "bcc" ? { recipient } : {}),
          ...details,
        },
      });
      if (options.outbox !== false)
        await db.query(
          `INSERT INTO webdock_auth.mail_webhook_delivery(customer_id,connection_id,webhook_id,event_id,body)
   SELECT customer_id,connection_id,id,$3,$4 FROM webdock_auth.mail_webhook WHERE customer_id=$1 AND connection_id=$2 AND deleted_at IS NULL AND events ? $5
   ON CONFLICT(webhook_id,event_id) DO NOTHING`,
          [event.customerID, event.connectionID, saved.id, body, event.type],
        );
      return { inserted: true };
    };
    return transaction ? run(transaction) : withMailTransaction(run);
  }

  return { recordMailEvent };
}
