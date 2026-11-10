import type { MailProductStore } from "./provider-ports";
import type {
  EmailRequest,
  EventType,
  MailActor,
  Permission,
} from "@spitzli/mail-contracts";
import { parseID } from "@spitzli/mail-contracts/validation";
import { MailError } from "./product-access";
type Dependencies = {
  withAuthenticatedDatabase: import("./tenant-mail").TenantMailDependencies["withAuthenticatedDatabase"];
  getTenantTracking: ReturnType<
    typeof import("./tenant-tracking").createTenantTracking
  >["getTenantTracking"];
  manageTenantTracking: ReturnType<
    typeof import("./tenant-tracking").createTenantTracking
  >["manageTenantTracking"];
  rotateManagedSendingCredential: ReturnType<
    typeof import("./provider-credentials").createMailCredentials
  >["rotateManagedSendingCredential"];
  getMailHealth: ReturnType<
    typeof import("./product-health").createMailHealth
  >["getMailHealth"];
  reconcileMail: ReturnType<
    typeof import("./product-reconcile").createMailReconcile
  >["reconcileMail"];
  database: MailProductStore["database"];
  getTenant: import("./tenant-mail").TenantMailDependencies["getTenant"];
  authorizeMailSession: (
    headers: Headers,
    customerID: string,
    connectionID: string,
    capability: import("@spitzli/mail-contracts").MailCapability,
  ) => Promise<import("@spitzli/mail-contracts").MailActor>;
  assertMailAccess: MailProductStore["assertMailAccess"];
  listTenantMailConnections: ReturnType<
    typeof import("./product-onboarding").createMailOnboarding
  >["listTenantMailConnections"];
  getMailOnboarding: ReturnType<
    typeof import("./product-onboarding").createMailOnboarding
  >["getMailOnboarding"];
  connectByokMail: ReturnType<
    typeof import("./product-onboarding").createMailOnboarding
  >["connectByokMail"];
  configureManagedMail: ReturnType<
    typeof import("./product-onboarding").createMailOnboarding
  >["configureManagedMail"];
  listMailMessages: ReturnType<
    typeof import("./product-messages").createMailMessages
  >["listMailMessages"];
  getMailMessage: ReturnType<
    typeof import("./product-messages").createMailMessages
  >["getMailMessage"];
  getMailAnalytics: ReturnType<
    typeof import("./product-analytics").createMailAnalytics
  >["getMailAnalytics"];
  submitMail: ReturnType<
    typeof import("./product-sending").createMailSending
  >["submitMail"];
  cancelScheduledMail: ReturnType<
    typeof import("./product-schedules").createMailSchedules
  >["cancelScheduledMail"];
  rescheduleMail: ReturnType<
    typeof import("./product-schedules").createMailSchedules
  >["rescheduleMail"];
  getMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["getMailDomain"];
  claimMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["claimMailDomain"];
  verifyMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["verifyMailDomain"];
  removeMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["removeMailDomain"];
  createMailKey: ReturnType<
    typeof import("./product-api-keys").createMailApiKeys
  >["createMailKey"];
  revokeMailKey: ReturnType<
    typeof import("./product-api-keys").createMailApiKeys
  >["revokeMailKey"];
  listMailResource: ReturnType<
    typeof import("./product-resource-lists").createMailResourceLists
  >["listMailResource"];
  getMailLog: ReturnType<
    typeof import("./product-resource-lists").createMailResourceLists
  >["getMailLog"];
  createMailWebhook: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["createMailWebhook"];
  getMailWebhook: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["getMailWebhook"];
  updateMailWebhook: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["updateMailWebhook"];
  deleteMailWebhook: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["deleteMailWebhook"];
  rotateMailWebhook: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["rotateMailWebhook"];
  replayMailWebhook: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["replayMailWebhook"];
  listMailWebhookDeliveries: ReturnType<
    typeof import("./product-webhooks").createMailWebhooks
  >["listMailWebhookDeliveries"];
  setMailSuppression: ReturnType<
    typeof import("./product-suppressions").createMailSuppressions
  >["setMailSuppression"];
  removeMailSuppression: ReturnType<
    typeof import("./product-suppressions").createMailSuppressions
  >["removeMailSuppression"];
};
import { mailStudioShapes } from "./product-studio-shapes";
export { mailStudioShapes } from "./product-studio-shapes";
export function createMailStudio(dependencies: Dependencies) {
  const {
    withAuthenticatedDatabase,
    getTenantTracking,
    manageTenantTracking,
    rotateManagedSendingCredential,
    getMailHealth,
    reconcileMail,
    database,
    getTenant,
    authorizeMailSession,
    assertMailAccess,
    listTenantMailConnections,
    getMailOnboarding,
    connectByokMail,
    configureManagedMail,
    listMailMessages,
    getMailMessage,
    getMailAnalytics,
    submitMail,
    cancelScheduledMail,
    rescheduleMail,
    getMailDomain,
    claimMailDomain,
    verifyMailDomain,
    removeMailDomain,
    createMailKey,
    revokeMailKey,
    listMailResource,
    getMailLog,
    createMailWebhook,
    getMailWebhook,
    updateMailWebhook,
    deleteMailWebhook,
    rotateMailWebhook,
    replayMailWebhook,
    listMailWebhookDeliveries,
    setMailSuppression,
    removeMailSuppression,
  } = dependencies;
  async function dispatchMailStudio(
    operation: string,
    args: unknown[],
    headers: Headers,
  ): Promise<unknown> {
    return withAuthenticatedDatabase(headers, async () => {
      const shape = mailStudioShapes[operation];
      if (
        !shape ||
        shape.length !== args.length ||
        shape.some((check, index) => !check(args[index]))
      )
        throw new MailError(
          422,
          "validation_error",
          "Invalid Mail operation arguments",
        );
      const customerID = parseID(args[0]);
      if (operation === "mailConnections")
        return listTenantMailConnections(headers, customerID);
      if (operation === "mailOnboarding")
        return getMailOnboarding(headers, customerID);
      if (operation === "mailConnectByok")
        return connectByokMail(
          headers,
          customerID,
          args[1] as Parameters<typeof connectByokMail>[2],
        );
      if (operation === "mailConfigureManaged")
        return configureManagedMail(headers, customerID);
      const actor = await authorizeMailSession(
        headers,
        customerID,
        parseID(args[1]),
        "read",
      );
      const input = (args[2] ?? {}) as Record<string, unknown>,
        id = args[2] as string;
      const reads = [
        "mailTracking",
        "mailHealth",
        "mailOverview",
        "mailMessages",
        "mailMessage",
        "mailContent",
        "mailAnalytics",
        "mailDomains",
        "mailDomain",
      ];
      if (
        !reads.includes(operation) &&
        !["mailTestSend", "mailReschedule", "mailCancel"].includes(operation)
      )
        await assertMailAccess(actor, "manage", database, false);
      switch (operation) {
        case "mailRotateManaged":
          await rotateManagedSendingCredential(actor);
          return { id: actor.connectionID };
        case "mailTracking":
          return getTenantTracking(headers, customerID, actor.connectionID);
        case "mailTrackingUpdate": {
          if (Object.values(input).some((value) => typeof value !== "string"))
            throw new MailError(
              422,
              "validation_error",
              "Invalid tracking input",
            );
          return manageTenantTracking(
            headers,
            customerID,
            input as Record<string, string>,
            {},
            actor.connectionID,
          );
        }
        case "mailHealth":
          return getMailHealth(actor);
        case "mailOverview": {
          const tenant = await getTenant(headers, customerID);
          return {
            canManage: tenant.canManage,
            operator: tenant.operator,
            analytics: await getMailAnalytics(actor, { days: 7 }),
            messages: await listMailMessages(actor, { limit: 5 }),
            domains: await listMailResource(actor, "domains", { limit: 100 }),
          };
        }
        case "mailMessages":
          return listMailMessages(actor, input);
        case "mailMessage":
          return getMailMessage(actor, id);
        case "mailContent":
          return (await getMailMessage(actor, id)).content;
        case "mailTestSend": {
          if ("attachments" in input)
            throw new MailError(
              422,
              "validation_error",
              "Use the Mail API for attachments",
            );
          return submitMail(actor, input as EmailRequest, {
            key: input.idempotencyKey as string,
          });
        }
        case "mailReschedule":
          await rescheduleMail(actor, id, args[3] as string, args[4] as number);
          return { id };
        case "mailCancel":
          await cancelScheduledMail(actor, id, args[3] as number);
          return { id };
        case "mailDomains":
          return listMailResource(actor, "domains", input);
        case "mailDomain":
          return getMailDomain(actor, id);
        case "mailDomainCreate":
          return claimMailDomain(actor, id);
        case "mailDomainVerify":
          return verifyMailDomain(actor, id);
        case "mailDomainRemove":
          await removeMailDomain(actor, id);
          return { id };
        case "mailKeys":
          return listMailResource(actor, "keys", input);
        case "mailKeyCreate":
          return createMailKey(actor, {
            name: input.name as string,
            permission: input.permission as Permission,
            domainID: input.domainID as string | undefined,
          });
        case "mailKeyRevoke":
          await revokeMailKey(actor, id);
          return { id };
        case "mailAnalytics":
          return getMailAnalytics(
            actor,
            input as Parameters<typeof getMailAnalytics>[1],
          );
        case "mailWebhooks":
          return listMailResource(actor, "webhooks", input);
        case "mailWebhook":
          return getMailWebhook(actor, id);
        case "mailWebhookCreate":
          return createMailWebhook(
            actor,
            input as { url: string; events: EventType[] },
          );
        case "mailWebhookUpdate":
          await updateMailWebhook(
            actor,
            id,
            args[3] as Parameters<typeof updateMailWebhook>[2],
          );
          return { id };
        case "mailWebhookDelete":
          await deleteMailWebhook(actor, id);
          return { id };
        case "mailWebhookRotate":
          return rotateMailWebhook(actor, id);
        case "mailWebhookReplay":
          await replayMailWebhook(actor, id);
          return { id };
        case "mailWebhookDeliveries":
          return listMailWebhookDeliveries(
            actor,
            id,
            args[3] as { limit?: number; after?: string },
          );
        case "mailSuppressions":
          return listMailResource(actor, "suppressions", input);
        case "mailSuppressionSet":
          await setMailSuppression(
            actor,
            input as { email: string; reason: string },
          );
          return { saved: true };
        case "mailSuppressionRemove":
          await removeMailSuppression(actor, id);
          return { removed: true };
        case "mailLogs":
          return listMailResource(actor, "logs", input);
        case "mailLog":
          return getMailLog(actor, id);
        case "mailReconcile":
          await reconcileMail(actor, id);
          return { id };
        case "mailConnectionUpdate":
          return updateConnection(actor, input);
      }
      throw new MailError(422, "validation_error", "Unknown Mail operation");
    });
  }
  async function updateConnection(
    actor: MailActor,
    input: Record<string, unknown>,
  ) {
    if (
      !["pause", "resume"].includes(String(input.action)) ||
      !Number.isSafeInteger(input.revision)
    )
      throw new MailError(422, "validation_error", "Invalid connection update");
    await assertMailAccess(actor, "manage");
    const result = await database.query(
      "UPDATE webdock_auth.mail_connection SET state=$4,revision=revision+1,updated_at=now() WHERE id=$1 AND customer_id=$2 AND revision=$3 AND deleted_at IS NULL AND state IN ('ready','paused') RETURNING id,revision",
      [
        actor.connectionID,
        actor.customerID,
        input.revision,
        input.action === "pause" ? "paused" : "ready",
      ],
    );
    if (!result.rowCount)
      throw new MailError(
        409,
        "connection_changed",
        "Connection changed; refresh first",
      );
    return result.rows[0];
  }
  return { dispatchMailStudio };
}
