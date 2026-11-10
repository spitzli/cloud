import type { MailProductStore } from "./provider-ports";
import type { MailFetch as Fetch } from "./provider-ports";
import { MailError } from "./product-access";
import { mailFetch } from "./product-deadline";
type Dependencies = {
  withAuthenticatedDatabase: import("./tenant-mail").TenantMailDependencies["withAuthenticatedDatabase"];
  database: MailProductStore["database"];
  authorizeMailSession: (
    headers: Headers,
    customerID: string,
    connectionID: string,
    capability: import("@spitzli/mail-contracts").MailCapability,
  ) => Promise<import("@spitzli/mail-contracts").MailActor>;
  assertMailAccess: MailProductStore["assertMailAccess"];
  mailManagementHeaders: ReturnType<
    typeof import("./provider-credentials").createMailCredentials
  >["mailManagementHeaders"];
  resolveSendingCredential: ReturnType<
    typeof import("./provider-credentials").createMailCredentials
  >["resolveSendingCredential"];
  trackingClient: import("./tracking-provider").MailTrackingFactory;
};

export function createMailTrackingContext(dependencies: Dependencies) {
  const {
    withAuthenticatedDatabase,
    database,
    authorizeMailSession,
    assertMailAccess,
    mailManagementHeaders,
    resolveSendingCredential,
    trackingClient,
  } = dependencies;
  async function productTrackingContext(
    headers: Headers,
    customerID: string,
    connectionID: string,
    write = false,
    fetcher: Fetch = mailFetch,
  ) {
    return withAuthenticatedDatabase(headers, async () => {
      const actor = await authorizeMailSession(
        headers,
        customerID,
        connectionID,
        write ? "manage" : "read",
      );
      const connection = (
        await database.query(
          "SELECT kind,state,revision FROM webdock_auth.mail_connection WHERE id=$1 AND customer_id=$2 AND deleted_at IS NULL",
          [connectionID, customerID],
        )
      ).rows[0];
      if (!connection)
        throw new MailError(404, "not_found", "Mail connection unavailable");
      const assertCurrent = async () => {
        await assertMailAccess(actor, "manage");
        if (connection.state !== "ready")
          throw new MailError(
            409,
            "connection_unavailable",
            "Mail connection is not ready",
          );
        if (
          connection.kind === "platform" &&
          !(
            await database.query(
              "SELECT 1 FROM webdock_auth.mail_tenant_account a JOIN webdock_auth.mail_connection c ON c.customer_id=a.customer_id AND c.provider_account_id=a.provider_id WHERE c.id=$1 AND a.state='ready' AND a.snapshot->>'active'='true'",
              [connectionID],
            )
          ).rowCount
        )
          throw new MailError(
            409,
            "connection_unavailable",
            "Managed Mail account is not ready",
          );
        if (
          !(
            await database.query(
              "SELECT 1 FROM webdock_auth.mail_connection WHERE id=$1 AND customer_id=$2 AND revision=$3 AND deleted_at IS NULL",
              [connectionID, customerID, connection.revision],
            )
          ).rowCount
        )
          throw new MailError(
            409,
            "connection_changed",
            "Mail connection changed",
          );
        if (connection.kind === "byok") await resolveSendingCredential(actor);
      };
      return {
        actor,
        state: connection.state as string,
        kind: connection.kind as "platform" | "byok",
        assertCurrent,
        client: async () => {
          await assertCurrent();
          const credentials = await mailManagementHeaders(actor, {
            requireActive: true,
          });
          return trackingClient(
            "Authorization" in credentials
              ? credentials.Authorization
              : {
                  consumerKey: credentials.consumerKey,
                  consumerSecret: credentials.consumerSecret,
                },
            fetcher,
          );
        },
      };
    });
  }
  return { productTrackingContext };
}
