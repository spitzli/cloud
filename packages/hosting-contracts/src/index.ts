import { setupCommands } from "./project-setup";
export * from "./project-setup";
import { appSpecSchema, preparedAppSpecSchema, type AppSpec } from "./app-spec";
export * from "./app-spec";
import { serviceCommands } from "./service-bindings";
export * from "./service-bindings";
import {
  resourceID,
  idempotencyKey as key,
  resourceRevision as revision,
} from "./validation";
export { resourceID } from "./validation";
import { databaseCommands, databaseReadActions } from "./databases";
export * from "./databases";
import { gitCommands, gitReadActions } from "./git-deployments";
export * from "./git-deployments";
import { byokCommands } from "./byok";
export * from "./byok";
import { parseResourceInput } from "./units";
export { parseResourceInput, resourceInput, formatResource } from "./units";
import { z } from "zod";
import { environmentPatchSchema } from "./environment";
export {
  environmentPatchSchema,
  environmentNameSchema,
  environmentValueSchema,
  type EnvironmentPatch,
} from "./environment";
export const hostingDimensions = [
  "apps",
  "cpuMillicores",
  "memoryBytes",
  "volumeBytes",
  "ephemeralBytes",
  "replicasPerApp",
  "concurrentDeployments",
] as const;
export type HostingDimension = (typeof hostingDimensions)[number];
export type HostingAllowances = Record<HostingDimension, number | null>;
export type HostingDemand = Record<HostingDimension, number>;
export type Enforcement =
  "provider-enforced" | "webdock-enforced" | "observed-only" | "unsupported";
export type HostingActor = {
  pluginProvisioningID?: string;
  consoleIdentity?: {issuer:string;userID:string;sessionID:string;organizationID:string|null};
  subject: string;
  sessionID: string;
  source: "studio" | "oauth" | "git-policy";
  gitSourceID?: string;
  gitSourceRevision?: number;
  scopes: readonly string[];
};
const quantity = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
export const allowanceSchema = z
  .object(
    Object.fromEntries(
      hostingDimensions.map((k) => [k, quantity.nullable().optional()]),
    ) as Record<
      HostingDimension,
      z.ZodOptional<z.ZodNullable<typeof quantity>>
    >,
  )
  .strict();
export function normalizeHostingAllowances(value: unknown): HostingAllowances {
  const parsed = allowanceSchema.parse(value === undefined ? {} : value);
  return Object.fromEntries(
    hostingDimensions.map((k) => [k, parsed[k] === undefined ? 0 : parsed[k]]),
  ) as HostingAllowances;
}
export function effectiveHostingAllowances(
  base: unknown,
  extras: unknown,
): HostingAllowances {
  const a = normalizeHostingAllowances(base),
    b = normalizeHostingAllowances(extras);
  return Object.fromEntries(
    hostingDimensions.map((k) => {
      if (b[k] === null) throw Error("Hosting extras must be finite.");
      const value = a[k] === null ? null : a[k] + b[k];
      if (value !== null && !Number.isSafeInteger(value))
        throw Error("Hosting allowance exceeds the supported range.");
      return [k, value];
    }),
  ) as HostingAllowances;
}
export function parseHostingFields(
  input: Record<string, string>,
  previous?: unknown,
  extras = false,
): HostingAllowances {
  const values = normalizeHostingAllowances(previous);
  for (const key of hostingDimensions) {
    const text = input[`hosting.${key}`];
    if (text === undefined) continue;
    if (text === "unlimited" && !extras) {
      values[key] = null;
      continue;
    }
    values[key] = parseResourceInput(key, text);
  }
  return values;
}
export const hostingLabels: Record<HostingDimension, string> = {
  apps: "Hosting apps",
  cpuMillicores: "CPU cores",
  memoryBytes: "Memory",
  volumeBytes: "Persistent storage",
  ephemeralBytes: "Temporary storage",
  replicasPerApp: "Replicas per app",
  concurrentDeployments: "Concurrent deployments",
};
const name = z.string().trim().min(1).max(160);
const provider = z.enum(["k3s", "vercel"]);
const mode = z.enum(["managed", "selfservice"]);
const paging = {
  sort: z.enum(["name", "-name"]).optional(),
  page: z.number().int().min(1).max(10000).default(1),
  limit: z.number().int().min(1).max(50).default(20),
  search: z.string().max(160).default(""),
};
export function appDemand(spec: AppSpec, deleted = false): HostingDemand {
  return {
    apps: deleted ? 0 : 1,
    cpuMillicores: deleted ? 0 : spec.cpuMillicores * spec.replicas,
    memoryBytes: deleted ? 0 : spec.memoryBytes * spec.replicas,
    volumeBytes: spec.volumeBytes ?? 0,
    ephemeralBytes: deleted ? 0 : spec.ephemeralBytes * spec.replicas,
    replicasPerApp: deleted ? 0 : spec.replicas,
    concurrentDeployments: 1,
  };
}
export const publicAddressRequestSchema = z.object({
  label: z.string().min(1).max(63).regex(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/).optional(),
}).strict();
export const publicAddressViewSchema = z.object({
  id: resourceID, hostname: z.string(), url: z.string().url(),
  state: z.enum(["allocated", "ready", "retired"]), revision,
});
const appViewFields = {
  publicAddress: publicAddressViewSchema.nullable().optional(),
  environmentNames: z.array(z.string()).optional(),
  id: resourceID,
  name: z.string(),
  projectID: resourceID,
  revision: z.number(),
  observedRevision: z.number(),
  status: z.string(),
  operationID: resourceID.nullable(),
  lastError: z.string().nullable(),
  logs: z.string().nullable(),
};
export const appViewSchema = z.discriminatedUnion("lifecycle", [
  z.object({
    ...appViewFields,
    lifecycle: z.literal("draft"),
    spec: preparedAppSpecSchema,
  }),
  z.object({
    ...appViewFields,
    lifecycle: z.literal("deployed").default("deployed"),
    spec: appSpecSchema,
  }),
]);
export type HostingAppView = z.infer<typeof appViewSchema>;
export const commandSchema = z.discriminatedUnion("action", [
  z.object({ action: z.literal("addresses.list"), customerID: resourceID, ...paging }).strict(),
  z.object({
    action: z.literal("clusters.publicSites.configure"), clusterID: resourceID,
    domain: z.string().min(3).max(189).regex(/^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/),
    enabled: z.boolean(), policyRevision: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
    tenantDailyLimit: z.number().int().min(1).max(1000), globalDailyLimit: z.number().int().min(1).max(1000),
    trustedCustomerIDs: z.array(resourceID).max(100),
  }).strict(),

  ...serviceCommands,
  ...setupCommands,
  z
    .object({
      action: z.literal("projects.relocate"),
      projectID: resourceID,
      sourceClusterID: resourceID,
      targetClusterID: resourceID,
      revision,
      idempotencyKey: key,
      confirmSharedImages: z.literal(true),
      checkedAt: z.string().datetime(),
      evidence: z
        .array(
          z
            .object({
              appID: resourceID,
              revision,
              sourceUID: z.string().min(1).max(128),
              sourcePodsZero: z.literal(true),
              targetPodsZero: z.literal(true),
              backupSHA256: z.string().regex(/^[a-f0-9]{64}$/),
              restoredSHA256: z.string().regex(/^[a-f0-9]{64}$/),
            })
            .strict(),
        )
        .min(1)
        .max(50),
    })
    .strict(),
  ...databaseCommands,
  ...byokCommands,
  ...gitCommands,
  z
    .object({
      action: z.literal("clusters.activate"),
      clusterID: resourceID,
      revision,
      capacity: allowanceSchema,
      verificationEvidence: z.string().min(40).max(3000),
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.list"),
      projectID: resourceID,
      ...paging,
    })
    .strict(),
  z.object({ action: z.literal("apps.get"), appID: resourceID }).strict(),
  z
    .object({
      action: z.literal("apps.prepare"),
      publicAddress: publicAddressRequestSchema.optional(),
      projectID: resourceID,
      name,
      spec: preparedAppSpecSchema,
      environment: environmentPatchSchema.optional(),
      subscriptionRevision: revision,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.deploy"),
      publicAddress: publicAddressRequestSchema.optional(),
      appID: resourceID,
      spec: appSpecSchema,
      environment: environmentPatchSchema.optional(),
      revision,
      subscriptionRevision: revision,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.create"),
      publicAddress: publicAddressRequestSchema.optional(),
      environment: environmentPatchSchema.optional(),
      projectID: resourceID,
      name,
      spec: appSpecSchema,
      subscriptionRevision: revision,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.update"),
      publicAddress: publicAddressRequestSchema.optional(),
      environment: environmentPatchSchema.optional(),
      appID: resourceID,
      spec: appSpecSchema,
      revision,
      subscriptionRevision: revision,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.scale"),
      appID: resourceID,
      replicas: z.number().int().min(0).max(20),
      revision,
      subscriptionRevision: revision,
      idempotencyKey: key,
    })
    .strict(),
  ...(
    ["apps.start", "apps.stop", "apps.restart", "apps.rollback"] as const
  ).map((action) =>
    z
      .object({
        action: z.literal(action),
        appID: resourceID,
        revision,
        subscriptionRevision: revision,
        idempotencyKey: key,
      })
      .strict(),
  ),
  z
    .object({
      action: z.literal("apps.logs"),
      appID: resourceID,
      idempotencyKey: key,
    })
    .strict(),
  z.object({ action: z.literal("apps.deletion"), appID: resourceID }).strict(),
  z
    .object({ action: z.literal("apps.storageDeletion"), appID: resourceID })
    .strict(),
  z
    .object({
      action: z.literal("apps.purgeStorage"),
      appID: resourceID,
      confirmName: name,
      planHash: z.string().regex(/^[a-f0-9]{64}$/),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.delete"),
      appID: resourceID,
      confirmName: name,
      planHash: z.string().regex(/^[a-f0-9]{64}$/),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("apps.reconcile"),
      appID: resourceID,
      idempotencyKey: key,
    })
    .strict(),
  z.object({ action: z.literal("clusters.list"), ...paging }).strict(),
  z
    .object({ action: z.literal("clusters.get"), clusterID: resourceID })
    .strict(),
  z
    .object({
      action: z.literal("clusters.register"),
      name,
      provider: name,
      country: z
        .enum([
          "AT",
          "BE",
          "BG",
          "HR",
          "CY",
          "CZ",
          "DE",
          "DK",
          "EE",
          "ES",
          "FI",
          "FR",
          "GR",
          "HU",
          "IE",
          "IT",
          "LT",
          "LU",
          "LV",
          "MT",
          "NL",
          "PL",
          "PT",
          "RO",
          "SE",
          "SI",
          "SK",
        ])
        .nullable(),
      region: name,
      locationEvidence: z.string().trim().min(1).max(2000),
      dedicatedCustomerID: resourceID.optional(),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({ action: z.literal("clusters.enrollment"), clusterID: resourceID })
    .strict(),
  z
    .object({
      action: z.literal("clusters.revoke"),
      clusterID: resourceID,
      revision,
    })
    .strict(),
  z
    .object({ action: z.literal("limits.get"), customerID: resourceID })
    .strict(),
  z
    .object({
      action: z.literal("limits.set"),
      customerID: resourceID,
      projectID: resourceID.optional(),
      provider: provider.optional(),
      values: allowanceSchema,
      revision,
      subscriptionRevision: revision,
    })
    .strict(),
  z.object({ action: z.literal("usage.get"), customerID: resourceID }).strict(),
  z
    .object({ action: z.literal("projects.get"), projectID: resourceID })
    .strict(),
  z
    .object({ action: z.literal("projects.overview"), projectID: resourceID })
    .strict(),
  z.object({ action: z.literal("operations.overview"), ...paging }).strict(),
  z
    .object({
      action: z.literal("projects.list"),
      customerID: resourceID,
      ...paging,
    })
    .strict(),
  z
    .object({
      action: z.literal("projects.create"),
      projectID: resourceID,
      provider,
      clusterID: resourceID.optional(),
      mode,
      ownImages: z.boolean().default(false),
      confirmSharedImages: z.boolean().default(false),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("projects.update"),
      projectID: resourceID,
      mode,
      ownImages: z.boolean(),
      confirmSharedImages: z.boolean().default(false),
      revision,
    })
    .strict(),
  z
    .object({ action: z.literal("operations.get"), operationID: resourceID })
    .strict(),
]);
export type HostingCommand = z.infer<typeof commandSchema>;
export const readActions = new Set<HostingCommand["action"]>([
  "addresses.list",
  "services.list",
  "setup.get",
  "setup.list",
  "setup.options",
  "setup.inspectSource",
  ...databaseReadActions,
  ...gitReadActions,
  "byok.mail.get",
  "byok.policy.get",
  "byok.list",
  "byok.cluster",
  "byok.inventory",
  "byok.vercel.projects",
  "byok.vercel.resources",
  "apps.list",
  "apps.get",
  "apps.deletion",
  "apps.storageDeletion",
  "apps.logs",
  "clusters.list",
  "clusters.get",
  "limits.get",
  "usage.get",
  "projects.list",
  "projects.get",
  "projects.overview",
  "operations.overview",
  "operations.get",
]);
export type HostingOperationStatus =
  "queued" | "running" | "succeeded" | "failed" | "needs-reconciliation";
export class HostingError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export function hostingError(error: unknown) {
  return error instanceof HostingError
    ? { status: error.status, message: error.message }
    : error instanceof z.ZodError
      ? { status: 400, message: "Invalid hosting input." }
      : {
          status: 503,
          message:
            "Hosting is unavailable. Check the current state before retrying.",
        };
}
export const observationSchema = z
  .object({
    sequence: quantity.positive(),
    generation: quantity.positive(),
    observedAt: z.iso.datetime(),
    version: name,
    nodes: z
      .array(
        z.object({
          id: name,
          role: z.enum(["server", "agent"]),
          architecture: z.enum(["amd64", "arm64"]),
          cpuMillicores: quantity,
          memoryBytes: quantity,
        }),
      )
      .max(200),
    capacity: allowanceSchema,
    capabilities: z
      .object({
        networkIsolation: z.boolean(),
        restrictedPods: z.boolean(),
        storage: z.boolean(),
        ingress: z.boolean(),
        environment: z.boolean().optional(),
        serviceBindingsVersion: z.literal(1).optional(),
        storageVersion: z.literal(1).optional(),
        provisioningGuardVersion: z.literal(1).optional(),
        databaseVersion: z.literal(1).optional(),
        databasePools: z
          .array(
            z
              .object({
                node: name,
                poolID: z.uuid(),
                capacityBytes: quantity,
                availableBytes: quantity,
                ready: z.boolean(),
                requestedCpuMillicores: quantity,
                requestedMemoryBytes: quantity,
                observedDatabases: z
                  .array(
                    z
                      .object({
                        installationID: resourceID,
                        cpuMillicores: quantity,
                        memoryBytes: quantity,
                      })
                      .strict(),
                  )
                  .max(1000)
                  .optional(),
                allocatedBytes: quantity.optional(),
                allocations: z
                  .array(
                    z
                      .object({
                        installationID: resourceID,
                        volumeBytes: quantity,
                      })
                      .strict(),
                  )
                  .max(1000)
                  .optional(),
              })
              .strict(),
          )
          .max(200)
          .optional(),
        databaseNodePorts: z
          .array(z.number().int().min(30000).max(32767))
          .max(2768)
          .optional(),
        nativeMail: z
          .object({
            version: z.literal(1),
            configurationVersion: z
              .union([z.literal(1), z.literal(2), z.literal(3)])
              .optional(),
            image: z
              .string()
              .regex(/^[A-Za-z0-9.:-]+\/[A-Za-z0-9_./-]+@sha256:[a-f0-9]{64}$/),
          })
          .strict()
          .optional(),

        registryVersion: z.literal(1).optional(),
        publicRoutingVersion: z.literal(1).optional(),
      })
      .strict(),
  })
  .strict();
export type ClusterObservation = z.infer<typeof observationSchema>;
export type HostingClusterView = {
  ownership?: "platform" | "tenant";
  id: string;
  name: string;
  provider: string;
  country: string | null;
  region: string;
  locationEvidence: string;
  dedicatedCustomerID: string | null;
  revision: number;
  state: "unconnected" | "connected" | "stale" | "revoked";
  lastSeen: string | null;
  verified: boolean;
  capacity?: HostingAllowances;
  verificationEvidence?: string;
  workloadReady: boolean;
  observation: ClusterObservation | null;
};
export type HostingProjectView = {
  projectID: string;
  name: string;
  provider: "k3s" | "vercel";
  mode: "managed" | "selfservice";
  ownImages: boolean;
  revision: number;
  clusterID: string | null;
  clusterName: string | null;
  country: string | null;
  region: string | null;
};
export type HostingProjectContext = {
  projectID: string;
  name: string;
  customerID: string;
  customerName: string;
  operator: boolean;
  configured: boolean;
  provider: "k3s" | "vercel" | null;
};
export type HostingOverview = HostingPage<{
  id: string;
  name: string;
  status: string;
  projectID: string;
  projectName: string;
  customerID: string;
  customerName: string;
  revision: number;
  observedRevision: number;
}> & {
  readAt: string;
  operations: {
    id: string;
    action: string;
    status: HostingOperationStatus;
    createdAt: string;
    projectID: string | null;
    projectName: string | null;
    customerName: string | null;
  }[];
};
export type HostingLimitView = {
  effective: HostingAllowances;
  subscriptionRevision: number;
  limits: {
    scope: string;
    values: Partial<HostingAllowances>;
    revision: number;
  }[];
};
export type HostingUsageView = HostingLimitView & {
  reserved: HostingDemand;
  allocated: HostingDemand;
  measured: null;
  checkedAt: null;
  enforcement: Enforcement;
  overallocated: boolean;
};
export type HostingPage<T> = {
  docs: T[];
  page: number;
  limit: number;
  totalDocs: number;
  totalPages: number;
  operator?: boolean;
};

export type ProjectOverview = {
  project: Pick<
    import("./project-setup").SetupOptions["project"],
    "name" | "configured" | "provider" | "canManage"
  >;
  applications: import("./project-setup").SetupOptions["applications"];
  databaseCount: number;
  bindings: Pick<
    import("./service-bindings").ServiceBindingView,
    "id" | "appID" | "service" | "state"
  >[];
  setups: {
    id: string;
    name: string;
    state: import("./project-setup").SetupView["state"];
  }[];
  sources: {
    targetID: string;
    repositoryName: string | null;
    branch: string;
  }[];
};
