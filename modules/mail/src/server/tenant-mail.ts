import type { MailFetch } from "./provider-ports";
import { randomBytes } from "node:crypto";
import {
  TenantMailError,
  normalizeMailDomain,
} from "@spitzli/mail-contracts/domain";
export {
  TenantMailError,
  normalizeMailDomain,
} from "@spitzli/mail-contracts/domain";
import type { DatabaseReader, SessionReference } from "@spitzli/core/identity";
import type {
  MailAccountProvider,
  MailProviderAccount,
  MailSecrets,
} from "./provider-ports";
export interface TenantMailDependencies {
  database: DatabaseReader;
  withAuthenticatedDatabase<T>(
    headers: Headers,
    run: () => Promise<T>,
  ): Promise<T>;
  getTenant(
    headers: Headers,
    customerID: string,
  ): Promise<{
    tenant: { id: string; name: string; status: string };
    operator: boolean;
    canManage: boolean;
  }>;
  resolveSessionReference(headers: Headers): Promise<SessionReference | null>;
  getPlatformSettings(): Promise<{
    mailEnabled: boolean;
    mailRegion: "eu" | "global";
    mailDefaultLimit: number;
    mailSendingIP: string;
  }>;
  getMailConnectionStatus(): Promise<{ configured: boolean }>;
  encryptMailSecret: MailSecrets["encryptMailSecret"];
  requireMail(intent: "provision" | "access"): void;
  provider(customerID: string): Promise<MailAccountProvider>;
  /** True only for a definitive rejection before account creation; timeout/unknown must retain the reservation. */
  definitelyRejectedCreation(error: unknown): boolean;
}
export const mailSchemaSQL = `
CREATE TABLE IF NOT EXISTS webdock_auth.mail_tenant_account (
 customer_id text PRIMARY KEY REFERENCES webdock_auth.tenant_customer(customer_id),
 provider_id text UNIQUE, email text NOT NULL UNIQUE, encrypted_password text,
 state text NOT NULL CHECK(state IN ('provisioning','ready','needs_review')),
 snapshot jsonb NOT NULL DEFAULT '{}', checked_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_auth.mail_sender_domain (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 customer_id text NOT NULL REFERENCES webdock_auth.tenant_customer(customer_id),
 domain text NOT NULL, token text NOT NULL,
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','ownership_verified')),
 checked_at timestamptz, deleted_at timestamptz, created_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE webdock_auth.mail_sender_domain ADD COLUMN IF NOT EXISTS deleted_at timestamptz;
CREATE UNIQUE INDEX IF NOT EXISTS mail_sender_domain_live_claim ON webdock_auth.mail_sender_domain(customer_id,domain) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS mail_verified_domain ON webdock_auth.mail_sender_domain(domain) WHERE status='ownership_verified' AND deleted_at IS NULL;`;
export type MailView = {
  tenant: { id: string; name: string; status: string };
  operator: boolean;
  canManage: boolean;
  enabled: boolean;
  configured: boolean;
  region: "eu" | "global";
  defaultLimit: number;
  account: null | {
    state: string;
    providerID: string | null;
    email: string;
    active?: boolean;
    limit?: number;
    sent?: number;
    interval?: string;
    checkedAt?: string;
    issue?: string;
  };
  domains: {
    id: string;
    domain: string;
    status: "pending" | "ownership_verified";
    token: string;
    checkedAt?: string;
  }[];
};
export type TenantMailOverrides = {
  provider?: () => Promise<MailAccountProvider>;
  dnsFetch?: MailFetch;
};
type Dependencies = TenantMailOverrides;
const failure = (message: string): never => {
  throw new TenantMailError(message);
};
const timestamp = (value: Date | string | null) =>
  value ? new Date(value).toISOString() : undefined;
function snapshot(account: MailProviderAccount) {
  return {
    active: account.active,
    ...(account.limit === undefined ? {} : { limit: account.limit }),
    ...(account.sent === undefined ? {} : { sent: account.sent }),
    ...(account.interval === undefined ? {} : { interval: account.interval }),
  };
}
function email(value: string) {
  const result = (value || "").trim().toLowerCase();
  if (
    result.length > 254 ||
    !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(result) ||
    /[\x00-\x1f\x7f]/.test(result)
  )
    failure("Enter a valid account email.");
  return result;
}
export function createTenantMail(dependencies: TenantMailDependencies) {
  const {
    database,
    withAuthenticatedDatabase,
    getTenant,
    getPlatformSettings,
    getMailConnectionStatus,
    resolveSessionReference,
    encryptMailSecret,
    requireMail,
  } = dependencies;
  async function getTenantMail(
    headers: Headers,
    customerID: string,
  ): Promise<MailView> {
    return withAuthenticatedDatabase(headers, async () => {
      const access = await getTenant(headers, customerID);
      const [settings, connection, accounts, domains] = await Promise.all([
        getPlatformSettings(),
        getMailConnectionStatus(),
        database.query(
          "SELECT state,provider_id,email,snapshot,checked_at,created_at FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
          [customerID],
        ),
        database.query(
          "SELECT id,domain,token,status,checked_at FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND customer_id=$1 ORDER BY domain",
          [customerID],
        ),
      ]);
      const row = accounts.rows[0];
      const stale =
        row?.state === "provisioning" &&
        new Date(row.created_at).getTime() < Date.now() - 600_000;
      return {
        tenant: {
          id: access.tenant.id,
          name: access.tenant.name,
          status: access.tenant.status,
        },
        operator: access.operator,
        canManage: access.canManage,
        enabled: settings.mailEnabled,
        configured: connection.configured,
        region: settings.mailRegion,
        defaultLimit: settings.mailDefaultLimit,
        account: row
          ? {
              state: stale ? "needs_review" : row.state,
              providerID: row.provider_id,
              email: row.email,
              ...snapshot(row.snapshot),
              checkedAt: timestamp(row.checked_at),
              ...(row.state === "needs_review" || stale
                ? {
                    issue: stale
                      ? "Provisioning has not completed. An operator must reconcile the existing provider account; do not create it again."
                      : "An operator must reconcile the provider account before provisioning again.",
                  }
                : {}),
            }
          : null,
        domains: domains.rows.map((row) => ({
          id: row.id,
          domain: row.domain,
          token: row.token,
          status: row.status,
          checkedAt: timestamp(row.checked_at),
        })),
      };
    });
  }
  async function verifyDNS(
    domain: string,
    token: string,
    fetchImpl: MailFetch,
  ) {
    const url = new URL("https://cloudflare-dns.com/dns-query");
    url.searchParams.set("name", `_webdock-mail.${domain}`);
    url.searchParams.set("type", "TXT");
    const controller = new AbortController();
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        controller.abort();
        reject(new TenantMailError("Public DNS lookup timed out."));
      }, 8000);
    });
    try {
      return await Promise.race([
        timeout,
        (async () => {
          const response = await fetchImpl(url, {
            headers: { Accept: "application/dns-json" },
            redirect: "error",
            cache: "no-store",
            signal: controller.signal,
          });
          if (
            !response.ok ||
            Number(response.headers.get("content-length")) > 65536
          )
            failure("Public DNS lookup failed.");
          reader = response.body?.getReader();
          if (!reader) failure("Public DNS lookup failed.");
          let body = "",
            size = 0;
          const decoder = new TextDecoder();
          for (;;) {
            const part = await reader!.read();
            if (part.done) break;
            size += part.value.byteLength;
            if (size > 65536) failure("Public DNS response too large.");
            body += decoder.decode(part.value, { stream: true });
          }
          const data = JSON.parse(body + decoder.decode());
          if (data.Status !== 0 || (data.AD === false && data.CD === true))
            return false;
          return (
            Array.isArray(data.Answer) &&
            data.Answer.some(
              (answer: { type?: number; name?: string; data?: string }) =>
                answer.type === 16 &&
                answer.name?.toLowerCase().replace(/\.$/, "") ===
                  `_webdock-mail.${domain}` &&
                typeof answer.data === "string" &&
                answer.data.replace(/^"|"$/g, "").replace(/"\s+"/g, "") ===
                  `webdock=${token}`,
            )
          );
        })(),
      ]);
    } catch (error) {
      if (error instanceof TenantMailError) throw error;
      failure("Public DNS lookup failed.");
    } finally {
      clearTimeout(timer);
      controller.abort();
      void reader?.cancel().catch(() => {});
    }
  }
  async function manageTenantMail(
    headers: Headers,
    customerID: string,
    input: Record<string, string>,
    overrides: Dependencies = {},
  ): Promise<{ message: string }> {
    return withAuthenticatedDatabase(headers, async () => {
      const access = await getTenant(headers, customerID);
      if (!access.canManage || access.tenant.status !== "active")
        failure("Active tenant administrator access is required.");
      const settings = await getPlatformSettings();
      if (!settings.mailEnabled)
        failure("Mail is disabled by the platform operator.");
      const action = input.action;
      if (
        ![
          "provision",
          "link",
          "refresh",
          "status",
          "quota",
          "add-domain",
          "verify-domain",
          "remove-domain",
        ].includes(action)
      )
        failure("Unknown mail action.");
      if (
        ["provision", "link", "status", "quota"].includes(action) &&
        !access.operator
      )
        failure("Platform operator access is required.");
      const session = await resolveSessionReference(headers);
      if (!session) failure("Sign in first.");
      const event = (
        await database.query(
          "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,$3,'started') RETURNING id",
          [session!.subject, `mail-${action}`, customerID],
        )
      ).rows[0].id;
      try {
        requireMail(
          ["provision", "link", "add-domain"].includes(action)
            ? "provision"
            : "access",
        );
        if (action === "add-domain") {
          await database.query(
            "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token) VALUES($1,$2,$3)",
            [
              customerID,
              normalizeMailDomain(input.domain),
              randomBytes(24).toString("hex"),
            ],
          );
        } else if (["verify-domain", "remove-domain"].includes(action)) {
          const domain = (
            await database.query(
              "SELECT id,domain,token FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND id=$1 AND customer_id=$2",
              [input.domainID, customerID],
            )
          ).rows[0];
          if (!domain) failure("Domain not found.");
          if (action === "remove-domain") {
            if (input.confirm !== "yes")
              failure("Confirm removal of this local domain entry.");
            const removed = await database.query(
              "DELETE FROM webdock_auth.mail_sender_domain WHERE deleted_at IS NULL AND id=$1 AND customer_id=$2 AND provider_id IS NULL AND provider_operation IS NULL AND provider_checked_at IS NULL",
              [domain.id, customerID],
            );
            if (!removed.rowCount)
              failure(
                "Remove or reconcile the sending registration before removing its local domain entry.",
              );
          } else {
            const verified = await verifyDNS(
              domain.domain,
              domain.token,
              overrides.dnsFetch || fetch,
            );
            // Recheck current authorization after network I/O; unique index arbitrates concurrent claims.
            if (!(await getTenant(headers, customerID)).canManage)
              failure("Tenant administrator access is required.");
            await database.query(
              "UPDATE webdock_auth.mail_sender_domain SET status=$3,checked_at=now() WHERE deleted_at IS NULL AND id=$1 AND customer_id=$2",
              [
                domain.id,
                customerID,
                verified ? "ownership_verified" : "pending",
              ],
            );
            if (!verified)
              failure("The public DNS ownership token does not match yet.");
          }
        } else {
          const provider = await (
            overrides.provider || (() => dependencies.provider(customerID))
          )();
          const stored = (
            await database.query(
              "SELECT * FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
              [customerID],
            )
          ).rows[0];
          let result: MailProviderAccount;
          if (action === "provision") {
            if (stored)
              failure(
                "An account is already reserved. Refresh it or reconcile it using Link; do not provision again.",
              );
            const address = email(input.email),
              firstName = (input.firstName || "").trim(),
              lastName = (input.lastName || "").trim();
            if (
              !settings.mailSendingIP ||
              input.policyAgree !== "yes" ||
              [firstName, lastName].some(
                (v) => !v || v.length > 50 || /[\x00-\x1f\x7f]/.test(v),
              )
            )
              failure(
                "Provide both names, accept the provider policy, and configure the assigned sending IP.",
              );
            const password = `W9!${randomBytes(32).toString("base64url")}`;
            const sealed = encryptMailSecret(password, customerID);
            await database.query(
              "INSERT INTO webdock_auth.mail_tenant_account(customer_id,email,encrypted_password,state) VALUES($1,$2,$3,'provisioning')",
              [customerID, address, sealed],
            );
            let created = false;
            try {
              result = await provider.createSubaccount({
                email: address,
                first_name: firstName,
                last_name: lastName,
                ip: settings.mailSendingIP,
                policy_agree: true,
                password,
                confirm_password: password,
              });
              created = true;
              const mapped = await database.query(
                "UPDATE webdock_auth.mail_tenant_account SET provider_id=$2,snapshot=$3,updated_at=now() WHERE customer_id=$1 AND state='provisioning' AND provider_id IS NULL",
                [customerID, result.id, JSON.stringify(snapshot(result))],
              );
              if (!mapped.rowCount)
                failure("This reservation has already been reconciled.");
              const createdID = result.id;
              result = await provider.setSubaccountLimit(
                createdID,
                settings.mailDefaultLimit,
              );
              if (result.id !== createdID)
                failure("Provider returned an unexpected account.");
            } catch (error) {
              if (!created && dependencies.definitelyRejectedCreation(error)) {
                await database.query(
                  "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1 AND state='provisioning' AND provider_id IS NULL AND encrypted_password=$2",
                  [customerID, sealed],
                );
                failure(
                  "The provider rejected account creation. Correct the provider configuration or account details, then submit again.",
                );
              }
              await database.query(
                "UPDATE webdock_auth.mail_tenant_account SET state='needs_review',updated_at=now() WHERE customer_id=$1 AND state='provisioning'",
                [customerID],
              );
              failure(
                "Provisioning needs operator review. The provider may have created the account; reconcile it before continuing.",
              );
            }
          } else if (action === "link") {
            const address = email(input.email),
              id = input.providerID;
            const stale =
              stored?.state === "provisioning" &&
              new Date(stored.created_at).getTime() < Date.now() - 600_000;
            if (
              input.confirm !== "yes" ||
              !/^[1-9]\d{0,15}$/.test(id || "") ||
              (stored &&
                ((stored.state !== "needs_review" && !stale) ||
                  stored.email !== address ||
                  (stored.provider_id && stored.provider_id !== id)))
            )
              failure(
                "Confirm an existing account matching the reserved email and provider ID.",
              );
            const owned = await provider.listSubaccounts({
              filterByEmail: address,
              limit: 100,
            });
            if (
              !owned.results.some(
                (account) =>
                  account.id === id && account.email?.toLowerCase() === address,
              )
            )
              failure(
                "This exact account was not found under the configured provider master.",
              );
            result = await provider.getSubaccount(id);
            if (result.id !== id || result.email?.toLowerCase() !== address)
              failure("Provider account email does not match.");
            const linked = await database.query(
              "INSERT INTO webdock_auth.mail_tenant_account(customer_id,email,provider_id,state) VALUES($1,$2,$3,'needs_review') ON CONFLICT(customer_id) DO UPDATE SET provider_id=$3,state='needs_review' WHERE (mail_tenant_account.state='needs_review' OR (mail_tenant_account.state='provisioning' AND mail_tenant_account.created_at < now() - interval '10 minutes')) AND mail_tenant_account.email=$2 AND (mail_tenant_account.provider_id IS NULL OR mail_tenant_account.provider_id=$3)",
              [customerID, address, id],
            );
            if (!linked.rowCount)
              failure(
                "This reservation has already changed. Refresh before reconciling it again.",
              );
            result = await provider.setSubaccountLimit(
              id,
              settings.mailDefaultLimit,
            );
            if (result.id !== id)
              failure("Provider returned an unexpected account.");
          } else {
            if (!stored?.provider_id)
              failure(
                "No mapped provider account exists. An operator must provision or link it first.",
              );
            if (stored.state !== "ready")
              failure(
                "An operator must reconcile this account before reading or changing provider settings.",
              );
            if (action === "refresh") {
              const account = await provider.getSubaccount(stored.provider_id),
                plan = await provider.getSubaccountPlan(stored.provider_id);
              if (
                account.id !== stored.provider_id ||
                plan.id !== stored.provider_id
              )
                failure("Provider returned an unexpected account.");
              result = { ...account, ...plan, active: account.active };
            } else if (action === "status") {
              if (!["yes", "no"].includes(input.active))
                failure("Choose active or paused.");
              result = await provider.setSubaccountActive(
                stored.provider_id,
                input.active === "yes",
              );
            } else {
              if (
                !/^\d{1,10}$/.test(input.limit || "") ||
                Number(input.limit) > 1_000_000_000
              )
                failure("Choose a mail limit from 0 to 1,000,000,000.");
              result = await provider.setSubaccountLimit(
                stored.provider_id,
                Number(input.limit),
              );
            }
            if (result.id !== stored.provider_id)
              failure("Provider returned an unexpected account.");
          }
          await database.query(
            "UPDATE webdock_auth.mail_tenant_account SET state='ready',snapshot=$2,checked_at=now(),updated_at=now() WHERE customer_id=$1 AND provider_id=$3",
            [customerID, JSON.stringify(snapshot(result!)), result!.id],
          );
        }
        await database.query(
          "UPDATE webdock_auth.access_event SET outcome='succeeded' WHERE id=$1",
          [event],
        );
        return {
          message:
            action === "verify-domain"
              ? "Domain ownership verified. You can now register this domain for sending."
              : action === "add-domain"
                ? "Domain added. Publish the DNS records and check ownership to continue."
                : "Mail change saved.",
        };
      } catch (error) {
        await database.query(
          "UPDATE webdock_auth.access_event SET outcome='failed' WHERE id=$1",
          [event],
        );
        if (error instanceof TenantMailError) throw error;
        if ((error as { code?: string }).code === "23505")
          failure(
            "This account or domain is already registered or verified elsewhere.",
          );
        return failure(
          "Mail operation failed. Retry reads safely; ask an operator to review uncertain provider changes.",
        );
      }
    });
  }

  return { getTenantMail, manageTenantMail };
}
