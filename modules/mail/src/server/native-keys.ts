import { createHash, randomBytes } from "node:crypto";
import { z } from "zod";
import { type Connection } from "./native-ports";
import { mailUnavailable, type MailActor } from "./native-access";
import type { createNativeMailAuthorization } from "./native-access";
import type { NativeMailPorts } from "./native-ports";
export const nativeKeyToken = z.string().regex(/^wd_mail_[A-Za-z0-9_-]{43}$/);

const tokenHash = (value: string) =>
  createHash("sha256").update(value).digest("hex");
export interface NativeKeysDependencies {
  authorizeMail: ReturnType<
    typeof createNativeMailAuthorization
  >["authorizeMail"];
  env: NativeMailPorts["env"];
}
export function createNativeKeys(dependencies: NativeKeysDependencies) {
  const { authorizeMail, env } = dependencies;
  function requireNativeSending() {
    if (env.SPITZLI_NATIVE_MAIL_SENDING_ENABLED !== "true") mailUnavailable();
  }

  /** Caller supplies the resolved From address; this never accepts a caller-selected customer. */
  async function authorizeMailKey(
    db: Connection,
    value: string,
    sender?: string,
  ) {
    requireNativeSending();
    nativeKeyToken.parse(value);
    const key = (
      await db.query(
        `SELECT k.*,d.domain,d.status AS domain_status FROM webdock_mail.api_key k
 LEFT JOIN webdock_auth.mail_sender_domain d ON d.deleted_at IS NULL AND d.id=k.domain_id AND d.customer_id=k.customer_id
 WHERE k.token_hash=$1 AND k.revoked_at IS NULL FOR SHARE OF k`,
        [tokenHash(value)],
      )
    ).rows[0];
    if (!key) mailUnavailable();
    return authorizeKeyRow(db, key, sender);
  }

  /** Worker-only recheck by the immutable source key ID; no plaintext API token is retained. */
  async function authorizeMailKeyID(
    db: Connection,
    customerID: string,
    keyID: string,
    sender?: string,
  ) {
    requireNativeSending();
    const key = (
      await db.query(
        `SELECT k.*,d.domain,d.status AS domain_status FROM webdock_mail.api_key k
 LEFT JOIN webdock_auth.mail_sender_domain d ON d.deleted_at IS NULL AND d.id=k.domain_id AND d.customer_id=k.customer_id
 WHERE k.id=$1 AND k.customer_id=$2 AND k.revoked_at IS NULL FOR SHARE OF k`,
        [keyID, customerID],
      )
    ).rows[0];
    if (!key) mailUnavailable();
    return authorizeKeyRow(db, key, sender);
  }

  async function authorizeKeyRow(
    db: Connection,
    key: {
      customer_id: string;
      id: string;
      domain_id: string | null;
      domain_restricted: boolean;
      domain: string | null;
      domain_status: string | null;
    },
    sender?: string,
  ) {
    const customer = (
      await db.query(
        `SELECT c.id FROM webdock_admin.customers c WHERE c.id=$1 AND c.status='active'
 AND EXISTS(SELECT 1 FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true') FOR SHARE OF c`,
        [key.customer_id],
      )
    ).rows[0];
    if (
      !customer ||
      (key.domain_restricted &&
        (!key.domain_id || key.domain_status !== "ownership_verified"))
    )
      mailUnavailable();
    if (sender !== undefined) {
      const parts = sender.split("@");
      if (parts.length !== 2 || !parts[0] || /[\s\x00-\x1f]/.test(sender))
        mailUnavailable();
      if (key.domain_restricted && parts[1].toLowerCase() !== key.domain)
        mailUnavailable();
    }
    return {
      customerID: key.customer_id as string,
      keyID: key.id as string,
      domainID: key.domain_id as string | null,
      restricted: key.domain_restricted as boolean,
    };
  }

  async function manageNativeMailKey(
    db: Connection,
    actor: MailActor,
    customerID: string,
    input: unknown,
  ) {
    requireNativeSending();
    await authorizeMail(db, actor, customerID, true, false);
    const command = z
      .discriminatedUnion("action", [
        z
          .object({
            action: z.literal("create"),
            label: z
              .string()
              .trim()
              .min(1)
              .max(100)
              .regex(/^[^\x00-\x1f\x7f]+$/),
            domainID: z
              .string()
              .regex(/^[1-9][0-9]{0,18}$/)
              .optional(),
          })
          .strict(),
        z
          .object({
            action: z.literal("revoke"),
            keyID: z.string().regex(/^[1-9][0-9]{0,18}$/),
          })
          .strict(),
      ])
      .parse(input);
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      "mail-keys:" + customerID,
    ]);
    if (command.action === "revoke") {
      if (
        !(
          await db.query(
            "UPDATE webdock_mail.api_key SET revoked_at=coalesce(revoked_at,now()) WHERE id=$1 AND customer_id=$2 RETURNING id",
            [command.keyID, customerID],
          )
        ).rowCount
      )
        mailUnavailable();
      await db.query(
        "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'native-mail-key-revoke',$2,'succeeded')",
        [actor.subject, command.keyID],
      );
      return { id: command.keyID };
    }
    if (
      (
        await db.query(
          "SELECT count(*)::int AS count FROM webdock_mail.api_key WHERE customer_id=$1 AND revoked_at IS NULL",
          [customerID],
        )
      ).rows[0].count >= 100
    )
      mailUnavailable();
    if (
      command.domainID &&
      !(
        await db.query(
          "SELECT id FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND id=$1 AND customer_id=$2 AND status='ownership_verified' FOR SHARE",
          [command.domainID, customerID],
        )
      ).rowCount
    )
      mailUnavailable();
    if (
      !(
        await db.query(
          "SELECT id FROM webdock_auth.platform_settings WHERE id=true AND settings->>'mailEnabled'='true' FOR SHARE",
        )
      ).rowCount
    )
      mailUnavailable();
    const token = "wd_mail_" + randomBytes(32).toString("base64url");
    const row = (
      await db.query(
        `INSERT INTO webdock_mail.api_key(customer_id,label,token_hash,key_suffix,domain_restricted,domain_id,created_by)
 VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id`,
        [
          customerID,
          command.label,
          tokenHash(token),
          token.slice(-4),
          Boolean(command.domainID),
          command.domainID ?? null,
          actor.subject,
        ],
      )
    ).rows[0];
    await db.query(
      "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'native-mail-key-create',$2,'succeeded')",
      [actor.subject, row.id],
    );
    return { id: row.id as string, token };
  }
  return {
    requireNativeSending,
    authorizeMailKey,
    authorizeMailKeyID,
    manageNativeMailKey,
  };
}
