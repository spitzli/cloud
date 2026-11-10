import type { MailProductStore } from "./provider-ports";
import type { MailFetch as Fetch } from "./provider-ports";
import type { MailActor, EventType } from "@spitzli/mail-contracts";
import { parseAddress, parseID } from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
type Dependencies = {
  discardExpiredUncertainContent: ReturnType<
    typeof import("./product-content").createMailContent
  >["discardExpiredUncertainContent"];
  database: MailProductStore["database"];
  withMailTransaction: MailProductStore["withMailTransaction"];
  assertMailAccess: MailProductStore["assertMailAccess"];
  mailManagementHeaders: ReturnType<
    typeof import("./provider-credentials").createMailCredentials
  >["mailManagementHeaders"];
  recordMailEvent: ReturnType<
    typeof import("./product-events").createMailEvents
  >["recordMailEvent"];
  settleMailQuota: ReturnType<
    typeof import("./product-quota").createMailQuota
  >["settleMailQuota"];
  canonicalHash: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["canonicalHash"];
  refreshBatchIdempotency: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["refreshBatchIdempotency"];
};

export function createMailReconcile(dependencies: Dependencies) {
  const {
    discardExpiredUncertainContent,
    database,
    withMailTransaction,
    assertMailAccess,
    mailManagementHeaders,
    recordMailEvent,
    settleMailQuota,
    canonicalHash,
    refreshBatchIdempotency,
  } = dependencies;
  async function reconcileMail(
    actor: MailActor,
    id: string,
    deps: { headers?: typeof mailManagementHeaders; fetcher?: Fetch } = {},
  ): Promise<void> {
    parseID(id);
    await assertMailAccess(actor, "operate", database, false);
    const message = (
      await database.query(
        "SELECT provider_mid FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
        [id, actor.customerID, actor.connectionID],
      )
    ).rows[0];
    if (!message) mailUnavailable();
    if (!message.provider_mid || !/^\d{1,64}$/.test(message.provider_mid))
      throw new MailError(
        409,
        "await_callback",
        "No proven provider MID is available; await a scoped callback",
      );
    const response = await (deps.fetcher ?? fetch)(
      "https://pro.api.serversmtp.com/api/v2/analytics/" +
        encodeURIComponent(message.provider_mid),
      {
        method: "GET",
        headers: await (deps.headers ?? mailManagementHeaders)(actor),
        redirect: "error",
        cache: "no-store",
        signal: AbortSignal.timeout(10000),
      },
    );
    if (!response.ok)
      throw new MailError(
        503,
        "provider_unavailable",
        "Provider observation is unavailable",
      );
    const reader = response.body?.getReader();
    if (!reader)
      throw new MailError(
        503,
        "provider_unavailable",
        "Provider observation is unavailable",
      );
    const chunks: Uint8Array[] = [];
    let size = 0;
    try {
      for (;;) {
        const part = await reader.read();
        if (part.done) break;
        size += part.value.byteLength;
        if (size > 65536)
          throw new MailError(
            503,
            "provider_unavailable",
            "Provider response is too large",
          );
        chunks.push(part.value);
      }
    } finally {
      await reader.cancel().catch(() => {});
    }
    const data = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks)),
      (key: string, value: unknown, context?: { source?: string }) =>
        key === "id" && typeof value === "number" ? context?.source : value,
    ) as { id?: unknown; recipient?: unknown; status?: unknown };
    if (data.id !== message.provider_mid || typeof data.status !== "string")
      throw new MailError(
        409,
        "provider_identity_mismatch",
        "Provider observation does not match the message",
      );
    const recipient = parseAddress(data.recipient);
    const mapping: Record<string, EventType[]> = {
      NEW: ["email.sent"],
      DEFER: ["email.delivery_delayed"],
      SUCCESS: ["email.delivered"],
      FAIL: ["email.bounced"],
      SYSFAIL: ["email.failed"],
      OPEN: ["email.delivered", "email.opened"],
      CLICK: ["email.delivered", "email.clicked"],
      UNSUB: ["email.delivered", "email.unsubscribed"],
      REPORT: ["email.delivered", "email.complained"],
    };
    if (!Object.hasOwn(mapping, data.status))
      throw new MailError(
        409,
        "provider_status_unknown",
        "Provider status has no verified mapping",
      );
    const types = mapping[data.status],
      observation = canonicalHash([
        message.provider_mid,
        recipient,
        data.status,
      ]);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "operate", db, false);
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        "mail-budget:" + actor.customerID,
      ]);
      const current = (
        await db.query(
          "SELECT id,handoff_started_at FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND provider_mid=$4 AND dispatch_status IN ('dispatching','outcome_unknown','accepted') AND handoff_started_at IS NOT NULL FOR UPDATE",
          [id, actor.customerID, actor.connectionID, message.provider_mid],
        )
      ).rows[0];
      if (
        !current ||
        !(
          await db.query(
            "SELECT 1 FROM webdock_auth.mail_recipient WHERE message_id=$1 AND email=$2 AND included_in_handoff AND suppression_reason IS NULL",
            [id, recipient],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "provider_identity_mismatch",
          "Observation does not match the actual handoff",
        );
      await discardExpiredUncertainContent(db, id);
      await db.query(
        "UPDATE webdock_auth.mail_message SET dispatch_status='accepted',accepted_at=coalesce(accepted_at,handoff_started_at),terminal_at=coalesce(terminal_at,now()),lease_until=NULL,error_code=NULL WHERE id=$1",
        [id],
      );
      for (const type of types)
        await recordMailEvent(
          {
            ...actor,
            resourceID: id,
            recipient,
            type,
            source: "reconciliation",
            occurredAt: new Date().toISOString(),
            dedupeKey: "reconciliation:" + observation + ":" + type,
          },
          db,
        );
      await settleMailQuota(id, "accepted", db);
      await db.query(
        "UPDATE webdock_auth.mail_message_content c SET encrypted_attachments=NULL,expires_at=m.terminal_at+interval '30 days' FROM webdock_auth.mail_message m WHERE c.message_id=m.id AND m.id=$1",
        [id],
      );
      await db.query(
        "UPDATE webdock_auth.mail_idempotency SET status='completed',expires_at=now()+interval '24 hours' WHERE message_id=$1 AND status='unknown'",
        [id],
      );
      for (const row of (
        await db.query(
          "SELECT batch_id FROM webdock_auth.mail_batch_item WHERE message_id=$1",
          [id],
        )
      ).rows)
        await refreshBatchIdempotency(db, row.batch_id);
    });
  }
  return { reconcileMail };
}
