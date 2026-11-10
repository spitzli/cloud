import type { MailProductStore } from "./provider-ports";
import { createHash, timingSafeEqual, randomUUID } from "node:crypto";
import type { DatabaseReader } from "@spitzli/core/identity";
import type { EventType } from "@spitzli/mail-contracts";
import {
  parseAddress,
  parseID,
  parseScheduleTime,
} from "@spitzli/mail-contracts/validation";
import { MailError } from "./product-access";
type Dependencies = {
  authDatabase: import("@spitzli/core/identity").DatabaseReader;
  withDatabaseScope: <T>(
    scope: { kind: "system"; purpose: string },
    run: () => T,
  ) => T;
  discardExpiredUncertainContent: ReturnType<
    typeof import("./product-content").createMailContent
  >["discardExpiredUncertainContent"];
  database: MailProductStore["database"];
  withMailTransaction: MailProductStore["withMailTransaction"];
  readBoundedText: ReturnType<
    typeof import("./product-http").createMailHttp
  >["readBoundedText"];
  canonicalHash: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["canonicalHash"];
  refreshBatchIdempotency: ReturnType<
    typeof import("./product-idempotency").createMailIdempotency
  >["refreshBatchIdempotency"];
  recordMailEvent: ReturnType<
    typeof import("./product-events").createMailEvents
  >["recordMailEvent"];
  settleMailQuota: ReturnType<
    typeof import("./product-quota").createMailQuota
  >["settleMailQuota"];
};
export type NormalizedProviderEvent = {
  mid: string;
  providerEventID?: string;
  reference?: string;
  recipient: string;
  type: EventType;
  occurredAt: string;
  details: Record<string, unknown>;
};
export function createMailCallbackInbox(dependencies: Dependencies) {
  const {
    authDatabase,
    withDatabaseScope,
    discardExpiredUncertainContent,
    database,
    withMailTransaction,
    readBoundedText,
    canonicalHash,
    refreshBatchIdempotency,
    recordMailEvent,
    settleMailQuota,
  } = dependencies;
  // Documented delivery/engagement statuses; SPAM is the retained legacy alias for REPORT.
  const statuses: Record<string, EventType> = {
    PROCESSED: "email.sent",
    DELIVERED: "email.delivered",
    DEFERRED: "email.delivery_delayed",
    BOUNCED: "email.bounced",
    DROPPED: "email.failed",
    OPENED: "email.opened",
    CLICKED: "email.clicked",
    UNSUBSCRIBED: "email.unsubscribed",
    REPORT: "email.complained",
    SPAM: "email.complained",
  };
  function decoded(raw: string): unknown[] {
    const value = JSON.parse(
      raw,
      (key: string, value: unknown, context?: { source?: string }) =>
        ["mid", "id", "event_id"].includes(key) && typeof value === "number"
          ? context?.source
          : value,
    );
    const entries = Array.isArray(value) ? value : [value];
    if (!entries.length || entries.length > 500)
      throw new MailError(
        413,
        "callback_limit",
        "Callback event limit exceeded",
      );
    return entries;
  }
  function normalize(value: unknown): NormalizedProviderEvent {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw Error("Invalid event");
    const row = value as Record<string, unknown>;
    if (
      typeof row.mid !== "string" ||
      !/^\d{1,64}$/.test(row.mid) ||
      typeof row.status !== "string" ||
      !Object.hasOwn(statuses, row.status)
    )
      throw Error("Invalid event");
    const recipient = parseAddress(row.email),
      stamp = row.timestamp ?? row.Timestamp;
    let at: Date;
    if (
      (typeof stamp === "string" && /^\d+(?:\.\d+)?$/.test(stamp)) ||
      typeof stamp === "number"
    )
      at = new Date(Number(stamp) * 1000);
    else at = parseScheduleTime(stamp, new Date(), true);
    if (
      !Number.isFinite(at.getTime()) ||
      at.getTime() < 0 ||
      at.getTime() > Date.now() + 300000
    )
      throw Error("Invalid timestamp");
    const id = row.event_id ?? row.id,
      reference = row.reference_id;
    if (
      id !== undefined &&
      (typeof id !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(id))
    )
      throw Error("Invalid event ID");
    if (
      reference !== undefined &&
      (typeof reference !== "string" ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
          reference,
        ))
    )
      throw Error("Invalid reference");
    return {
      mid: row.mid,
      ...(id !== undefined ? { providerEventID: id as string } : {}),
      ...(reference ? { reference: reference as string } : {}),
      recipient,
      type: statuses[row.status],
      occurredAt: at.toISOString(),
      details: {},
    };
  }
  function parseProviderEvents(raw: string): NormalizedProviderEvent[] {
    return decoded(raw).map(normalize);
  }
  async function acceptProviderEvents(
    sourceID: string,
    request: Request,
  ): Promise<Response> {
    try {
      parseID(sourceID);
      if (request.method !== "POST") return new Response(null, { status: 405 });
      const source = (
        await authDatabase.query(
          "SELECT secret_hash FROM webdock_auth.mail_callback_source WHERE id=$1 AND enabled AND verified_at IS NOT NULL",
          [sourceID],
        )
      ).rows[0];
      const header = request.headers.get("authorization") ?? "";
      const credentials = /^Basic [A-Za-z0-9+/]+={0,2}$/.test(header)
        ? Buffer.from(header.slice(6), "base64").toString("utf8")
        : "";
      const secret = credentials.startsWith("webdock:")
        ? credentials.slice(8)
        : "";
      const expected = Buffer.from(
          source?.secret_hash ?? "0".repeat(64),
          "hex",
        ),
        actual = createHash("sha256").update(secret).digest();
      if (
        !source ||
        !secret ||
        expected.length !== 32 ||
        !timingSafeEqual(expected, actual)
      )
        return new Response(null, { status: 401 });
      return await withDatabaseScope(
        { kind: "system", purpose: "mail-callback" },
        async () => {
          const raw = await readBoundedText(request, 1048576);
          let entries: unknown[];
          try {
            entries = decoded(raw);
          } catch (error) {
            return new Response(null, {
              status: error instanceof MailError ? error.status : 406,
            });
          }
          let quarantined = 0;
          await withMailTransaction(async (db) => {
            const rate = await db.query(
              `UPDATE webdock_auth.mail_callback_source SET rate_window=date_trunc('minute',now()),rate_count=CASE WHEN rate_window=date_trunc('minute',now()) THEN rate_count+1 ELSE 1 END,last_seen_at=now()
    WHERE id=$1 AND enabled AND verified_at IS NOT NULL AND (rate_window IS DISTINCT FROM date_trunc('minute',now()) OR rate_count<600) RETURNING id`,
              [sourceID],
            );
            if (!rate.rowCount)
              throw new MailError(
                429,
                "rate_limited",
                "Callback rate exceeded",
              );
            const counts = (
              await db.query(
                "SELECT count(*) FILTER(WHERE status<>'processed')::int AS pending,count(*) FILTER(WHERE status='quarantined')::int AS quarantined FROM webdock_auth.mail_callback_inbox WHERE source_id=$1",
                [sourceID],
              )
            ).rows[0];
            if (counts.pending + entries.length > 10000)
              throw new MailError(
                429,
                "callback_backlog",
                "Callback backlog exceeded",
              );
            for (const entry of entries) {
              let payload: unknown,
                status = "pending";
              try {
                payload = normalize(entry);
              } catch {
                payload = { invalid: true, hash: canonicalHash(entry) };
                status = "quarantined";
                quarantined++;
                if (counts.quarantined + quarantined > 1000)
                  throw new MailError(
                    429,
                    "callback_quarantine",
                    "Callback quarantine limit exceeded",
                  );
              }
              await db.query(
                "INSERT INTO webdock_auth.mail_callback_inbox(source_id,payload,dedupe_key,status) VALUES($1,$2,$3,$4) ON CONFLICT(source_id,dedupe_key) DO NOTHING",
                [
                  sourceID,
                  JSON.stringify(payload),
                  canonicalHash(payload),
                  status,
                ],
              );
            }
          });
          return Response.json({ received: entries.length, quarantined });
        },
      );
    } catch (error) {
      return new Response(null, {
        status: error instanceof MailError ? error.status : 503,
      });
    }
  }
  /** Apply only a source-authorized event to an actual, nonsuppressed handoff recipient. */
  async function applyProviderEvent(
    db: DatabaseReader,
    sourceID: string,
    event: NormalizedProviderEvent,
  ): Promise<boolean> {
    const candidates = (
      await db.query(
        `SELECT m.id,m.customer_id FROM webdock_auth.mail_message m
 JOIN webdock_auth.mail_callback_source_connection s ON s.connection_id=m.connection_id
 JOIN webdock_auth.mail_recipient r ON r.message_id=m.id AND r.customer_id=m.customer_id AND r.connection_id=m.connection_id
 WHERE s.source_id=$1 AND r.email=$2 AND r.included_in_handoff AND r.suppression_reason IS NULL AND NOT m.simulated
 AND m.handoff_started_at IS NOT NULL AND m.dispatch_status IN ('dispatching','outcome_unknown','accepted')
 AND (($3::uuid IS NOT NULL AND m.provider_reference=$3) OR ($3 IS NULL AND m.provider_mid=$4))
 AND (m.provider_mid IS NULL OR m.provider_mid=$4) LIMIT 2`,
        [sourceID, event.recipient, event.reference ?? null, event.mid],
      )
    ).rows;
    if (candidates.length !== 1) return false;
    const candidate = candidates[0];
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      "mail-budget:" + candidate.customer_id,
    ]);
    await discardExpiredUncertainContent(db, candidate.id);
    const message = (
      await db.query(
        `UPDATE webdock_auth.mail_message SET dispatch_status='accepted',provider_mid=$2,accepted_at=coalesce(accepted_at,handoff_started_at),terminal_at=coalesce(terminal_at,now()),error_code=NULL,lease_until=NULL
 WHERE id=$1 AND dispatch_status IN ('dispatching','outcome_unknown','accepted') AND (provider_mid IS NULL OR provider_mid=$2) RETURNING *`,
        [candidate.id, event.mid],
      )
    ).rows[0];
    if (!message) return false;
    const recipients = (
      await db.query(
        "SELECT email FROM webdock_auth.mail_recipient WHERE message_id=$1 AND included_in_handoff AND suppression_reason IS NULL",
        [message.id],
      )
    ).rows;
    for (const recipient of recipients)
      await recordMailEvent(
        {
          customerID: message.customer_id,
          connectionID: message.connection_id,
          resourceID: message.id,
          type: "email.sent",
          recipient: recipient.email,
          source: "provider",
          occurredAt: new Date(message.accepted_at).toISOString(),
          dedupeKey: "accepted:" + message.id + ":" + recipient.email,
        },
        db,
      );
    const dedupe = event.providerEventID
      ? canonicalHash([
          sourceID,
          event.providerEventID,
          event.mid,
          event.recipient,
          event.type,
        ])
      : canonicalHash([sourceID, event]);
    await recordMailEvent(
      {
        customerID: message.customer_id,
        connectionID: message.connection_id,
        resourceID: message.id,
        type: event.type,
        recipient: event.recipient,
        source: "provider",
        occurredAt: event.occurredAt,
        dedupeKey: "provider:" + dedupe,
        details: event.details,
      },
      db,
    );
    await settleMailQuota(message.id, "accepted", db);
    await db.query(
      "UPDATE webdock_auth.mail_message_content c SET encrypted_attachments=NULL,expires_at=m.terminal_at+interval '30 days' FROM webdock_auth.mail_message m WHERE c.message_id=m.id AND m.id=$1",
      [message.id],
    );
    await db.query(
      "UPDATE webdock_auth.mail_idempotency SET status='completed',expires_at=now()+interval '24 hours' WHERE message_id=$1 AND status='unknown'",
      [message.id],
    );
    const batches = (
      await db.query(
        "SELECT batch_id FROM webdock_auth.mail_batch_item WHERE message_id=$1",
        [message.id],
      )
    ).rows;
    for (const batch of batches)
      await refreshBatchIdempotency(db, batch.batch_id);
    return true;
  }
  async function processCallbackInbox(input: {
    limit: number;
    deadline: Date;
  }): Promise<{ processed: number }> {
    if (!Number.isInteger(input.limit) || input.limit < 1 || input.limit > 500)
      throw new MailError(422, "invalid_limit", "Invalid inbox limit");
    const token = randomUUID();
    const rows = (
      await database.query(
        `WITH selected AS (SELECT id FROM webdock_auth.mail_callback_inbox
 WHERE status IN ('pending','quarantined','processing') AND (lease_until IS NULL OR lease_until<now()) AND NOT payload ? 'invalid'
 ORDER BY (status='pending') DESC,created_at,id FOR UPDATE SKIP LOCKED LIMIT $1)
 UPDATE webdock_auth.mail_callback_inbox i SET status='processing',claim_token=$2,lease_until=now()+interval '1 minute',attempts=attempts+1
 FROM selected s WHERE i.id=s.id RETURNING i.id`,
        [input.limit, token],
      )
    ).rows;
    let processed = 0;
    for (const row of rows) {
      if (Date.now() >= input.deadline.getTime()) break;
      try {
        const applied = await withMailTransaction(async (db) => {
          const item = (
            await db.query(
              "SELECT * FROM webdock_auth.mail_callback_inbox WHERE id=$1 AND claim_token=$2 FOR UPDATE",
              [row.id, token],
            )
          ).rows[0];
          if (!item) return false;
          const ok = await applyProviderEvent(db, item.source_id, item.payload);
          await db.query(
            "UPDATE webdock_auth.mail_callback_inbox SET status=$3,processed_at=CASE WHEN $3='processed' THEN now() ELSE NULL END,claim_token=NULL,lease_until=NULL WHERE id=$1 AND claim_token=$2",
            [item.id, token, ok ? "processed" : "quarantined"],
          );
          return ok;
        });
        if (applied) processed++;
      } catch {
        await database.query(
          "UPDATE webdock_auth.mail_callback_inbox SET status='pending',claim_token=NULL,lease_until=NULL WHERE id=$1 AND claim_token=$2",
          [row.id, token],
        );
      }
    }
    return { processed };
  }
  return {
    parseProviderEvents,
    acceptProviderEvents,
    applyProviderEvent,
    processCallbackInbox,
  };
}
