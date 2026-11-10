import { z } from "zod";
import { resourceID, resourceRevision, idempotencyKey } from "./validation";
import { preparedAppSpecSchema } from "./app-spec";
import { environmentNameSchema, environmentPatchSchema } from "./environment";
import { gitBranch, gitRootDirectory } from "./git-deployments";
export const setupConfigurationSchema = z
  .object({
    name: z.string().trim().min(1).max(160),
    source: z.discriminatedUnion("kind", [
      z
        .object({
          kind: z.literal("git"),
          connectionID: resourceID.optional(),
          repositoryID: resourceID.optional(),
          branch: gitBranch,
          rootDirectory: gitRootDirectory,
          dockerfile: gitRootDirectory.default("Dockerfile"),
          buildProvider: z
            .enum(["isolated", "github-actions"])
            .default("isolated"),
          workflowPath: z
            .string()
            .regex(/^\.github\/workflows\/[A-Za-z0-9_-]+\.ya?ml$/)
            .default(".github/workflows/webdock.yml"),
          artifactPrefix: z
            .string()
            .regex(/^[A-Za-z0-9_-]{1,80}$/)
            .default("webdock"),
        })
        .strict(),
      z
        .object({
          kind: z.literal("image"),
          reference: z.string().min(1).max(512),
        })
        .strict(),
      z
        .object({
          kind: z.literal("template"),
          template: z.literal("healthcheck"),
        })
        .strict(),
    ]),
    spec: preparedAppSpecSchema.refine(
      (s) => s.replicas > 0,
      "Initial deployment requires at least one instance.",
    ),
    database: z
      .discriminatedUnion("mode", [
        z.object({ mode: z.literal("none") }).strict(),
        z
          .object({
            mode: z.literal("existing"),
            databaseID: resourceID,
            variable: environmentNameSchema.default("DATABASE_URL"),
          })
          .strict(),
        z
          .object({
            mode: z.literal("new"),
            name: z.string().trim().min(1).max(160),
            planRevisionID: resourceID,
            variable: environmentNameSchema.default("DATABASE_URL"),
          })
          .strict(),
      ])
      .default({ mode: "none" }),
    mail: z
      .object({
        connectionID: resourceID,
        domainID: resourceID,
        variable: environmentNameSchema.default("SPITZLI_MAIL_API_KEY"),
      })
      .strict()
      .nullable()
      .default(null),
    autoPublish: z.boolean().default(false),
  })
  .strict();
export type SetupConfiguration = z.infer<typeof setupConfigurationSchema>;
const ref = { projectID: resourceID, setupID: resourceID };
export const setupCommands = [
  z
    .object({
      action: z.literal("setup.save"),
      projectID: resourceID,
      setupID: resourceID.optional(),
      revision: resourceRevision,
      configuration: setupConfigurationSchema,
      environment: environmentPatchSchema.optional(),
      idempotencyKey,
    })
    .strict(),
  z.object({ action: z.literal("setup.get"), ...ref }).strict(),
  z
    .object({
      action: z.literal("setup.list"),
      projectID: resourceID,
      page: z.number().int().min(1).max(10000).default(1),
      limit: z.number().int().min(1).max(50).default(20),
    })
    .strict(),
  z
    .object({ action: z.literal("setup.options"), projectID: resourceID })
    .strict(),
  z
    .object({
      action: z.literal("setup.inspectSource"),
      ...ref,
      revision: resourceRevision,
    })
    .strict(),
  z
    .object({
      action: z.literal("setup.preview"),
      ...ref,
      revision: resourceRevision,
    })
    .strict(),
  z
    .object({
      action: z.literal("setup.confirm"),
      ...ref,
      revision: resourceRevision,
      previewID: z.uuid(),
      acceptedOfferIDs: z.array(resourceID).max(1),
      idempotencyKey,
    })
    .strict(),
  z
    .object({
      action: z.literal("setup.resume"),
      ...ref,
      revision: resourceRevision,
      idempotencyKey,
    })
    .strict(),
] as const;
export const setupCommandSchema = z.discriminatedUnion("action", setupCommands);
export const setupSteps = [
  "application",
  "database",
  "bindings",
  "source",
  "build",
  "release",
  "readiness",
] as const;
export type SetupStep = (typeof setupSteps)[number];
export type SetupState =
  | "draft"
  | "awaiting-confirmation"
  | "running"
  | "blocked"
  | "failed"
  | "ready";
export type SetupProgress = {
  name: SetupStep;
  attempt?: number;
  state: "pending" | "running" | "blocked" | "failed" | "succeeded" | "skipped";
  operationID: string | null;
  reason: string | null;
};
export type SetupResources = {
  appID?: string;
  databaseID?: string;
  bindingIDs?: string[];
  sourceID?: string;
  buildID?: string;
  releaseID?: string;
  operationID?: string;
};
export type SetupPreview = {
  id: string;
  revision: number;
  expiresAt: string;
  sourceSHA: string | null;
  image: string | null;
  subscriptionRevision: number;
  blockers: string[];
  offers: {
    id: string;
    name: string;
    monthlyPriceCents: number;
    currency: string;
    terms: string;
    expiresAt: string;
    backupStatus: string;
  }[];
};
export type SetupView = {
  id: string;
  projectID: string;
  revision: number;
  state: SetupState;
  configuration: SetupConfiguration;
  environmentNames: string[];
  resources: SetupResources;
  steps: SetupProgress[];
  preview: SetupPreview | null;
  reason: string | null;
};

export type SetupOptions = {
  enabled: boolean;
  project: {
    projectID: string;
    customerID: string;
    name: string;
    customerName: string;
    configured: boolean;
    provider: "k3s" | "vercel" | null;
    canManage: boolean;
    canAssignHosting: boolean;
  };
  applications: {
    id: string;
    name: string;
    lifecycle: "draft" | "deployed";
    status: string;
  }[];
  clusters: {
    id: string;
    name: string;
    country: string | null;
    region: string | null;
  }[];
  builds: { isolated: boolean; githubActions: boolean };
  connections: {
    id: string;
    accountLogin: string;
    state: string;
    repositories: {
      repositoryID: string;
      fullName: string;
      defaultBranch: string;
    }[];
  }[];
  databases: import("./databases").DatabaseView[];
  plans: {
    id: string;
    plan: import("./databases").DatabasePlan;
    availability: "disabled" | "pilot" | "public";
    revision: number;
  }[];
  mail: {
    id: string;
    kind: "platform" | "byok";
    state: string;
    domainID: string | null;
    domain: string | null;
    status: string | null;
  }[];
  allowances: {
    effective: import("./index").HostingAllowances;
    subscriptionRevision: number;
  };
};
export type SetupInspection = {
  sourceSHA: string;
  rootDirectory: string;
  dockerfile: string | null;
  port: number | null;
  suggestedServices: string[];
  warnings: string[];
};
