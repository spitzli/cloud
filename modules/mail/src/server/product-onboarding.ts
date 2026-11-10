import { createHash } from "node:crypto";
import type { DatabaseReader, SessionReference } from "@spitzli/core/identity";
import { MailError } from "./product-access";
import type { createMailAuthorization } from "./product-access";
import type { createMailCredentials } from "./provider-credentials";
import type { TenantMailDependencies } from "./tenant-mail";
import type {
  MailProductStore,
  MailSecrets,
  MailAccountVerification,
  SenderDomain,
} from "./provider-ports";
export interface MailOnboardingDependencies
  extends
    Pick<MailProductStore, "database" | "withMailTransaction">,
    Pick<
      TenantMailDependencies,
      | "getTenant"
      | "resolveSessionReference"
      | "withAuthenticatedDatabase"
      | "requireMail"
    >,
    Pick<MailSecrets, "encryptMailSecret"> {
  currentTenantPreview(reference: {
    userID: string;
    sessionID: string;
  }): Promise<unknown>;
  byokPolicy(
    db: DatabaseReader,
    customerID: string,
  ): Promise<{ turbosmtp: boolean }>;
  assertMailManager: ReturnType<
    typeof createMailAuthorization
  >["assertMailManager"];
  verifyTurboSendingAccount: MailAccountVerification;
  listProviderDomains(pair: {
    consumerKey: string;
    consumerSecret: string;
  }): Promise<SenderDomain[]>;
  ensureManagedSendingCredential: ReturnType<
    typeof createMailCredentials
  >["ensureManagedSendingCredential"];
}
export function createMailOnboarding(dependencies: MailOnboardingDependencies) {
  const {
    database,
    withMailTransaction,
    getTenant,
    resolveSessionReference,
    withAuthenticatedDatabase,
    requireMail,
    encryptMailSecret,
    currentTenantPreview,
    byokPolicy,
    assertMailManager,
    verifyTurboSendingAccount,
    ensureManagedSendingCredential,
  } = dependencies;
  async function tenantIdentity(
    headers: Headers,
    customerID: string,
    manage = false,
  ) {
    const session = await resolveSessionReference(headers);
    if (!session)
      throw new MailError(401, "session_required", "Sign in to Spitzli Cloud");
    if (
      await currentTenantPreview({
        userID: session.subject,
        sessionID: session.sessionID,
      })
    )
      throw new MailError(
        403,
        "preview_read_only",
        "Mail gateway is unavailable in customer preview",
      );
    const tenant = await getTenant(headers, customerID);
    if (tenant.tenant.status !== "active" || (manage && !tenant.canManage))
      throw new MailError(404, "not_found", "Tenant unavailable");
    return { tenant, session };
  }
  async function listTenantMailConnections(
    headers: Headers,
    customerID: string,
  ) {
    return withAuthenticatedDatabase(headers, async () => {
      await tenantIdentity(headers, customerID);
      return (
        await database.query(
          "SELECT id,kind,region,state,revision,created_at FROM webdock_auth.mail_connection WHERE customer_id=$1 AND deleted_at IS NULL ORDER BY created_at,id",
          [customerID],
        )
      ).rows;
    });
  }
  async function connectByokMail(
    headers: Headers,
    customerID: string,
    input: {
      consumerKey: string;
      consumerSecret: string;
      expectedRevision: number;
    },
    deps: {
      verify?: MailAccountVerification;
      domains?: (pair: {
        consumerKey: string;
        consumerSecret: string;
      }) => Promise<SenderDomain[]>;
    } = {},
  ): Promise<{ id: string; revision: string }> {
    return withAuthenticatedDatabase(headers, async () => {
      requireMail("provision");
      const identity = await tenantIdentity(headers, customerID, true);
      if (
        !Number.isSafeInteger(input.expectedRevision) ||
        input.expectedRevision < 0
      )
        throw new MailError(
          422,
          "invalid_revision",
          "Current provider revision is required",
        );
      if (!(await byokPolicy(database, customerID)).turbosmtp)
        throw new MailError(
          403,
          "byok_unavailable",
          "Own Mail provider is not enabled",
        );
      const credentials = {
        consumerKey: input.consumerKey,
        consumerSecret: input.consumerSecret,
      };
      const proof = await (deps.verify ?? verifyTurboSendingAccount)({
        ...credentials,
        region: "eu",
      });
      const domains = await (deps.domains
        ? deps.domains(credentials)
        : dependencies.listProviderDomains(credentials));
      try {
        return await withMailTransaction(async (db) => {
          await assertCurrentManager(
            db,
            identity.session.subject,
            identity.session.sessionID,
            customerID,
          );
          await db.query(
            "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR UPDATE",
            [customerID],
          );
          if (!(await byokPolicy(db, customerID)).turbosmtp)
            throw new MailError(
              403,
              "byok_unavailable",
              "Own Mail provider is not enabled",
            );
          const flags = (
            await db.query(
              "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
            )
          ).rows[0]?.settings;
          if (flags?.mailGatewayEnabled !== true)
            throw new MailError(
              503,
              "gateway_disabled",
              "Mail gateway is disabled",
            );
          const source = (
            await db.query(
              "SELECT revision,state,pending_domain FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1 FOR UPDATE",
              [customerID],
            )
          ).rows[0];
          if (source && (source.state !== "ready" || source.pending_domain))
            throw new MailError(
              409,
              "provider_busy",
              "Provider operation is in progress",
            );
          if ((source?.revision ?? 0) !== input.expectedRevision)
            throw new MailError(
              409,
              "provider_changed",
              "Provider connection changed; refresh first",
            );
          const existing = (
            await db.query(
              "SELECT id,customer_id,kind,revision FROM webdock_auth.mail_connection WHERE provider_account_id=$1 FOR UPDATE",
              [proof.accountID],
            )
          ).rows[0];
          if (
            existing &&
            (existing.customer_id !== customerID || existing.kind !== "byok")
          )
            throw new MailError(
              409,
              "account_already_bound",
              "Provider account already has a binding",
            );
          await db.query(
            "UPDATE webdock_auth.mail_connection SET state='paused',deleted_at=now(),revision=revision+1 WHERE customer_id=$1 AND kind='byok' AND deleted_at IS NULL AND provider_account_id<>$2",
            [customerID, proof.accountID],
          );
          const connection = existing
            ? (
                await db.query(
                  "UPDATE webdock_auth.mail_connection SET state='ready',deleted_at=NULL,revision=revision+1,updated_at=now() WHERE id=$1 RETURNING id,revision",
                  [existing.id],
                )
              ).rows[0]
            : (
                await db.query(
                  "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id,state,region) VALUES($1,'byok',$2,'ready','eu') RETURNING id,revision",
                  [customerID, proof.accountID],
                )
              ).rows[0];
          const version = Number(
            (
              await db.query(
                "SELECT coalesce(max(version),0)+1 AS next FROM webdock_auth.mail_connection_secret WHERE connection_id=$1",
                [connection.id],
              )
            ).rows[0].next,
          );
          await db.query(
            "UPDATE webdock_auth.mail_connection_secret SET active=false,revoked_at=now() WHERE connection_id=$1 AND active",
            [connection.id],
          );
          await db.query(
            "INSERT INTO webdock_auth.mail_connection_secret(customer_id,connection_id,version,encrypted_secret,key_suffix,active) VALUES($1,$2,$3,$4,$5,true)",
            [
              customerID,
              connection.id,
              version,
              encryptMailSecret(
                credentials,
                `mail-send:${customerID}:${connection.id}:${version}`,
              ),
              input.consumerKey.slice(-6),
            ],
          );
          const fingerprint = createHash("sha256")
            .update("webdock:byok:mail:" + input.consumerKey)
            .digest("hex");
          await db.query(
            "INSERT INTO webdock_auth.hosting_turbosmtp(customer_id,label,encrypted_secret,credential_hash,key_suffix,snapshot,checked_at) VALUES($1,'Webdock Mail BYOK',$2,$3,$4,$5,now()) ON CONFLICT(customer_id) DO UPDATE SET encrypted_secret=$2,credential_hash=$3,key_suffix=$4,snapshot=$5,checked_at=now(),state='ready',pending_domain=NULL,operation_token=NULL,revision=hosting_turbosmtp.revision+1,updated_at=now()",
            [
              customerID,
              encryptMailSecret(credentials, `byok-mail:${customerID}`),
              fingerprint,
              input.consumerKey.slice(-6),
              JSON.stringify(domains),
            ],
          );
          await db.query(
            "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'mail-byok-bound',$2,'succeeded')",
            [identity.session.subject, customerID],
          );
          return { id: connection.id, revision: String(connection.revision) };
        });
      } catch (error) {
        if ((error as { code?: string }).code === "23505")
          throw new MailError(
            409,
            "account_already_bound",
            "Provider account already has a binding",
          );
        throw error;
      }
    });
  }

  async function assertCurrentManager(
    db: DatabaseReader,
    userID: string,
    sessionID: string,
    customerID: string,
  ) {
    return assertMailManager({ subject: userID, sessionID }, customerID, db);
  }
  async function getMailOnboarding(headers: Headers, customerID: string) {
    return withAuthenticatedDatabase(headers, async () => {
      const { tenant } = await tenantIdentity(headers, customerID);
      const source = (
        await database.query(
          "SELECT revision FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",
          [customerID],
        )
      ).rows[0];
      return {
        canManage: tenant.canManage,
        operator: tenant.operator,
        byokAllowed: (await byokPolicy(database, customerID)).turbosmtp,
        providerRevision: source?.revision ?? 0,
      };
    });
  }
  async function configureManagedMail(
    headers: Headers,
    customerID: string,
  ): Promise<{ id: string }> {
    return withAuthenticatedDatabase(headers, async () => {
      requireMail("provision");
      const identity = await tenantIdentity(headers, customerID, true);
      if (!identity.tenant.operator)
        throw new MailError(
          403,
          "operator_required",
          "Platform operator access is required",
        );
      const connection = await withMailTransaction(async (db) => {
        const authority = await assertCurrentManager(
          db,
          identity.session.subject,
          identity.session.sessionID,
          customerID,
        );
        if (!authority.operator)
          throw new MailError(
            403,
            "operator_required",
            "Platform operator access is required",
          );
        const flags = (
          await db.query(
            "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
          )
        ).rows[0]?.settings;
        if (flags?.mailGatewayEnabled !== true || flags?.mailEnabled !== true)
          throw new MailError(
            503,
            "gateway_disabled",
            "Managed Mail gateway is disabled",
          );
        const source = (
          await db.query(
            "SELECT provider_id FROM webdock_auth.mail_tenant_account WHERE customer_id=$1 AND state='ready'",
            [customerID],
          )
        ).rows[0];
        if (!source?.provider_id)
          throw new MailError(
            409,
            "account_required",
            "Provision a managed Mail account first",
          );
        const existing = (
          await db.query(
            "SELECT id FROM webdock_auth.mail_connection WHERE customer_id=$1 AND kind='platform' AND provider_account_id=$2 AND deleted_at IS NULL",
            [customerID, source.provider_id],
          )
        ).rows[0];
        if (existing) return existing;
        return (
          await db.query(
            "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id,region,state) VALUES($1,'platform',$2,$3,'pending') RETURNING id",
            [
              customerID,
              source.provider_id,
              flags.mailRegion === "global" ? "global" : "eu",
            ],
          )
        ).rows[0];
      });
      await ensureManagedSendingCredential({
        customerID,
        connectionID: connection.id,
        principal: {
          kind: "session",
          userID: identity.session.subject,
          sessionID: identity.session.sessionID,
        },
      });
      return { id: connection.id };
    });
  }

  return {
    listTenantMailConnections,
    connectByokMail,
    getMailOnboarding,
    configureManagedMail,
  };
}
