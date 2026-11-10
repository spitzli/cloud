import type { MailProductStore } from "./provider-ports";
import { createHash } from "node:crypto";
import type { MailActor } from "@spitzli/mail-contracts";
import { parseID, parsePageQuery } from "@spitzli/mail-contracts/validation";
import { MailError, mailUnavailable } from "./product-access";
type Dependencies = {
  database: MailProductStore["database"];
  assertMailAccess: MailProductStore["assertMailAccess"];
};
export type MessageQuery = {
  limit?: number;
  after?: string;
  search?: string;
  domainID?: string;
  status?: string;
  includeSimulated?: boolean;
};
export function createMailMessages(dependencies: Dependencies) {
  const { database, assertMailAccess } = dependencies;
  const fields = `m.id,m.domain_id,m.from_address,m.subject,m.dispatch_status,m.simulated,m.scheduled_at,m.accepted_at,m.created_at,m.revision,m.error_code,m.blocked_reason`;
  async function listMailMessages(actor: MailActor, query: MessageQuery = {}) {
    await assertMailAccess(actor, "read");
    const { limit } = parsePageQuery(query);
    if (query.domainID) parseID(query.domainID);
    const search = query.search ?? "";
    if (
      typeof search !== "string" ||
      search.length > 200 ||
      typeof (query.includeSimulated ?? false) !== "boolean"
    )
      throw new MailError(422, "validation_error", "Invalid message filters");
    if (
      query.status &&
      ![
        "scheduled",
        "dispatching",
        "accepted",
        "failed",
        "outcome_unknown",
        "canceled",
        "suppressed",
      ].includes(query.status)
    )
      throw new MailError(422, "validation_error", "Invalid status filter");
    const hash = createHash("sha256")
      .update(
        JSON.stringify([
          actor.customerID,
          actor.connectionID,
          search,
          query.domainID ?? "",
          query.status ?? "",
          !!query.includeSimulated,
        ]),
      )
      .digest("hex");
    let afterTime: string | null = null,
      afterID: string | null = null;
    if (query.after) {
      try {
        if (query.after.length > 512) throw Error();
        const cursor = JSON.parse(
          Buffer.from(query.after, "base64url").toString("utf8"),
        );
        if (
          cursor.f !== hash ||
          typeof cursor.t !== "string" ||
          !Number.isFinite(Date.parse(cursor.t))
        )
          throw Error();
        afterTime = cursor.t;
        afterID = parseID(cursor.id);
      } catch {
        throw new MailError(
          422,
          "invalid_cursor",
          "Cursor does not match these filters",
        );
      }
    }
    const rows = (
      await database.query(
        `SELECT ${fields},to_char(m.created_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS cursor_time,
 (SELECT count(*)::int FROM webdock_auth.mail_recipient r WHERE r.message_id=m.id) AS recipient_count
 FROM webdock_auth.mail_message m WHERE m.customer_id=$1 AND m.connection_id=$2 AND ($3::text IS NULL OR m.domain_id=$3) AND ($4::text IS NULL OR m.dispatch_status=$4)
 AND ($5 OR NOT m.simulated) AND ($6='' OR position(lower($6) in lower(m.subject))>0 OR position(lower($6) in lower(m.from_address))>0 OR m.id=$6 OR EXISTS(SELECT 1 FROM webdock_auth.mail_recipient r WHERE r.message_id=m.id AND r.category<>'bcc' AND position(lower($6) in r.email)>0))
 AND ($7::timestamptz IS NULL OR (m.created_at,m.id)<($7,$8)) ORDER BY m.created_at DESC,m.id DESC LIMIT $9`,
        [
          actor.customerID,
          actor.connectionID,
          query.domainID ?? null,
          query.status ?? null,
          !!query.includeSimulated,
          search,
          afterTime,
          afterID,
          limit + 1,
        ],
      )
    ).rows;
    const more = rows.length > limit;
    if (more) rows.pop();
    const last = rows.at(-1);
    return {
      data: rows.map(({ cursor_time: unused, ...row }) => {
        void unused;
        return row;
      }),
      nextCursor:
        more && last
          ? Buffer.from(
              JSON.stringify({ t: last.cursor_time, id: last.id, f: hash }),
            ).toString("base64url")
          : null,
    };
  }
  async function getMailMessage(actor: MailActor, id: string) {
    parseID(id);
    await assertMailAccess(actor, "read");
    const message = (
      await database.query(
        `SELECT ${fields},m.provider_mid,m.terminal_at FROM webdock_auth.mail_message m WHERE m.id=$1 AND m.customer_id=$2 AND m.connection_id=$3`,
        [id, actor.customerID, actor.connectionID],
      )
    ).rows[0];
    if (!message) mailUnavailable();
    const [recipients, content, events] = await Promise.all([
      database.query(
        "SELECT email,category,delivery_status,suppression_reason,first_delivered_at,first_open_at,last_open_at,first_click_at,last_click_at,complained_at,unsubscribed_at FROM webdock_auth.mail_recipient WHERE message_id=$1 AND customer_id=$2 AND connection_id=$3 ORDER BY category,email",
        [id, actor.customerID, actor.connectionID],
      ),
      database.query(
        `SELECT c.text_body,c.html_body,c.headers,c.tags,c.reply_to,c.attachment_metadata,e.at AS expires_at,e.at IS NULL OR e.at>now() AS available
 FROM webdock_auth.mail_message_content c JOIN webdock_auth.mail_message m ON m.id=c.message_id
 CROSS JOIN LATERAL(SELECT CASE WHEN m.dispatch_status='outcome_unknown' THEN least(c.expires_at,coalesce(m.handoff_started_at,m.created_at)+interval '7 days') WHEN m.terminal_at IS NOT NULL THEN least(c.expires_at,m.terminal_at+interval '30 days') ELSE c.expires_at END AS at) e
 WHERE c.message_id=$1 AND c.customer_id=$2 AND c.connection_id=$3`,
        [id, actor.customerID, actor.connectionID],
      ),
      database.query(
        "SELECT id,type,recipient,source,occurred_at,details FROM webdock_auth.mail_event WHERE resource_id=$1 AND customer_id=$2 AND connection_id=$3 ORDER BY occurred_at DESC,id DESC LIMIT 501",
        [id, actor.customerID, actor.connectionID],
      ),
    ]);
    const row = content.rows[0],
      available = !!row?.available;
    return {
      ...message,
      recipients: recipients.rows,
      content_available: available,
      content_expires_at: row?.expires_at ?? null,
      content: available
        ? {
            text: row.text_body,
            html: row.html_body,
            headers: row.headers,
            tags: row.tags,
            reply_to: row.reply_to,
            attachments: row.attachment_metadata,
          }
        : null,
      events: events.rows.slice(0, 500),
      events_truncated: events.rows.length > 500,
    };
  }
  return { listMailMessages, getMailMessage };
}
