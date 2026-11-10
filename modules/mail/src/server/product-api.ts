import type { MailProductStore } from "./provider-ports";
import type {
  EmailRequest,
  EventType,
  Permission,
} from "@spitzli/mail-contracts";
import { MailError } from "./product-access";
import { mailOperations, type MailOperation } from "./product-api-contract";
type Dependencies = {
  mailEndpoint: ReturnType<
    typeof import("./product-http").createMailHttp
  >["mailEndpoint"];
  submitMail: ReturnType<
    typeof import("./product-sending").createMailSending
  >["submitMail"];
  submitMailBatch: ReturnType<
    typeof import("./product-batches").createMailBatches
  >["submitMailBatch"];
  getMailBatch: ReturnType<
    typeof import("./product-batches").createMailBatches
  >["getMailBatch"];
  listMailMessages: ReturnType<
    typeof import("./product-messages").createMailMessages
  >["listMailMessages"];
  getMailMessage: ReturnType<
    typeof import("./product-messages").createMailMessages
  >["getMailMessage"];
  getMailAnalytics: ReturnType<
    typeof import("./product-analytics").createMailAnalytics
  >["getMailAnalytics"];
  rescheduleMail: ReturnType<
    typeof import("./product-schedules").createMailSchedules
  >["rescheduleMail"];
  cancelScheduledMail: ReturnType<
    typeof import("./product-schedules").createMailSchedules
  >["cancelScheduledMail"];
  createMailKey: ReturnType<
    typeof import("./product-api-keys").createMailApiKeys
  >["createMailKey"];
  revokeMailKey: ReturnType<
    typeof import("./product-api-keys").createMailApiKeys
  >["revokeMailKey"];
  claimMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["claimMailDomain"];
  getMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["getMailDomain"];
  verifyMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["verifyMailDomain"];
  removeMailDomain: ReturnType<
    typeof import("./product-domains").createMailDomains
  >["removeMailDomain"];
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
  listMailResource: ReturnType<
    typeof import("./product-resource-lists").createMailResourceLists
  >["listMailResource"];
  getMailLog: ReturnType<
    typeof import("./product-resource-lists").createMailResourceLists
  >["getMailLog"];
  database: MailProductStore["database"];
};

export function createMailApi(dependencies: Dependencies) {
  const {
    mailEndpoint,
    submitMail,
    submitMailBatch,
    getMailBatch,
    listMailMessages,
    getMailMessage,
    getMailAnalytics,
    rescheduleMail,
    cancelScheduledMail,
    createMailKey,
    revokeMailKey,
    claimMailDomain,
    getMailDomain,
    verifyMailDomain,
    removeMailDomain,
    createMailWebhook,
    getMailWebhook,
    updateMailWebhook,
    deleteMailWebhook,
    rotateMailWebhook,
    replayMailWebhook,
    listMailWebhookDeliveries,
    setMailSuppression,
    removeMailSuppression,
    listMailResource,
    getMailLog,
    database,
  } = dependencies;
  function input(value: unknown): Record<string, unknown> {
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new MailError(422, "validation_error", "Expected a JSON object");
    return value as Record<string, unknown>;
  }
  function stringInput(value: unknown): string {
    if (typeof value !== "string")
      throw new MailError(422, "validation_error", "Expected a string");
    return value;
  }
  function revision(request: Request) {
    const raw = request.headers.get("if-match");
    if (!raw)
      throw new MailError(
        428,
        "revision_required",
        "Send the current revision in If-Match",
      );
    const match = /^(?:"([1-9][0-9]*)"|([1-9][0-9]*))$/.exec(raw);
    const value = Number(match?.[1] ?? match?.[2]);
    if (!Number.isSafeInteger(value))
      throw new MailError(422, "invalid_revision", "Invalid If-Match revision");
    return value;
  }
  function mailRoute(operation: MailOperation) {
    const contract = mailOperations.find((row) => row.id === operation)!;
    return mailEndpoint(
      contract.permission,
      async (request, actor, context) => {
        const url = new URL(request.url),
          id = context.params.id,
          body = context.body;
        if (
          url.searchParams.has("include_simulated") &&
          !["true", "false"].includes(
            url.searchParams.get("include_simulated")!,
          )
        )
          throw new MailError(
            422,
            "validation_error",
            "Invalid simulator filter",
          );
        const page = {
          limit: url.searchParams.has("limit")
            ? Number(url.searchParams.get("limit"))
            : undefined,
          after: url.searchParams.get("after") ?? undefined,
          search: url.searchParams.get("search") ?? undefined,
        };
        const ok = (body: unknown, status = 200) => ({ status, body });
        switch (operation) {
          case "emails.send":
            return submitMail(actor, body as EmailRequest, {
              key: request.headers.get("idempotency-key") ?? undefined,
            });
          case "emails.batch":
            return submitMailBatch(actor, body as EmailRequest[], {
              key: request.headers.get("idempotency-key") ?? undefined,
            });
          case "emails.list":
            return ok(
              await listMailMessages(actor, {
                ...page,
                search: url.searchParams.get("search") ?? undefined,
                domainID: url.searchParams.get("domain_id") ?? undefined,
                status: url.searchParams.get("status") ?? undefined,
                includeSimulated:
                  url.searchParams.get("include_simulated") === "true",
              }),
            );
          case "emails.get":
            return ok(await getMailMessage(actor, id));
          case "emails.reschedule":
            await rescheduleMail(
              actor,
              id,
              input(body).scheduled_at as string,
              revision(request),
            );
            return ok({ id });
          case "emails.cancel":
            await cancelScheduledMail(actor, id, revision(request));
            return ok({ id });
          case "batches.get":
            return ok(await getMailBatch(actor, id));
          case "domains.list":
            return ok(await listMailResource(actor, "domains", page));
          case "domains.create":
            return ok(
              await claimMailDomain(actor, stringInput(input(body).name)),
              201,
            );
          case "domains.get":
            return ok(await getMailDomain(actor, id));
          case "domains.verify":
            return ok(await verifyMailDomain(actor, id));
          case "domains.delete":
            await removeMailDomain(actor, id);
            return ok({ id });
          case "keys.list":
            return ok(await listMailResource(actor, "keys", page));
          case "keys.create": {
            const value = input(body);
            return ok(
              await createMailKey(actor, {
                name: value.name as string,
                permission: value.permission as Permission,
                domainID: value.domain_id as string | undefined,
              }),
              201,
            );
          }
          case "keys.delete":
            await revokeMailKey(actor, id);
            return ok({ id });
          case "webhooks.list":
            return ok(await listMailResource(actor, "webhooks", page));
          case "webhooks.create": {
            const value = input(body);
            return ok(
              await createMailWebhook(actor, {
                url: value.url as string,
                events: value.events as EventType[],
              }),
              201,
            );
          }
          case "webhooks.get":
            return ok(await getMailWebhook(actor, id));
          case "webhooks.update": {
            const value = input(body);
            await updateMailWebhook(actor, id, {
              url: value.url as string | undefined,
              events: value.events as EventType[] | undefined,
              enabled: value.enabled as boolean | undefined,
            });
            return ok({ id });
          }
          case "webhooks.delete":
            await deleteMailWebhook(actor, id);
            return ok({ id });
          case "webhooks.rotate":
            return ok(await rotateMailWebhook(actor, id));
          case "webhooks.deliveries":
            return ok(await listMailWebhookDeliveries(actor, id, page));
          case "webhooks.replay": {
            await getMailWebhook(actor, id);
            const delivery = context.params.deliveryID;
            if (
              !(
                await database.query(
                  "SELECT id FROM webdock_auth.mail_webhook_delivery WHERE id=$1 AND webhook_id=$2 AND customer_id=$3 AND connection_id=$4",
                  [delivery, id, actor.customerID, actor.connectionID],
                )
              ).rowCount
            )
              throw new MailError(404, "not_found", "Delivery not found");
            await replayMailWebhook(actor, delivery);
            return ok({ id: delivery });
          }
          case "suppressions.list":
            return ok(await listMailResource(actor, "suppressions", page));
          case "suppressions.create": {
            const value = input(body);
            await setMailSuppression(actor, {
              email: value.email as string,
              reason: (value.reason ?? "manual") as string,
            });
            return ok({ email: value.email }, 201);
          }
          case "suppressions.delete":
            await removeMailSuppression(actor, context.params.email);
            return ok({ email: context.params.email });
          case "logs.list":
            return ok(
              await listMailResource(actor, "logs", {
                ...page,
                method: url.searchParams.get("method") ?? undefined,
                status: url.searchParams.has("status")
                  ? Number(url.searchParams.get("status"))
                  : undefined,
              }),
            );
          case "logs.get":
            return ok(await getMailLog(actor, id));
          case "analytics.get":
            return ok(
              await getMailAnalytics(actor, {
                days: Number(url.searchParams.get("days") ?? 30) as 7 | 30 | 90,
                domainID: url.searchParams.get("domain_id") ?? undefined,
                includeSimulated:
                  url.searchParams.get("include_simulated") === "true",
              }),
            );
        }
      },
      contract.path,
    );
  }
  return { mailRoute };
}
