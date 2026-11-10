import type { MailProductStore } from "./provider-ports";
import type { MailActor } from "@spitzli/mail-contracts";
import {
  parsePageQuery,
  parseID,
  parseAddress,
} from "@spitzli/mail-contracts/validation";
import { createHash } from "node:crypto";
import { MailError, mailUnavailable } from "./product-access";
export type MailListKind =
  "domains" | "keys" | "webhooks" | "suppressions" | "logs";
// Only static SQL fragments from this closed resource map can become identifiers.
const resources = {
  domains: {
    select:
      "d.id,d.domain,d.status,d.token,d.deleted_at,b.provider_snapshot,b.provider_checked_at",
    from: "webdock_auth.mail_sender_domain d JOIN webdock_auth.mail_connection_domain b ON b.domain_id=d.id AND b.customer_id=d.customer_id",
    where: "d.customer_id=$1 AND b.connection_id=$2 AND d.deleted_at IS NULL",
    cursor: "d.id",
  },
  keys: {
    select:
      "d.id,d.name,d.prefix,d.permission,d.domain_id,d.created_at,d.revoked_at,d.last_used_at",
    from: "webdock_auth.mail_api_key d",
    where: "d.customer_id=$1 AND d.connection_id=$2",
    cursor: "d.id",
  },
  webhooks: {
    select: "d.id,d.url,d.events,d.enabled,d.version,d.created_at",
    from: "webdock_auth.mail_webhook d",
    where: "d.customer_id=$1 AND d.connection_id=$2 AND d.deleted_at IS NULL",
    cursor: "d.id",
  },
  suppressions: {
    select: "d.email,d.reason,d.source,d.created_at",
    from: "webdock_auth.mail_suppression d",
    where: "d.customer_id=$1 AND $2::text IS NOT NULL",
    cursor: "d.email",
  },
  logs: {
    select:
      "d.id,d.route,d.method,d.status,d.duration_ms,d.request_id,d.created_at",
    from: "webdock_auth.mail_request_log d",
    where: "d.customer_id=$1 AND d.connection_id=$2",
    cursor: "d.id",
  },
} as const;
export function createMailResourceLists(
  dependencies: Pick<MailProductStore, "database" | "assertMailAccess">,
) {
  const { database, assertMailAccess } = dependencies;
  async function listMailResource(
    actor: MailActor,
    kind: MailListKind,
    query: {
      limit?: number;
      after?: string;
      search?: string;
      method?: string;
      status?: number;
    } = {},
  ) {
    await assertMailAccess(
      actor,
      kind === "logs" ? "manage" : "read",
      database,
      false,
    );
    const { limit } = parsePageQuery(query),
      spec = resources[kind];
    const search = query.search ?? "",
      method = query.method ?? "",
      status = query.status ?? null;
    if (
      typeof search !== "string" ||
      search.length > 200 ||
      typeof method !== "string" ||
      (method &&
        !["GET", "POST", "PATCH", "PUT", "DELETE", "HEAD", "OPTIONS"].includes(
          method,
        )) ||
      (status !== null &&
        (!Number.isInteger(status) ||
          (status !== 0 && (status < 100 || status > 599)))) ||
      (kind !== "logs" && (method || status !== null))
    )
      throw new MailError(422, "validation_error", "Invalid list filters");
    const searchable = {
      domains: "d.domain",
      keys: "d.name||' '||d.prefix",
      webhooks: "d.url",
      suppressions: "d.email",
      logs: "d.route||' '||d.request_id",
    }[kind];
    const scope = createHash("sha256")
      .update(
        JSON.stringify([
          actor.customerID,
          actor.connectionID,
          kind,
          search,
          method,
          status,
        ]),
      )
      .digest("hex");
    let after: string | null = null;
    if (query.after) {
      try {
        const data = JSON.parse(
          Buffer.from(query.after, "base64url").toString(),
        );
        if (data.s !== scope || typeof data.id !== "string") throw Error();
        after =
          kind === "suppressions" ? parseAddress(data.id) : parseID(data.id);
      } catch {
        throw new MailError(
          422,
          "invalid_cursor",
          "Cursor does not match this resource",
        );
      }
    }
    const rows = (
      await database.query(
        `SELECT ${spec.select} FROM ${spec.from} WHERE ${spec.where} AND ($3::text IS NULL OR ${spec.cursor}<$3) AND ($5='' OR position(lower($5) in lower(${searchable}))>0) ${kind === "logs" ? "AND ($6='' OR d.method=$6) AND ($7::int IS NULL OR d.status=$7)" : "AND $6::text='' AND $7::int IS NULL"} ORDER BY ${spec.cursor} DESC LIMIT $4`,
        [
          actor.customerID,
          actor.connectionID,
          after,
          limit + 1,
          search,
          method,
          status,
        ],
      )
    ).rows;
    const more = rows.length > limit;
    if (more) rows.pop();
    const last = rows.at(-1);
    return {
      data: rows,
      nextCursor:
        more && last
          ? Buffer.from(
              JSON.stringify({
                s: scope,
                id: kind === "suppressions" ? last.email : last.id,
              }),
            ).toString("base64url")
          : null,
    };
  }
  async function getMailLog(actor: MailActor, id: string) {
    parseID(id);
    await assertMailAccess(actor, "manage", database, false);
    const row = (
      await database.query(
        "SELECT id,route,method,status,duration_ms,request_id,request_body,response_body,created_at FROM webdock_auth.mail_request_log WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
        [id, actor.customerID, actor.connectionID],
      )
    ).rows[0];
    if (!row) mailUnavailable();
    return row;
  }

  return { listMailResource, getMailLog };
}
