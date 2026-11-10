import type { MailFetch } from "./provider-ports";
import { randomBytes, randomUUID } from "node:crypto";
import type { MailActor } from "@spitzli/mail-contracts";
import { parseID } from "@spitzli/mail-contracts/validation";
import { normalizeMailDomain } from "./tenant-mail";
import { lookupMailTxt, planMailRecords } from "./mail-dns";
import { mailUnavailable, MailError } from "./product-access";
import type {
  MailProductStore,
  MailDomainProvider,
  MailProviderAuth,
  MailCredentialMaster,
  SenderDomain,
} from "./provider-ports";
import type { createMailCredentials } from "./provider-credentials";
export function createMailDomains(
  dependencies: MailProductStore & {
    requireMail(intent?: "access" | "provision"): void;
    mailFetch: MailFetch;
    master(actor: MailActor, fetcher: MailFetch): Promise<MailCredentialMaster>;
    domainProvider(
      auth: MailProviderAuth,
      fetcher: MailFetch,
    ): MailDomainProvider;
    senderDomain(value: unknown): SenderDomain;
    resolveSendingCredential: ReturnType<
      typeof createMailCredentials
    >["resolveSendingCredential"];
  },
) {
  const {
    database,
    withMailTransaction,
    assertMailAccess,
    requireMail,
    mailFetch,
    senderDomain,
    resolveSendingCredential,
  } = dependencies;
  async function clientFor(actor: MailActor) {
    const c = (
      await database.query(
        "SELECT * FROM webdock_auth.mail_connection WHERE id=$1 AND customer_id=$2 AND deleted_at IS NULL",
        [actor.connectionID, actor.customerID],
      )
    ).rows[0];
    if (!c) mailUnavailable();
    if (c.kind === "byok")
      return dependencies.domainProvider(
        await resolveSendingCredential(actor),
        mailFetch,
      );
    const row = (
      await database.query(
        "SELECT provider_id,email FROM webdock_auth.mail_tenant_account WHERE customer_id=$1 AND state='ready'",
        [actor.customerID],
      )
    ).rows[0];
    if (!row || row.provider_id !== c.provider_account_id) mailUnavailable();
    const master = await dependencies.master(actor, mailFetch),
      confirmed = await master.getSubaccount(row.provider_id);
    if (
      confirmed.id !== row.provider_id ||
      confirmed.email?.toLowerCase() !== row.email.toLowerCase() ||
      !confirmed.active
    )
      mailUnavailable();
    return dependencies.domainProvider(
      { apiKey: await master.authorizeSubaccount(row.email) },
      mailFetch,
    );
  }
  async function assertVerifiedSender(
    actor: MailActor,
    domain: string,
    options: { simulation?: boolean } = {},
  ): Promise<{ domainID: string }> {
    await assertMailAccess(actor, "send");
    const name = normalizeMailDomain(domain);
    const row = (
      await database.query(
        `SELECT d.id,d.domain,b.provider_id,b.provider_snapshot,b.provider_checked_at FROM webdock_auth.mail_sender_domain d
 JOIN webdock_auth.mail_connection_domain b ON b.domain_id=d.id AND b.customer_id=d.customer_id
 JOIN webdock_auth.mail_connection c ON c.id=b.connection_id
 WHERE d.customer_id=$1 AND b.connection_id=$2 AND d.domain=$3 AND d.deleted_at IS NULL AND d.status='ownership_verified' AND d.provider_operation IS NULL AND (c.kind='byok' OR d.provider_id=b.provider_id)`,
        [actor.customerID, actor.connectionID, name],
      )
    ).rows[0];
    if (!row)
      throw new MailError(
        422,
        "sender_not_verified",
        "Sender ownership is not verified",
      );
    if (actor.principal.kind === "api_key") {
      const key = (
        await database.query(
          "SELECT domain_id FROM webdock_auth.mail_api_key WHERE id=$1 AND revoked_at IS NULL",
          [actor.principal.keyID],
        )
      ).rows[0];
      if (!key || (key.domain_id && key.domain_id !== row.id))
        mailUnavailable();
    }
    let observation;
    try {
      observation = senderDomain(row.provider_snapshot);
    } catch {
      observation = undefined;
    }
    if (
      !options.simulation &&
      (!row.provider_checked_at ||
        Date.now() - new Date(row.provider_checked_at).getTime() > 300_000)
    ) {
      const provider = await clientFor(actor);
      const matches = (await provider.listSenderDomains()).filter(
        (d) => d.id === row.provider_id && d.domain === name,
      );
      if (matches.length !== 1)
        throw new MailError(
          422,
          "sender_not_verified",
          "Provider verification unavailable",
        );
      observation = matches[0];
      await assertMailAccess(actor, "send");
      if (process.env.DATABASE_RLS_SECRET)
        await database.query(
          "SELECT webdock_rls.mail_refresh_domain($1,$2,$3,$4)",
          [
            actor.customerID,
            actor.connectionID,
            row.id,
            JSON.stringify(observation),
          ],
        );
      else
        await database.query(
          "UPDATE webdock_auth.mail_connection_domain SET provider_snapshot=$3,provider_checked_at=now() WHERE connection_id=$1 AND domain_id=$2",
          [actor.connectionID, row.id, JSON.stringify(observation)],
        );
    }
    if (
      !observation ||
      observation.id !== row.provider_id ||
      observation.domain !== name ||
      !observation.spf_verified ||
      !observation.dkim_verified
    )
      throw new MailError(
        422,
        "sender_not_verified",
        "Sender DNS is not verified",
      );
    // Recheck the local proof after any network wait.
    if (
      !(
        await database.query(
          "SELECT 1 FROM webdock_auth.mail_sender_domain WHERE id=$1 AND customer_id=$2 AND deleted_at IS NULL AND status='ownership_verified'",
          [row.id, actor.customerID],
        )
      ).rowCount
    )
      mailUnavailable();
    return { domainID: row.id };
  }
  async function claimMailDomain(
    actor: MailActor,
    name: string,
  ): Promise<{ id: string }> {
    requireMail("provision");
    const domain = normalizeMailDomain(name);
    return withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      let row = (
        await db.query(
          "SELECT id FROM webdock_auth.mail_sender_domain WHERE customer_id=$1 AND domain=$2 AND deleted_at IS NULL FOR UPDATE",
          [actor.customerID, domain],
        )
      ).rows[0];
      if (!row)
        row = (
          await db.query(
            "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token) VALUES($1,$2,$3) RETURNING id",
            [actor.customerID, domain, randomBytes(24).toString("hex")],
          )
        ).rows[0];
      await db.query(
        "INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
        [actor.customerID, actor.connectionID, row.id],
      );
      return { id: row.id };
    });
  }
  async function removeMailDomain(actor: MailActor, id: string): Promise<void> {
    parseID(id);
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      if (
        !(
          await db.query(
            "SELECT d.id FROM webdock_auth.mail_sender_domain d JOIN webdock_auth.mail_connection_domain b ON b.domain_id=d.id AND b.customer_id=d.customer_id WHERE b.customer_id=$1 AND b.connection_id=$2 AND b.domain_id=$3 FOR UPDATE OF d",
            [actor.customerID, actor.connectionID, id],
          )
        ).rowCount
      )
        mailUnavailable();
      if (
        (
          await db.query(
            "SELECT 1 FROM webdock_auth.mail_connection_domain WHERE domain_id=$1 AND connection_id<>$2 LIMIT 1",
            [id, actor.connectionID],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "domain_in_use",
          "Domain is shared with another connection",
        );
      if (
        (await db.query("SELECT to_regclass('webdock_mail.domain') AS present"))
          .rows[0].present &&
        (
          await db.query(
            "SELECT 1 FROM webdock_mail.domain WHERE sender_domain_id=$1 LIMIT 1",
            [id],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "domain_in_use",
          "Domain is used by native Mail",
        );
      await db.query(
        "UPDATE webdock_auth.mail_sender_domain SET deleted_at=coalesce(deleted_at,now()) WHERE id=$1 AND customer_id=$2",
        [id, actor.customerID],
      );
    });
  }
  async function listMailDomains(actor: MailActor) {
    await assertMailAccess(actor, "read");
    return (
      await database.query(
        `SELECT d.id,d.domain,d.status,d.token,d.deleted_at,b.provider_snapshot,b.provider_checked_at FROM webdock_auth.mail_sender_domain d
 JOIN webdock_auth.mail_connection_domain b ON b.domain_id=d.id AND b.customer_id=d.customer_id
 JOIN webdock_auth.mail_connection c ON c.id=b.connection_id
 WHERE d.customer_id=$1 AND b.connection_id=$2 AND d.deleted_at IS NULL ORDER BY d.domain LIMIT 100`,
        [actor.customerID, actor.connectionID],
      )
    ).rows;
  }
  async function verifyMailDomain(
    actor: MailActor,
    id: string,
    deps: {
      lookup?: typeof lookupMailTxt;
      provider?: () => Promise<
        Pick<MailDomainProvider, "registerSenderDomain" | "listSenderDomains">
      >;
    } = {},
  ) {
    parseID(id);
    await assertMailAccess(actor, "manage");
    const row = (
      await database.query(
        "SELECT d.* FROM webdock_auth.mail_sender_domain d JOIN webdock_auth.mail_connection_domain b ON b.domain_id=d.id AND b.customer_id=d.customer_id WHERE d.id=$1 AND d.customer_id=$2 AND b.connection_id=$3 AND d.deleted_at IS NULL",
        [id, actor.customerID, actor.connectionID],
      )
    ).rows[0];
    if (!row) mailUnavailable();
    const txt = await (deps.lookup ?? lookupMailTxt)(row.domain);
    const records = planMailRecords(row.domain, row.token, txt);
    const ownership = records.find(
      (r) => r.name === "_webdock-mail." + row.domain,
    );
    if (ownership?.status !== "verified")
      throw new MailError(
        422,
        "ownership_not_verified",
        "Publish the ownership TXT record first",
      );
    const operation = randomUUID();
    await withMailTransaction(async (db) => {
      await assertMailAccess(actor, "manage", db);
      if (
        !(
          await db.query(
            "UPDATE webdock_auth.mail_sender_domain SET status='ownership_verified',checked_at=now(),provider_operation=$3,provider_operation_started_at=now() WHERE id=$1 AND customer_id=$2 AND deleted_at IS NULL AND (provider_operation IS NULL OR provider_operation_started_at<now()-interval '5 minutes') RETURNING id",
            [id, actor.customerID, operation],
          )
        ).rowCount
      )
        throw new MailError(
          409,
          "domain_busy",
          "Domain operation is already in progress",
        );
    });
    try {
      const provider = await (deps.provider
        ? deps.provider()
        : clientFor(actor));
      await assertMailAccess(actor, "manage");
      await provider.registerSenderDomain(row.domain);
      const matching = (await provider.listSenderDomains()).filter(
        (d) => d.domain === row.domain,
      );
      if (matching.length !== 1)
        throw new MailError(
          409,
          "verification_pending",
          "Provider verification is pending",
        );
      await withMailTransaction(async (db) => {
        await assertMailAccess(actor, "manage", db);
        if (
          !(
            await db.query(
              `UPDATE webdock_auth.mail_sender_domain SET provider_id=CASE WHEN (SELECT kind FROM webdock_auth.mail_connection WHERE id=$6)='platform' THEN $4 ELSE provider_id END,provider_snapshot=CASE WHEN (SELECT kind FROM webdock_auth.mail_connection WHERE id=$6)='platform' THEN $5::jsonb ELSE provider_snapshot END,provider_checked_at=now() WHERE id=$1 AND customer_id=$2 AND provider_operation=$3 AND deleted_at IS NULL RETURNING id`,
              [
                id,
                actor.customerID,
                operation,
                matching[0].id,
                JSON.stringify(matching[0]),
                actor.connectionID,
              ],
            )
          ).rowCount
        )
          mailUnavailable();
        await db.query(
          "UPDATE webdock_auth.mail_connection_domain SET provider_id=$3,provider_snapshot=$4,provider_checked_at=now() WHERE connection_id=$1 AND domain_id=$2",
          [actor.connectionID, id, matching[0].id, JSON.stringify(matching[0])],
        );
      });
      return {
        id,
        domain: row.domain,
        ownershipVerified: true,
        provider: matching[0],
        records,
      };
    } catch (error) {
      await database.query(
        "UPDATE webdock_auth.mail_connection_domain SET provider_snapshot='{}',provider_checked_at=NULL WHERE connection_id=$1 AND domain_id=$2",
        [actor.connectionID, id],
      );
      throw error;
    } finally {
      await database.query(
        "UPDATE webdock_auth.mail_sender_domain SET provider_operation=NULL,provider_operation_started_at=NULL WHERE id=$1 AND customer_id=$2 AND provider_operation=$3",
        [id, actor.customerID, operation],
      );
    }
  }
  async function getMailDomain(actor: MailActor, id: string) {
    parseID(id);
    await assertMailAccess(actor, "read");
    const row = (
      await database.query(
        `SELECT d.id,d.domain,d.status,d.token,b.provider_snapshot,b.provider_checked_at FROM webdock_auth.mail_sender_domain d
 JOIN webdock_auth.mail_connection_domain b ON b.domain_id=d.id AND b.customer_id=d.customer_id WHERE d.id=$1 AND d.customer_id=$2 AND b.connection_id=$3 AND d.deleted_at IS NULL`,
        [id, actor.customerID, actor.connectionID],
      )
    ).rows[0];
    if (!row) mailUnavailable();
    let txt;
    let dnsIssue: string | undefined;
    try {
      txt = await lookupMailTxt(row.domain);
    } catch {
      txt = { root: [], dkim: [], dmarc: [], ownership: [] };
      dnsIssue = "dns_lookup_unavailable";
    }
    return {
      ...row,
      records: planMailRecords(row.domain, row.token, txt),
      ...(dnsIssue ? { dnsIssue } : {}),
    };
  }

  return {
    assertVerifiedSender,
    claimMailDomain,
    removeMailDomain,
    listMailDomains,
    verifyMailDomain,
    getMailDomain,
  };
}
