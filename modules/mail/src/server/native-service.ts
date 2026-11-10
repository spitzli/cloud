import type { IdentityDirectory } from "@spitzli/core/identity";
import {
  getMailService,
  requestMailService,
  MailServiceError,
} from "./instances";
import type { NativeMailPorts } from "./native-ports";
export { MailServiceError };
export interface NativeServiceDependencies {
  identity: Pick<IdentityDirectory, "subject">;
  requireMail: NativeMailPorts["requireMail"];
  withAuthenticatedDatabase: NativeMailPorts["withAuthenticatedDatabase"];
  database: NativeMailPorts["database"];
  getTenant: NativeMailPorts["getTenant"];
  requireAccessOperator: NativeMailPorts["requireAccessOperator"];
  getPlatformSettings: NativeMailPorts["getPlatformSettings"];
  env: NativeMailPorts["env"];
}
export function createNativeService(dependencies: NativeServiceDependencies) {
  const {
    requireMail,
    withAuthenticatedDatabase,
    database,
    getTenant,
    requireAccessOperator,
    getPlatformSettings,
    env,
  } = dependencies;
  async function getTenantMailService(headers: Headers, customerID: string) {
    return withAuthenticatedDatabase(headers, async () => {
      const access = await getTenant(headers, customerID);
      if (env.WEBDOCK_NATIVE_MAIL_ENABLED !== "true") return null;
      const [service, settings] = await Promise.all([
        getMailService(database, customerID),
        getPlatformSettings(),
      ]);
      const verified =
        access.operator &&
        service.state === "needs_review" &&
        (
          await database.query(
            "SELECT customer_id FROM webdock_mail.instance WHERE customer_id=$1 AND verified_at IS NOT NULL",
            [customerID],
          )
        ).rowCount;
      return {
        service,
        mailboxesAvailable: env.WEBDOCK_NATIVE_MAILBOXES_ENABLED === "true",
        sendingAvailable: env.WEBDOCK_NATIVE_MAIL_SENDING_ENABLED === "true",
        canReconcile:
          Boolean(verified) &&
          (!service.enabled ||
            (access.tenant.status === "active" && settings.mailEnabled)),
        canActivate:
          access.operator &&
          access.tenant.status === "active" &&
          settings.mailEnabled,
        canSuspend: access.operator && service.enabled,
      };
    });
  }

  async function manageTenantMailService(
    headers: Headers,
    customerID: string,
    input: Record<string, string>,
  ) {
    return withAuthenticatedDatabase(headers, async () => {
      if (env.WEBDOCK_NATIVE_MAIL_ENABLED !== "true")
        throw new MailServiceError("Native Mail is not available yet.");
      const session = await requireAccessOperator(headers);
      await getTenant(headers, customerID);
      requireMail();
      if (
        input.action === "activate" &&
        (await getMailService(database, customerID)).state === "disabled"
      )
        requireMail("provision");
      if (!["activate", "suspend", "reconcile"].includes(input.action))
        throw new MailServiceError("Unknown Mail service action.");
      const hostID =
        env.MAIL_MANAGED_CLUSTER_ID ||
        (env.AUTH_TEST_MAIL === "true"
          ? env.MAIL_DEFAULT_HOST_ID || "primary"
          : "");
      if (
        !hostID ||
        (env.AUTH_TEST_MAIL !== "true" && !/^[1-9][0-9]{0,18}$/.test(hostID))
      )
        throw new MailServiceError(
          "A managed Mail cluster must be configured.",
        );
      const enabled =
        input.action === "reconcile"
          ? (await getMailService(database, customerID)).enabled
          : input.action === "activate";
      const result = await requestMailService(
        database,
        {
          customerID,
          enabled,
          reconcile: input.action === "reconcile",
          actorID: session.user.id,
          expectedRevision: input.revision,
          hostID,
        },
        dependencies.identity,
      );
      return {
        message:
          result.state === "needs_review"
            ? "Mail change recorded. An operator must reconcile the existing operation."
            : result.state === "pending" || result.state === "provisioning"
              ? "Mail change queued. Existing mailbox data will be retained."
              : "Mail settings are unchanged.",
      };
    });
  }
  return { getTenantMailService, manageTenantMailService };
}
