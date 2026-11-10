import type { MailProductStore } from "./provider-ports";
import { createHash, randomUUID } from "node:crypto";
import type { MailActor, EventType } from "@spitzli/mail-contracts";
import { isMailEvent } from "@spitzli/mail-contracts";
import { parseID, parsePageQuery } from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
import {
  webhookEndpointProblem,
  postMailWebhook,
} from "./product-webhook-transport";
import { newWebhookSecret, signWebhook } from "./product-webhook-signing";
type Dependencies = {
  database: MailProductStore["database"];
  encryptMailSecret: import("./provider-ports").MailSecrets["encryptMailSecret"];
  decryptMailSecret: import("./provider-ports").MailSecrets["decryptMailSecret"];
  withMailTransaction: MailProductStore["withMailTransaction"];
  assertMailAccess: MailProductStore["assertMailAccess"];
};

export function createMailWebhooks(dependencies: Dependencies) {
  const {
    database,
    encryptMailSecret,
    decryptMailSecret,
    withMailTransaction,
    assertMailAccess,
  } = dependencies;
  const purpose = (
    customer: string,
    connection: string,
    id: string,
    version: number,
  ) => `mail-webhook:${customer}:${connection}:${id}:${version}`;
  function validate(input: {
    url?: string;
    events?: EventType[];
    enabled?: boolean;
  }) {
    if (input.url !== undefined) {
      const error = webhookEndpointProblem(input.url);
      if (error) throw new MailError(422, "validation_error", error);
    }
    if (
      input.events !== undefined &&
      (!Array.isArray(input.events) ||
        !input.events.length ||
        input.events.length > 15 ||
        !input.events.every(isMailEvent))
    )
      throw new MailError(
        422,
        "validation_error",
        "Choose valid webhook events",
      );
    if (input.enabled !== undefined && typeof input.enabled !== "boolean")
      throw new MailError(422, "validation_error", "Invalid enabled flag");
  }
  async function createMailWebhook(
    actor: MailActor,
    input: { url: string; events: EventType[] },
  ): Promise<{ id: string; secret: string }> {
    validate(input);
    if (!input.url || !input.events)
      throw new MailError(422, "validation_error", "URL and events required");
    return withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      await db.query(
        "SELECT id FROM webdock_auth.mail_connection WHERE id=$1 FOR UPDATE",
        [actor.connectionID],
      );
      if (
        Number(
          (
            await db.query(
              "SELECT count(*) FROM webdock_auth.mail_webhook WHERE connection_id=$1 AND deleted_at IS NULL",
              [actor.connectionID],
            )
          ).rows[0].count,
        ) >= 20
      )
        throw new MailError(
          429,
          "webhook_limit",
          "Webhook endpoint limit reached",
        );
      const id = (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
          .rows[0].id,
        secret = newWebhookSecret();
      await db.query(
        "INSERT INTO webdock_auth.mail_webhook(id,customer_id,connection_id,url,events,encrypted_secret) VALUES($1,$2,$3,$4,$5,$6)",
        [
          id,
          actor.customerID,
          actor.connectionID,
          input.url,
          JSON.stringify([...new Set(input.events)]),
          encryptMailSecret(
            { secret },
            purpose(actor.customerID, actor.connectionID, id, 1),
          ),
        ],
      );
      return { id, secret };
    });
  }
  async function listMailWebhooks(actor: MailActor) {
    await assertMailAccess(actor, "read");
    return (
      await database.query(
        "SELECT id,url,events,enabled,version,created_at FROM webdock_auth.mail_webhook WHERE customer_id=$1 AND connection_id=$2 AND deleted_at IS NULL ORDER BY created_at DESC,id DESC LIMIT 100",
        [actor.customerID, actor.connectionID],
      )
    ).rows;
  }
  async function getMailWebhook(actor: MailActor, id: string) {
    parseID(id);
    await assertMailAccess(actor, "read");
    const row = (
      await database.query(
        "SELECT id,url,events,enabled,version,created_at FROM webdock_auth.mail_webhook WHERE customer_id=$1 AND connection_id=$2 AND id=$3 AND deleted_at IS NULL",
        [actor.customerID, actor.connectionID, id],
      )
    ).rows[0];
    if (!row) mailUnavailable();
    return row;
  }
  async function updateMailWebhook(
    actor: MailActor,
    id: string,
    input: { url?: string; events?: EventType[]; enabled?: boolean },
  ): Promise<void> {
    parseID(id);
    validate(input);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      if (
        !(
          await db.query(
            "UPDATE webdock_auth.mail_webhook SET url=coalesce($4,url),events=coalesce($5,events),enabled=coalesce($6,enabled) WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND deleted_at IS NULL",
            [
              id,
              actor.customerID,
              actor.connectionID,
              input.url ?? null,
              input.events ? JSON.stringify([...new Set(input.events)]) : null,
              input.enabled ?? null,
            ],
          )
        ).rowCount
      )
        mailUnavailable();
    });
  }
  async function deleteMailWebhook(
    actor: MailActor,
    id: string,
  ): Promise<void> {
    parseID(id);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      if (
        !(
          await db.query(
            "UPDATE webdock_auth.mail_webhook SET deleted_at=coalesce(deleted_at,now()),enabled=false WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
            [id, actor.customerID, actor.connectionID],
          )
        ).rowCount
      )
        mailUnavailable();
      await db.query(
        "UPDATE webdock_auth.mail_webhook_delivery SET status='canceled',finished_at=now() WHERE webhook_id=$1 AND status='pending'",
        [id],
      );
    });
  }
  async function rotateMailWebhook(
    actor: MailActor,
    id: string,
  ): Promise<{ secret: string }> {
    parseID(id);
    return withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      const row = (
        await db.query(
          "SELECT version FROM webdock_auth.mail_webhook WHERE id=$1 AND customer_id=$2 AND connection_id=$3 AND deleted_at IS NULL FOR UPDATE",
          [id, actor.customerID, actor.connectionID],
        )
      ).rows[0];
      if (!row) mailUnavailable();
      const secret = newWebhookSecret(),
        version = row.version + 1;
      await db.query(
        "UPDATE webdock_auth.mail_webhook SET version=$2,encrypted_secret=$3 WHERE id=$1",
        [
          id,
          version,
          encryptMailSecret(
            { secret },
            purpose(actor.customerID, actor.connectionID, id, version),
          ),
        ],
      );
      return { secret };
    });
  }
  async function replayMailWebhook(
    actor: MailActor,
    id: string,
  ): Promise<void> {
    parseID(id);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      const row = (
        await db.query(
          "SELECT d.id,d.lease_until FROM webdock_auth.mail_webhook_delivery d JOIN webdock_auth.mail_webhook w ON w.id=d.webhook_id WHERE d.id=$1 AND d.customer_id=$2 AND d.connection_id=$3 AND w.deleted_at IS NULL FOR UPDATE OF d",
          [id, actor.customerID, actor.connectionID],
        )
      ).rows[0];
      if (!row) mailUnavailable();
      if (row.lease_until && new Date(row.lease_until) > new Date())
        throw new MailError(
          409,
          "delivery_busy",
          "Delivery is currently leased",
        );
      await db.query(
        "UPDATE webdock_auth.mail_webhook_delivery SET status='pending',attempts=0,next_at=now(),finished_at=NULL,claim_token=NULL,lease_until=NULL WHERE id=$1",
        [id],
      );
    });
  }
  async function listMailWebhookDeliveries(
    actor: MailActor,
    id: string,
    query: { after?: string; limit?: number } = {},
  ) {
    await getMailWebhook(actor, id);
    const { limit } = parsePageQuery(query);
    let after: string | null = null;
    if (query.after) {
      try {
        const cursor = JSON.parse(
          Buffer.from(query.after, "base64url").toString(),
        );
        if (cursor.webhook !== id || cursor.customer !== actor.customerID)
          throw Error();
        after = parseID(cursor.id);
      } catch {
        throw new MailError(
          422,
          "invalid_cursor",
          "Cursor does not match this webhook",
        );
      }
    }
    const rows = (
      await database.query(
        `SELECT d.id,d.event_id,d.status,d.attempts,d.next_at,d.created_at,d.finished_at,
 (SELECT count(*)::int FROM webdock_auth.mail_webhook_attempt a WHERE a.delivery_id=d.id) AS history_count,
 coalesce((SELECT jsonb_agg(h ORDER BY h.created_at DESC,h.id DESC) FROM (SELECT a.id,a.http_status,a.response,a.error_code,a.duration_ms,a.created_at FROM webdock_auth.mail_webhook_attempt a WHERE a.delivery_id=d.id ORDER BY a.created_at DESC,a.id DESC LIMIT 7) h),'[]'::jsonb) AS history
 FROM webdock_auth.mail_webhook_delivery d WHERE d.webhook_id=$1 AND d.customer_id=$2 AND d.connection_id=$3 AND ($4::text IS NULL OR d.id<$4) ORDER BY d.id DESC LIMIT $5`,
        [id, actor.customerID, actor.connectionID, after, limit + 1],
      )
    ).rows;
    const more = rows.length > limit;
    if (more) rows.pop();
    return {
      data: rows,
      nextCursor: more
        ? Buffer.from(
            JSON.stringify({
              id: rows.at(-1)!.id,
              webhook: id,
              customer: actor.customerID,
            }),
          ).toString("base64url")
        : null,
    };
  }
  const delays = [1, 5, 30, 120, 300, 600];
  async function deliverDueWebhooks(
    input: { limit: number; deadline: Date },
    options: { transport?: typeof postMailWebhook } = {},
  ): Promise<{ processed: number }> {
    if (!Number.isInteger(input.limit) || input.limit < 1 || input.limit > 100)
      throw new MailError(422, "invalid_limit", "Invalid delivery limit");
    await database.query(`WITH expired AS (UPDATE webdock_auth.mail_webhook_delivery SET status='failed',finished_at=now(),claim_token=NULL,lease_until=NULL
  WHERE status='delivering' AND attempts>=7 AND lease_until<now() RETURNING id,customer_id,connection_id)
  INSERT INTO webdock_auth.mail_webhook_attempt(customer_id,connection_id,delivery_id,error_code,duration_ms) SELECT customer_id,connection_id,id,'lease_expired',0 FROM expired`);
    let processed = 0;
    for (
      let offset = 0;
      offset < input.limit && Date.now() < input.deadline.getTime();
      offset += 4
    ) {
      const token = randomUUID();
      const rows = (
        await database.query(
          `WITH selected AS (SELECT d.id FROM webdock_auth.mail_webhook_delivery d
   JOIN webdock_auth.mail_webhook w ON w.id=d.webhook_id JOIN webdock_admin.customers c ON c.id=d.customer_id JOIN webdock_auth.platform_settings p ON p.id=true
   WHERE d.status IN ('pending','delivering') AND d.next_at<=now() AND (d.lease_until IS NULL OR d.lease_until<now()) AND d.attempts<7
   AND w.enabled AND w.deleted_at IS NULL AND c.status='active' AND p.settings->>'mailGatewayEnabled'='true' AND p.settings->>'mailWebhooksEnabled'='true'
   ORDER BY d.next_at,d.id FOR UPDATE OF d SKIP LOCKED LIMIT $1)
   UPDATE webdock_auth.mail_webhook_delivery d SET status='delivering',claim_token=$2,lease_until=now()+interval '2 minutes',attempts=attempts+1
   FROM selected s WHERE d.id=s.id RETURNING d.*`,
          [Math.min(4, input.limit - offset), token],
        )
      ).rows;
      if (!rows.length) break;
      await Promise.all(
        rows.map(async (row) => {
          const endpoint = (
            await database.query(
              "SELECT url,encrypted_secret,version,enabled,deleted_at FROM webdock_auth.mail_webhook WHERE id=$1",
              [row.webhook_id],
            )
          ).rows[0];
          if (!endpoint || !endpoint.enabled || endpoint.deleted_at) {
            await database.query(
              "UPDATE webdock_auth.mail_webhook_delivery SET status=$3,finished_at=CASE WHEN $3='canceled' THEN now() ELSE NULL END,claim_token=NULL,lease_until=NULL,attempts=attempts-1 WHERE id=$1 AND claim_token=$2",
              [
                row.id,
                token,
                !endpoint || endpoint.deleted_at ? "canceled" : "pending",
              ],
            );
            return;
          }
          const started = Date.now();
          let status = 0,
            response = "",
            error: string | null = null;
          try {
            const { secret } = decryptMailSecret<{ secret: string }>(
              endpoint.encrypted_secret,
              purpose(
                row.customer_id,
                row.connection_id,
                row.webhook_id,
                endpoint.version,
              ),
            );
            const timestamp = Math.floor(Date.now() / 1000);
            const headers = {
              "content-type": "application/json",
              "webhook-id": row.id,
              "webhook-timestamp": String(timestamp),
              "webhook-signature": signWebhook(
                secret,
                row.id,
                timestamp,
                row.body,
              ),
            };
            const result = await (options.transport ?? postMailWebhook)(
              endpoint.url,
              headers,
              row.body,
              {
                signal: AbortSignal.timeout(
                  Math.max(1, input.deadline.getTime() - Date.now()),
                ),
              },
            );
            status = result.status;
            response = result.response;
          } catch {
            error = "transport_error";
          }
          await withMailTransaction(async (db) => {
            const current = (
              await db.query(
                "SELECT id FROM webdock_auth.mail_webhook_delivery WHERE id=$1 AND claim_token=$2 FOR UPDATE",
                [row.id, token],
              )
            ).rows[0];
            if (!current) return;
            const success = status >= 200 && status < 300,
              retry = !success && row.attempts < 7;
            const redacted = response
              ? `[redacted ${Buffer.byteLength(response)} bytes; sha256 ${createHash("sha256").update(response).digest("hex")}]`
              : "";
            await db.query(
              "INSERT INTO webdock_auth.mail_webhook_attempt(customer_id,connection_id,delivery_id,http_status,response,error_code,duration_ms) VALUES($1,$2,$3,$4,$5,$6,$7)",
              [
                row.customer_id,
                row.connection_id,
                row.id,
                status || null,
                redacted,
                error,
                Math.max(0, Date.now() - started),
              ],
            );
            await db.query(
              "UPDATE webdock_auth.mail_webhook_delivery SET status=$3,next_at=now()+($4::integer*interval '1 minute'),finished_at=CASE WHEN $3='pending' THEN NULL ELSE now() END,claim_token=NULL,lease_until=NULL WHERE id=$1 AND claim_token=$2",
              [
                row.id,
                token,
                success ? "succeeded" : retry ? "pending" : "failed",
                retry ? delays[row.attempts - 1] : 0,
              ],
            );
            processed++;
          });
        }),
      );
    }
    return { processed };
  }
  return {
    createMailWebhook,
    listMailWebhooks,
    getMailWebhook,
    updateMailWebhook,
    deleteMailWebhook,
    rotateMailWebhook,
    replayMailWebhook,
    listMailWebhookDeliveries,
    deliverDueWebhooks,
  };
}
