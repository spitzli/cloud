import { z } from "zod";
import {
  appViewSchema,
  appSpecSchema,
  commandSchema,
  environmentPatchSchema,
  parseResourceInput,
  resourceID,
} from "@webdock/hosting-contracts";
import { formText } from "./form";
const revision = z.number().int().nonnegative();
export const appSchema = appViewSchema.and(
  z.object({
    subscriptionRevision: revision,
    operator: z.boolean(),
    canManage: z.boolean(),
    canRequestLogs: z.boolean(),
  }),
);
export type HostingApp = z.infer<typeof appSchema>;
const paging = {
  page: z.number().int().positive(),
  limit: z.number().int().positive(),
  totalDocs: revision,
  totalPages: revision,
};
export const projectPageSchema = z.object({
  ...paging,
  operator: z.boolean(),
  docs: z
    .array(
      z.object({
        projectID: resourceID,
        name: z.string(),
        provider: z.string(),
        mode: z.enum(["managed", "selfservice"]),
        clusterID: resourceID.nullable(),
        clusterName: z.string().nullable(),
        revision,
        ownImages: z.boolean(),
      }),
    )
    .max(50),
});
export const projectSchema = z.object({
  projectID: resourceID,
  name: z.string(),
  customerID: resourceID,
  customerName: z.string(),
  operator: z.boolean(),
  configured: z.boolean(),
  publicSitePolicy:z.object({domain:z.string().nullable(),enabled:z.boolean()}).nullable().optional(),
  provider: z.string().nullable(),
});
export const appsPageSchema = z.object({
  ...paging,
  docs: z.array(appViewSchema).max(50),
  subscriptionRevision: revision,
  operator: z.boolean(),
  canManage: z.boolean(),
  ownImages: z.boolean(),
});
export const operationStatus = z.enum([
  "queued",
  "running",
  "succeeded",
  "failed",
  "needs-reconciliation",
]);
export const operationSchema = z.object({
  id: resourceID,
  action: z.string(),
  status: operationStatus,
  customerID: resourceID.nullable(),
  projectID: resourceID.nullable(),
  createdAt: z.string().nullable(),
});
export const receiptSchema = z.object({
  id: resourceID,
  appID: resourceID,
  status: operationStatus,
  revision: revision.optional(),
});
export type Receipt = z.infer<typeof receiptSchema>;
export const lifecycleActions = [
  "apps.start",
  "apps.stop",
  "apps.restart",
  "apps.rollback",
  "apps.scale",
] as const;
export type LifecycleAction = (typeof lifecycleActions)[number];
export function canManageApp(app: HostingApp, preview: boolean): boolean {
  return (
    !preview &&
    app.canManage &&
    app.lifecycle === "deployed" &&
    app.status !== "deleted"
  );
}
export function operationPolling(
  status: string | undefined,
  failed: boolean,
): number | false {
  return !failed && (status === "queued" || status === "running")
    ? 2000
    : false;
}
export function actionCommand(
  app: HostingApp,
  action: LifecycleAction,
  idempotencyKey: string,
  replicas?: number,
) {
  return commandSchema.parse({
    action,
    appID: app.id,
    revision: app.revision,
    subscriptionRevision: app.subscriptionRevision,
    idempotencyKey,
    ...(action === "apps.scale" ? { replicas } : {}),
  });
}
function formSpec(data: FormData) {
  const args: unknown = JSON.parse(formText(data, "args"));
  return {
    image: formText(data, "image"),
    args,
    port: Number(formText(data, "port")),
    healthPath: formText(data, "healthPath"),
    cpuMillicores: parseResourceInput("cpuMillicores", formText(data, "cpuMillicores")),
    memoryBytes: parseResourceInput("memoryBytes", formText(data, "memoryBytes")),
    ephemeralBytes: parseResourceInput("ephemeralBytes", formText(data, "ephemeralBytes")),
    replicas: Number(formText(data, "replicas")),
  };
}
// Only explicit changes are sent; saved values are never read back or resubmitted.
function formEnvironment(data: FormData) {
  const raw = data.get("environment");
  return environmentPatchSchema.parse(typeof raw === "string" && raw ? JSON.parse(raw) : []);
}
export function settingsCommand(
  app: HostingApp,
  data: FormData,
  idempotencyKey: string,
) {
  if (app.lifecycle !== "deployed")
    throw new Error(
      "Deploy this application before editing its configuration.",
    );
  const spec = appSpecSchema.parse({ ...app.spec, ...formSpec(data) });
  const environment = formEnvironment(data);
  const result = commandSchema.parse({
    action: "apps.update",
    appID: app.id,
    revision: app.revision,
    subscriptionRevision: app.subscriptionRevision,
    spec,
    idempotencyKey,
    ...(environment.length ? { environment } : {}),
  });
  if (result.action !== "apps.update")
    throw new Error("Unsupported application update.");
  return result;
}
export function createCommand(
  projectID: string,
  subscriptionRevision: number,
  data: FormData,
  idempotencyKey: string,
) {
  const spec = appSpecSchema.parse({
    template: "custom",
    ...formSpec(data),
    volumeBytes: parseResourceInput("volumeBytes", formText(data, "volumeBytes")),
  });
  const environment = formEnvironment(data);
  if (environment.some((row) => row.value === null))
    throw new Error("A new application has no variables to remove.");
  const result = commandSchema.parse({
    action: "apps.create",
    ...(data.get("publicAddress")==="on"?{publicAddress:{}}:{}),
    projectID,
    name: formText(data, "name").trim(),
    spec,
    subscriptionRevision,
    idempotencyKey,
    ...(environment.length ? { environment } : {}),
  });
  if (result.action !== "apps.create")
    throw new Error("Unsupported application creation.");
  return result;
}
export const deletionPlanSchema = z.object({
  plan: z.object({
    name: z.string(),
    resources: z.array(z.string()).max(16),
    persistentData: z.boolean(),
    volumeBytes: z.number().int().nonnegative(),
  }),
  planHash: z.string().regex(/^[a-f0-9]{64}$/),
});
export type DeletionPlan = z.infer<typeof deletionPlanSchema>;
/** The server re-derives the plan; a stale hash or mistyped name is rejected there as well. */
export function deleteCommand(
  app: HostingApp,
  review: DeletionPlan,
  typedName: string,
  idempotencyKey: string,
) {
  if (review.plan.name !== app.name || typedName !== app.name)
    throw new Error("Type the exact application name to confirm deletion.");
  return commandSchema.parse({
    action: "apps.delete",
    appID: app.id,
    confirmName: typedName,
    planHash: review.planHash,
    idempotencyKey,
  });
}

export const logReadSchema = z.object({
  id: resourceID,
  status: operationStatus,
  result: z.object({ logs: z.string(), truncated: z.boolean() }).nullable(),
  error: z.string().nullable(),
});

export { tenantsSchema } from "./workspace-contracts";

// Browser-side live checks mirroring the shared schema; the Core still validates every request.
export const fieldRules = {
  image: { pattern: "[a-zA-Z0-9.\\-]+(:[0-9]+)?/[a-zA-Z0-9_.\\/\\-]+@sha256:[a-f0-9]{64}", title: "Use registry/name@sha256: followed by 64 lowercase hex characters." },
  healthPath: { pattern: "/[a-zA-Z0-9\\/_\\-]*", title: "Start with / and use letters, digits, /, _ or -." },
  cpuMillicores: { pattern: "\\d+([.,]\\d+)?\\s*(cores)?", title: "For example, 0.5 cores." },
  bytes: { pattern: "\\d+([.,]\\d+)?\\s*(MB|GB|TB)?", title: "For example, 512 MB or 1 GB." },
} as const;
/** Startup arguments must be a JSON list of strings; reported live through native validity. */
export function checkArguments(input: HTMLTextAreaElement) {
  let valid = false;
  try {
    const value: unknown = JSON.parse(input.value);
    valid = Array.isArray(value) && value.length <= 20 && value.every((item) => typeof item === "string" && item.length <= 512);
  } catch { /* reported below */ }
  input.setCustomValidity(valid ? "" : 'Use a JSON list of strings, for example ["--serve"] or [].');
}
