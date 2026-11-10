import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
import { parseID, parseScheduleTime } from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
import type { createMailEvents } from "./product-events";
export function createMailSchedules(
  dependencies: Pick<
    MailProductStore,
    "withMailTransaction" | "assertMailAccess"
  > & {
    recordMailEvent: ReturnType<typeof createMailEvents>["recordMailEvent"];
  },
) {
  const { withMailTransaction, assertMailAccess, recordMailEvent } =
    dependencies;
  async function rescheduleMail(
    actor: MailActor,
    id: string,
    scheduledAt: string,
    revision: number,
  ): Promise<void> {
    parseID(id);
    const at = parseScheduleTime(scheduledAt);
    if (!Number.isSafeInteger(revision) || revision < 1)
      throw new MailError(422, "invalid_revision", "Invalid revision");
    await withMailTransaction(async (db) => {
      await assertMailAccess(
        actor,
        actor.principal.kind === "api_key" ? "read" : "send",
        db,
        false,
      );
      if (
        !(
          await db.query(
            "SELECT id FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
            [id, actor.customerID, actor.connectionID],
          )
        ).rowCount
      )
        mailUnavailable();
      if (
        !(
          await db.query(
            "UPDATE webdock_auth.mail_message SET scheduled_at=$4,revision=revision+1,updated_at=now() WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND dispatch_status='scheduled' AND revision=$5 AND scheduled_at IS NOT NULL RETURNING id",
            [id, actor.customerID, actor.connectionID, at, revision],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "schedule_conflict",
          "Schedule was changed or claimed",
        );
    });
  }
  async function cancelScheduledMail(
    actor: MailActor,
    id: string,
    revision: number,
  ): Promise<void> {
    parseID(id);
    if (!Number.isSafeInteger(revision) || revision < 1)
      throw new MailError(422, "invalid_revision", "Invalid revision");
    await withMailTransaction(async (db) => {
      await assertMailAccess(
        actor,
        actor.principal.kind === "api_key" ? "read" : "send",
        db,
        false,
      );
      if (
        !(
          await db.query(
            "SELECT id FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
            [id, actor.customerID, actor.connectionID],
          )
        ).rowCount
      )
        mailUnavailable();
      if (
        !(
          await db.query(
            "UPDATE webdock_auth.mail_message SET dispatch_status='canceled',revision=revision+1,terminal_at=now(),updated_at=now() WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND dispatch_status='scheduled' AND revision=$4 AND scheduled_at IS NOT NULL RETURNING id",
            [id, actor.customerID, actor.connectionID, revision],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "schedule_conflict",
          "Schedule was changed or claimed",
        );
      await db.query(
        "UPDATE webdock_auth.mail_message_content SET encrypted_attachments=NULL,expires_at=now()+interval '30 days' WHERE message_id=$1",
        [id],
      );
      await recordMailEvent(
        {
          ...actor,
          resourceID: id,
          type: "email.canceled",
          occurredAt: new Date().toISOString(),
          source: "application",
          dedupeKey: "canceled:" + id,
        },
        db,
      );
    });
  }

  return { rescheduleMail, cancelScheduledMail };
}
