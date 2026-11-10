import { randomUUID, createHash, timingSafeEqual } from "node:crypto";
import type { MailActor } from "@spitzli/mail-contracts";
import { mailUnavailable, MailError } from "./product-access";
import type {
  MailCredentialMaster,
  MailCredentialIssuer,
  MailSecrets,
  MailProductStore,
} from "./provider-ports";
export type CredentialOverrides = {
  master?: () => Promise<MailCredentialMaster>;
  child?: (token: string) => MailCredentialIssuer;
};
type Dependencies = CredentialOverrides;
export function createMailCredentials(
  dependencies: MailProductStore &
    MailSecrets & {
      master(actor: MailActor): Promise<MailCredentialMaster>;
      child(token: string): MailCredentialIssuer;
    },
) {
  const {
    database,
    withMailTransaction,
    assertMailAccess,
    encryptMailSecret,
    decryptMailSecret,
  } = dependencies;
  async function ensureManagedSendingCredential(
    actor: MailActor,
    deps: Dependencies = {},
    options: { rotate?: boolean } = {},
  ): Promise<void> {
    const operation = randomUUID();
    const reserved = await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "operate", db);
      const c = (
        await db.query(
          "SELECT * FROM webdock_auth.mail_connection WHERE id=$1 AND customer_id=$2 FOR UPDATE",
          [actor.connectionID, actor.customerID],
        )
      ).rows[0];
      if (
        !c ||
        c.kind !== "platform" ||
        ["paused", "disconnected", "needs_review"].includes(c.state)
      )
        mailUnavailable();
      if (
        !options.rotate &&
        (
          await db.query(
            "SELECT 1 FROM webdock_auth.mail_connection_secret WHERE connection_id=$1 AND active",
            [c.id],
          )
        ).rowCount
      )
        return null;
      if (
        (
          await db.query(
            "SELECT 1 FROM webdock_auth.mail_provider_operation WHERE connection_id=$1 AND status IN ('pending','needs_review')",
            [c.id],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "needs_review",
          "Provider operation requires reconciliation",
        );
      const account = (
        await db.query(
          "SELECT provider_id,email,snapshot FROM webdock_auth.mail_tenant_account WHERE customer_id=$1 AND state='ready'",
          [actor.customerID],
        )
      ).rows[0];
      if (
        !account ||
        account.provider_id !== c.provider_account_id ||
        account.snapshot?.active !== true
      )
        mailUnavailable();
      const label = "Webdock internal mail " + operation;
      await db.query(
        "INSERT INTO webdock_auth.mail_provider_operation(customer_id,connection_id,label,status) VALUES($1,$2,$3,'pending')",
        [actor.customerID, c.id, label],
      );
      await db.query(
        "UPDATE webdock_auth.mail_connection SET state='pending' WHERE id=$1",
        [c.id],
      );
      const old = (
        await db.query(
          "SELECT version,encrypted_secret FROM webdock_auth.mail_connection_secret WHERE connection_id=$1 AND revoked_at IS NULL ORDER BY version",
          [c.id],
        )
      ).rows as { version: number; encrypted_secret: string }[];
      const version = Number(
        (
          await db.query(
            "SELECT coalesce(max(version),0)+1 AS version FROM webdock_auth.mail_connection_secret WHERE connection_id=$1",
            [c.id],
          )
        ).rows[0].version,
      );
      return { account, label, revision: String(c.revision), old, version };
    });
    if (!reserved) return;
    try {
      const master = await (
        deps.master ?? (() => dependencies.master(actor))
      )();
      const account = await master.getSubaccount(reserved.account.provider_id);
      if (
        account.id !== reserved.account.provider_id ||
        account.email?.toLowerCase() !== reserved.account.email.toLowerCase() ||
        !account.active
      )
        mailUnavailable();
      const token = await master.authorizeSubaccount(reserved.account.email);
      const child = deps.child ? deps.child(token) : dependencies.child(token);
      const secret = await child.createConsumerKey(token, reserved.label, {
        permissions: ["SEND_API"],
        ips: [],
      });
      const version = reserved.version;
      const encrypted = encryptMailSecret(
        secret,
        `mail-send:${actor.customerID}:${actor.connectionID}:${version}`,
      );
      await withMailTransaction(async (db) => {
        await assertMailAccess(actor, "operate", db);
        const c = (
          await db.query(
            "SELECT id FROM webdock_auth.mail_connection WHERE id=$1 AND revision=$2 AND state='pending' FOR UPDATE",
            [actor.connectionID, reserved.revision],
          )
        ).rows[0];
        const account = (
          await db.query(
            "SELECT provider_id FROM webdock_auth.mail_tenant_account WHERE customer_id=$1 AND state='ready'",
            [actor.customerID],
          )
        ).rows[0];
        if (!c || account?.provider_id !== reserved.account.provider_id)
          mailUnavailable();
        await db.query(
          "UPDATE webdock_auth.mail_connection_secret SET active=false WHERE connection_id=$1 AND active",
          [actor.connectionID],
        );
        await db.query(
          "INSERT INTO webdock_auth.mail_connection_secret(customer_id,connection_id,version,encrypted_secret,key_suffix,active) VALUES($1,$2,$3,$4,$5,true)",
          [
            actor.customerID,
            c.id,
            version,
            encrypted,
            secret.consumerKey.slice(-6),
          ],
        );
        await db.query(
          "UPDATE webdock_auth.mail_connection SET state='ready',updated_at=now() WHERE id=$1",
          [c.id],
        );
        await db.query(
          "UPDATE webdock_auth.mail_provider_operation SET status='succeeded',updated_at=now() WHERE label=$1",
          [reserved.label],
        );
      });
      for (const old of reserved.old) {
        const pair = decryptMailSecret<{ consumerKey: string }>(
          old.encrypted_secret,
          `mail-send:${actor.customerID}:${actor.connectionID}:${old.version}`,
        );
        if (!child.deleteConsumerKey)
          throw Error("Scoped key revocation unavailable");
        await child.deleteConsumerKey(pair.consumerKey);
        await database.query(
          "UPDATE webdock_auth.mail_connection_secret SET revoked_at=now() WHERE connection_id=$1 AND version=$2 AND NOT active",
          [actor.connectionID, old.version],
        );
      }
    } catch {
      // A timed out create may already have issued a key. Never issue another automatically.
      await withMailTransaction(async (db) => {
        await db.query(
          "UPDATE webdock_auth.mail_provider_operation SET status='needs_review',updated_at=now() WHERE label=$1 AND status IN ('pending','succeeded')",
          [reserved.label],
        );
        await db.query(
          "UPDATE webdock_auth.mail_connection SET state='needs_review',updated_at=now() WHERE id=$1 AND state='pending'",
          [actor.connectionID],
        );
      });
      throw new MailError(
        409,
        "needs_review",
        "Sending credential operation requires reconciliation",
      );
    }
  }
  async function resolveSendingCredential(actor: MailActor): Promise<{
    consumerKey: string;
    consumerSecret: string;
    region: "eu" | "global";
  }> {
    await assertMailAccess(actor, "send");
    const row = (
      await database.query(
        `SELECT c.region,c.kind,c.provider_account_id,s.version,s.encrypted_secret FROM webdock_auth.mail_connection c
 JOIN webdock_auth.mail_connection_secret s ON s.connection_id=c.id AND s.customer_id=c.customer_id AND s.active AND s.revoked_at IS NULL
 WHERE c.id=$1 AND c.customer_id=$2 AND c.state='ready'`,
        [actor.connectionID, actor.customerID],
      )
    ).rows[0];
    if (!row) mailUnavailable();
    if (row.kind === "platform") {
      if (
        !(
          await database.query(
            "SELECT 1 FROM webdock_auth.mail_tenant_account WHERE customer_id=$1 AND provider_id=$2 AND state='ready' AND snapshot->>'active'='true'",
            [actor.customerID, row.provider_account_id],
          )
        ).rowCount
      )
        mailUnavailable();
    }
    const pair = decryptMailSecret<{
      consumerKey: string;
      consumerSecret: string;
    }>(
      row.encrypted_secret,
      `mail-send:${actor.customerID}:${actor.connectionID}:${row.version}`,
    );
    if (row.kind === "byok") {
      const current = (
        await database.query(
          "SELECT encrypted_secret FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1 AND state='ready' AND pending_domain IS NULL",
          [actor.customerID],
        )
      ).rows[0];
      if (!current?.encrypted_secret) mailUnavailable();
      const source = decryptMailSecret<{
        consumerKey: string;
        consumerSecret: string;
      }>(current.encrypted_secret, `byok-mail:${actor.customerID}`);
      const hash = (value: unknown) =>
        createHash("sha256").update(JSON.stringify(value)).digest();
      if (
        !timingSafeEqual(
          hash([pair.consumerKey, pair.consumerSecret]),
          hash([source.consumerKey, source.consumerSecret]),
        )
      )
        throw new MailError(
          409,
          "provider_changed",
          "Verify the changed provider account before sending",
        );
    }
    if (
      !pair ||
      typeof pair.consumerKey !== "string" ||
      !pair.consumerKey ||
      typeof pair.consumerSecret !== "string" ||
      !pair.consumerSecret
    )
      mailUnavailable();
    return { ...pair, region: row.region };
  }
  /** Scoped management authorization for reconciliation; never exposed in a DTO. */
  async function mailManagementHeaders(
    actor: MailActor,
    options: { requireActive?: boolean } = {},
  ): Promise<Record<string, string>> {
    await assertMailAccess(actor, "read");
    const connection = (
      await database.query(
        "SELECT kind,provider_account_id FROM webdock_auth.mail_connection WHERE id=$1 AND customer_id=$2",
        [actor.connectionID, actor.customerID],
      )
    ).rows[0];
    if (!connection) mailUnavailable();
    if (connection.kind === "platform") {
      const source = (
        await database.query(
          "SELECT provider_id,email FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
          [actor.customerID],
        )
      ).rows[0];
      if (!source || source.provider_id !== connection.provider_account_id)
        mailUnavailable();
      const master = await dependencies.master(actor),
        confirmed = await master.getSubaccount(source.provider_id);
      if (
        confirmed.id !== source.provider_id ||
        confirmed.email?.toLowerCase() !== source.email.toLowerCase() ||
        (options.requireActive && !confirmed.active)
      )
        mailUnavailable();
      return { Authorization: await master.authorizeSubaccount(source.email) };
    }
    const stored = (
      await database.query(
        "SELECT version,encrypted_secret FROM webdock_auth.mail_connection_secret WHERE connection_id=$1 AND customer_id=$2 AND active AND revoked_at IS NULL",
        [actor.connectionID, actor.customerID],
      )
    ).rows[0];
    if (!stored) mailUnavailable();
    const pair = decryptMailSecret<{
      consumerKey: string;
      consumerSecret: string;
    }>(
      stored.encrypted_secret,
      `mail-send:${actor.customerID}:${actor.connectionID}:${stored.version}`,
    );
    return {
      consumerKey: pair.consumerKey,
      consumerSecret: pair.consumerSecret,
    };
  }

  async function rotateManagedSendingCredential(
    actor: MailActor,
    deps: Dependencies = {},
  ): Promise<void> {
    return ensureManagedSendingCredential(actor, deps, { rotate: true });
  }

  return {
    ensureManagedSendingCredential,
    resolveSendingCredential,
    mailManagementHeaders,
    rotateManagedSendingCredential,
  };
}
