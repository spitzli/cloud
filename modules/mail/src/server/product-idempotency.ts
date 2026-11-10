import type { MailProductStore } from "./provider-ports";
import { createHash, randomUUID } from "node:crypto";
import { MailValidationError } from "@spitzli/mail-contracts/validation";
import type { MailActor, ApiResult } from "@spitzli/mail-contracts";
import { MailError } from "./product-access";
import type { DatabaseReader } from "@spitzli/core/identity";
export function createMailIdempotency(
  dependencies: Pick<
    MailProductStore,
    "database" | "withMailTransaction" | "assertMailAccess"
  >,
) {
  const { database, withMailTransaction, assertMailAccess } = dependencies;
  function canonicalHash(value: unknown): string {
    const canonical = (v: unknown): string => {
      if (Array.isArray(v)) return "[" + v.map(canonical).join(",") + "]";
      if (v && typeof v === "object")
        return (
          "{" +
          Object.keys(v)
            .sort()
            .map(
              (k) =>
                JSON.stringify(k) +
                ":" +
                canonical((v as Record<string, unknown>)[k]),
            )
            .join(",") +
          "}"
        );
      const out = JSON.stringify(v);
      if (out === undefined)
        throw new MailError(422, "validation_error", "Invalid JSON value");
      return out;
    };
    return createHash("sha256").update(canonical(value)).digest("hex");
  }
  async function reserveMailRequest(
    actor: MailActor,
    input: {
      key: string;
      method: string;
      path: string;
      body: unknown;
      now: Date;
    },
  ): Promise<
    { kind: "reserved"; token: string } | { kind: "replay"; result: ApiResult }
  > {
    if (
      typeof input.key !== "string" ||
      !/^[\x21-\x7e]{1,256}$/.test(input.key)
    )
      throw new MailError(422, "validation_error", "Invalid idempotency key");
    const hash = canonicalHash(input.body);
    return withMailTransaction(async (db) => {
      await assertMailAccess(actor, "send", db, false);
      // The lock serializes even the initial absence; ON CONFLICT alone cannot fence cleanup.
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        actor.customerID + ":" + actor.connectionID + ":" + input.key,
      ]);
      let existing = (
        await db.query(
          "SELECT * FROM webdock_auth.mail_idempotency WHERE customer_id=$1 AND connection_id=$2 AND key=$3 FOR UPDATE",
          [actor.customerID, actor.connectionID, input.key],
        )
      ).rows[0];
      if (
        existing?.status === "completed" &&
        existing.expires_at &&
        new Date(existing.expires_at) <= input.now
      ) {
        const work = await workState(
          db,
          existing.message_id,
          existing.batch_id,
        );
        if (work.unresolved || work.processing) {
          await db.query(
            "UPDATE webdock_auth.mail_idempotency SET status=$4,expires_at=NULL WHERE customer_id=$1 AND connection_id=$2 AND key=$3",
            [
              actor.customerID,
              actor.connectionID,
              input.key,
              work.unresolved ? "unknown" : "pending",
            ],
          );
        } else {
          await db.query(
            "DELETE FROM webdock_auth.mail_idempotency WHERE customer_id=$1 AND connection_id=$2 AND key=$3",
            [actor.customerID, actor.connectionID, input.key],
          );
          existing = undefined;
        }
      }
      if (existing) {
        if (
          existing.method !== input.method ||
          existing.path !== input.path ||
          existing.body_hash !== hash
        )
          throw new MailError(
            409,
            "idempotency_conflict",
            "Idempotency key was used for another request",
          );
        if (existing.response_status !== null)
          return {
            kind: "replay",
            result: {
              status: existing.response_status,
              body: existing.response_body,
            },
          };
        if (existing.message_id) {
          const message = (
            await db.query(
              "SELECT dispatch_status FROM webdock_auth.mail_message WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
              [existing.message_id, actor.customerID, actor.connectionID],
            )
          ).rows[0];
          const uncertain = message?.dispatch_status === "outcome_unknown";
          return {
            kind: "replay",
            result: {
              status: uncertain ? 502 : 409,
              body: {
                statusCode: uncertain ? 502 : 409,
                name: uncertain
                  ? "send_outcome_unknown"
                  : "request_in_progress",
                message: uncertain
                  ? "Provider acceptance is uncertain; do not resend"
                  : "This request is already in progress",
                email_id: existing.message_id,
                request_id: existing.operation_token,
              },
            },
          };
        }
        throw new MailError(
          409,
          "request_in_progress",
          "This request is already in progress",
        );
      }
      await assertMailAccess(actor, "send", db);
      const token = randomUUID();
      await db.query(
        "INSERT INTO webdock_auth.mail_idempotency(customer_id,connection_id,key,method,path,body_hash,status,created_at,operation_token) VALUES($1,$2,$3,$4,$5,$6,'pending',$7,$8)",
        [
          actor.customerID,
          actor.connectionID,
          input.key,
          input.method,
          input.path,
          hash,
          input.now,
          token,
        ],
      );
      return { kind: "reserved", token };
    });
  }
  /** Internal completion only, never a customer HTTP action. Unknown results have no TTL. */
  async function completeMailRequest(
    actor: MailActor,
    key: string,
    token: string,
    result: ApiResult,
    unknown: boolean,
    now: Date,
  ): Promise<void> {
    await withMailTransaction(async (db) => {
      const current = (
        await db.query(
          "SELECT message_id,batch_id FROM webdock_auth.mail_idempotency WHERE customer_id=$1 AND connection_id=$2 AND key=$3 AND operation_token=$4 AND status='pending' FOR UPDATE",
          [actor.customerID, actor.connectionID, key, token],
        )
      ).rows[0];
      if (!current) return;
      const work = await workState(db, current.message_id, current.batch_id);
      const state =
        unknown || work.unresolved
          ? "unknown"
          : work.processing
            ? "pending"
            : "completed";
      await db.query(
        "UPDATE webdock_auth.mail_idempotency SET status=$4,response_status=$5,response_body=$6,expires_at=$7 WHERE customer_id=$1 AND connection_id=$2 AND key=$3 AND operation_token=$8",
        [
          actor.customerID,
          actor.connectionID,
          key,
          state,
          result.status,
          JSON.stringify(result.body),
          state === "completed" ? new Date(now.getTime() + 86400_000) : null,
          token,
        ],
      );
    });
  }
  async function runIdempotent(
    actor: MailActor,
    input: { key: string; method: string; path: string; body: unknown },
    run: (token: string) => Promise<ApiResult>,
    now = new Date(),
  ): Promise<ApiResult> {
    let reservation;
    try {
      reservation = await reserveMailRequest(actor, { ...input, now });
    } catch (error) {
      if (error instanceof MailError)
        return {
          status: error.status,
          body: {
            statusCode: error.status,
            name: error.code,
            message: error.message,
          },
        };
      throw error;
    }
    if (reservation.kind === "replay") return reservation.result;
    let result: ApiResult;
    try {
      result = await run(reservation.token);
    } catch (error) {
      if (
        !(error instanceof MailError) &&
        !(error instanceof MailValidationError)
      )
        throw error;
      const status = error instanceof MailError ? error.status : 422;
      result = {
        status,
        body: {
          statusCode: status,
          name: error instanceof MailError ? error.code : "validation_error",
          message: error.message,
          request_id: randomUUID(),
        },
      };
    }
    let unknown =
      result.body !== null &&
      typeof result.body === "object" &&
      "name" in result.body &&
      result.body.name === "send_outcome_unknown";
    if (!unknown) {
      unknown = !!(
        await database.query(
          `SELECT 1 FROM webdock_auth.mail_idempotency i JOIN webdock_auth.mail_batch_item bi ON bi.batch_id=i.batch_id
   JOIN webdock_auth.mail_message m ON m.id=bi.message_id WHERE i.customer_id=$1 AND i.connection_id=$2 AND i.key=$3
   AND m.dispatch_status IN ('dispatching','outcome_unknown') LIMIT 1`,
          [actor.customerID, actor.connectionID, input.key],
        )
      ).rowCount;
    }
    try {
      await completeMailRequest(
        actor,
        input.key,
        reservation.token,
        result,
        unknown,
        now,
      );
    } catch (error) {
      const body = result.body as { id?: unknown; email_id?: unknown } | null;
      const id =
        body && typeof body === "object"
          ? (body.email_id ?? body.id)
          : undefined;
      if (typeof id === "string")
        return {
          status: 502,
          body: {
            statusCode: 502,
            name: "send_outcome_unknown",
            message: "Saved response is uncertain; do not resend",
            email_id: id,
          },
        };
      throw error;
    }
    return result;
  }

  async function workState(
    db: DatabaseReader,
    messageID: string | null,
    batchID: string | null,
  ): Promise<{ unresolved: boolean; processing: boolean }> {
    return (
      await db.query(
        `SELECT EXISTS(SELECT 1 FROM webdock_auth.mail_message m WHERE (m.id=$1 OR m.id IN (SELECT message_id FROM webdock_auth.mail_batch_item WHERE batch_id=$2)) AND m.dispatch_status IN ('dispatching','outcome_unknown')) AS unresolved,
 EXISTS(SELECT 1 FROM webdock_auth.mail_batch WHERE id=$2 AND status='processing') AS processing`,
        [messageID, batchID],
      )
    ).rows[0];
  }
  async function refreshBatchIdempotency(
    db: DatabaseReader,
    batchID: string,
  ): Promise<void> {
    const work = await workState(db, null, batchID),
      state = work.unresolved
        ? "unknown"
        : work.processing
          ? "pending"
          : "completed";
    await db.query(
      "UPDATE webdock_auth.mail_idempotency SET status=$2,expires_at=CASE WHEN $2='completed' THEN coalesce(expires_at,now()+interval '24 hours') ELSE NULL END WHERE batch_id=$1 AND response_status IS NOT NULL",
      [batchID, state],
    );
  }

  return {
    canonicalHash,
    reserveMailRequest,
    completeMailRequest,
    runIdempotent,
    refreshBatchIdempotency,
  };
}
