import type { MailProductStore } from "./provider-ports";
import { createHash, randomBytes } from "node:crypto";
import type { MailActor, Permission } from "@spitzli/mail-contracts";
import { parseID } from "@spitzli/mail-contracts/validation";
import { mailUnavailable, MailError } from "./product-access";
type Dependencies = {
  authDatabase: import("@spitzli/core/identity").DatabaseReader;
  withMailDatabase: <T>(
    actor: import("@spitzli/mail-contracts").MailActor,
    run: () => Promise<T>,
  ) => Promise<T>;
  database: MailProductStore["database"];
  assertMailAccess: MailProductStore["assertMailAccess"];
  withMailTransaction: MailProductStore["withMailTransaction"];
};

export function createMailApiKeys(dependencies: Dependencies) {
  const {
    authDatabase,
    withMailDatabase,
    database,
    assertMailAccess,
    withMailTransaction,
  } = dependencies;
  const hash = (value: string) =>
    createHash("sha256").update(value).digest("hex");
  async function authorizeMailKey(
    request: Request,
    permission: Permission,
  ): Promise<MailActor> {
    const header = request.headers.get("authorization") ?? "";
    if (!/^Bearer wdm_[A-Za-z0-9_-]{43}$/.test(header))
      throw new MailError(
        401,
        "missing_api_key",
        "A valid Mail API key is required",
      );
    const row = (
      await authDatabase.query(
        "SELECT id,customer_id,connection_id,permission FROM webdock_auth.mail_api_key WHERE key_hash=$1 AND revoked_at IS NULL",
        [hash(header.slice(7))],
      )
    ).rows[0];
    if (!row)
      throw new MailError(
        401,
        "invalid_api_key",
        "Mail API key is invalid or revoked",
      );
    if (permission === "full_access" && row.permission !== "full_access")
      throw new MailError(
        403,
        "restricted_api_key",
        "Full Mail API access is required",
      );
    const actor: MailActor = {
      customerID: row.customer_id,
      connectionID: row.connection_id,
      principal: { kind: "api_key", keyID: row.id },
    };
    return withMailDatabase(actor, async () => {
      await assertMailAccess(
        actor,
        permission === "sending_access" ? "send" : "read",
        database,
        false,
      );
      await authDatabase.query(
        "UPDATE webdock_auth.mail_api_key SET last_used_at=now() WHERE id=$1 AND revoked_at IS NULL AND (last_used_at IS NULL OR last_used_at<now()-interval '1 minute')",
        [row.id],
      );
      return actor;
    });
  }
  async function createMailKey(
    actor: MailActor,
    input: { name: string; permission: Permission; domainID?: string },
  ): Promise<{ id: string; key: string; prefix: string }> {
    return withMailTransaction((db) =>
      createMailKeyInTransaction(db, actor, input),
    );
  }
  async function createMailKeyInTransaction(
    db: import("@spitzli/core/identity").DatabaseReader,
    actor: MailActor,
    input: { name: string; permission: Permission; domainID?: string },
  ): Promise<{ id: string; key: string; prefix: string }> {
    if (
      typeof input.name !== "string" ||
      !input.name.trim() ||
      input.name.length > 80 ||
      /[\x00-\x1f\x7f]/.test(input.name) ||
      !["sending_access", "full_access"].includes(input.permission)
    )
      throw new MailError(422, "validation_error", "Invalid key details");
    if (input.domainID) parseID(input.domainID);
    await assertMailAccess(actor, "manage", db);
    // Serialize creation per tenant, including separate connections, for the hourly limit.
    await db.query(
      "SELECT pg_advisory_xact_lock(hashtextextended('native-mail-keys:' || $1,0))",
      [actor.customerID],
    );
    if (actor.principal.kind === "api_key") {
      const parent = (
        await db.query(
          "SELECT domain_id FROM webdock_auth.mail_api_key WHERE id=$1",
          [actor.principal.keyID],
        )
      ).rows[0];
      if (parent?.domain_id && input.domainID !== parent.domain_id)
        mailUnavailable();
    }
    if (
      input.domainID &&
      !(
        await db.query(
          "SELECT 1 FROM webdock_auth.mail_connection_domain b JOIN webdock_auth.mail_sender_domain d ON d.id=b.domain_id WHERE b.customer_id=$1 AND b.connection_id=$2 AND d.id=$3 AND d.deleted_at IS NULL",
          [actor.customerID, actor.connectionID, input.domainID],
        )
      ).rowCount
    )
      mailUnavailable();
    if (
      Number(
        (
          await db.query(
            process.env.DATABASE_RLS_SECRET
              ? "SELECT webdock_rls.mail_admission_count($1,'keys') AS count"
              : "SELECT count(*) FROM webdock_auth.mail_api_key WHERE customer_id=$1 AND created_at>now()-interval '1 hour'",
            [actor.customerID],
          )
        ).rows[0].count,
      ) >= 10
    )
      throw new MailError(
        429,
        "rate_limit_exceeded",
        "Key creation limit reached",
      );
    const key = "wdm_" + randomBytes(32).toString("base64url"),
      prefix = key.slice(0, 12);
    const row = (
      await db.query(
        "INSERT INTO webdock_auth.mail_api_key(customer_id,connection_id,name,prefix,key_hash,permission,domain_id) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id",
        [
          actor.customerID,
          actor.connectionID,
          input.name.trim(),
          prefix,
          hash(key),
          input.permission,
          input.domainID ?? null,
        ],
      )
    ).rows[0];
    return { id: row.id, key, prefix };
  }
  async function listMailKeys(actor: MailActor) {
    await assertMailAccess(actor, "manage");
    return (
      await database.query(
        "SELECT id,name,prefix,permission,domain_id,created_at,revoked_at,last_used_at FROM webdock_auth.mail_api_key WHERE customer_id=$1 AND connection_id=$2 ORDER BY created_at DESC LIMIT 100",
        [actor.customerID, actor.connectionID],
      )
    ).rows;
  }
  async function revokeMailKey(actor: MailActor, id: string): Promise<void> {
    parseID(id);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      if (
        !(
          await db.query(
            "UPDATE webdock_auth.mail_api_key SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1 AND customer_id=$2 AND connection_id=$3",
            [id, actor.customerID, actor.connectionID],
          )
        ).rowCount
      )
        mailUnavailable();
    });
  }
  return {
    authorizeMailKey,
    createMailKey,
    createMailKeyInTransaction,
    listMailKeys,
    revokeMailKey,
  };
}
