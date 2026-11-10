import { z } from "zod";
const id = z
  .string()
  .regex(/^[1-9][0-9]{0,18}$/)
  .refine((v) => BigInt(v) <= 9223372036854775807n);
const name = z.string().trim().min(1).max(160);
const key = z.string().regex(/^[A-Za-z0-9_-]{16,128}$/);
export const namespaceSchema = z
  .string()
  .max(63)
  .regex(/^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/)
  .refine(
    (v) =>
      ![
        "kube-system",
        "kube-public",
        "kube-node-lease",
        "spitzli-agent",
      ].includes(v),
  );
export const byokPolicySchema = z
  .object({
    kubernetes: z.boolean(),
    vercel: z.boolean(),
    turbosmtp: z.boolean().default(false),
    maxMailDomains: z.number().int().min(0).max(1000).default(10),
    maxClusters: z.number().int().min(0).max(100),
    manageExisting: z.boolean(),
    namespaces: z.array(namespaceSchema).max(100),
  })
  .strict();
export const resourceKindSchema = z.enum([
  "Node",
  "Namespace",
  "Deployment",
  "StatefulSet",
  "DaemonSet",
  "Pod",
  "Job",
  "CronJob",
  "Service",
  "Ingress",
  "PersistentVolumeClaim",
  "StorageClass",
]);
const nonnegative = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
export const inventoryResourceSchema = z
  .object({
    uid: z.string().min(1).max(128),
    resourceVersion: z.string().min(1).max(128),
    kind: resourceKindSchema,
    name,
    namespace: z.string().max(63),
    status: z.enum(["ready", "pending", "stopped", "failed", "unknown"]),
    replicas: nonnegative.optional(),
    ready: nonnegative.optional(),
    restarts: nonnegative.optional(),
    cpuMillicores: nonnegative.optional(),
    memoryBytes: nonnegative.optional(),
    storageBytes: nonnegative.optional(),
    usageCPU: nonnegative.optional(),
    usageMemory: nonnegative.optional(),
    suspended: z.boolean().optional(),
    controlled: z.boolean().optional(),
  })
  .strict();
export const inventorySchema = z
  .object({
    clusterUID: z.string().min(1).max(128),
    observedAt: z.iso.datetime(),
    complete: z.boolean(),
    unavailable: z.array(z.string().max(100)).max(30),
    resources: z.array(inventoryResourceSchema).max(2000),
  })
  .strict();
export type Inventory = z.infer<typeof inventorySchema>;
export type InventoryResource = z.infer<typeof inventoryResourceSchema>;
export const byokCommands = [
  z.object({ action: z.literal("byok.mail.get"), customerID: id }).strict(),
  z
    .object({
      action: z.literal("byok.mail.connect"),
      customerID: id,
      label: name,
      consumerKey: z
        .string()
        .min(1)
        .max(4096)
        .regex(/^[\x21-\x7e]+$/),
      consumerSecret: z
        .string()
        .min(1)
        .max(4096)
        .regex(/^[\x21-\x7e]+$/),
      confirmAccountAccess: z.literal(true),
      revision: nonnegative,
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.mail.refresh"),
      customerID: id,
      revision: nonnegative,
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.mail.disconnect"),
      customerID: id,
      revision: nonnegative,
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.mail.domain"),
      customerID: id,
      revision: nonnegative,
      domain: z.string().min(1).max(253),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.mail.review"),
      customerID: id,
      revision: nonnegative,
      confirmDomain: z.string().min(1).max(253),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.reconcile"),
      operationID: id,
      resourceVersion: z.string().min(1).max(128),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.vercel.region"),
      customerID: id,
      projectID: z.string().regex(/^prj_[A-Za-z0-9_-]+$/),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.vercel.cancel"),
      customerID: id,
      projectID: z.string().regex(/^prj_[A-Za-z0-9_-]+$/),
      deploymentID: z.string().regex(/^dpl_[A-Za-z0-9]+$/),
    })
    .strict(),
  z.object({ action: z.literal("byok.policy.get"), customerID: id }).strict(),
  z
    .object({
      action: z.literal("byok.policy.set"),
      customerID: id,
      revision: nonnegative,
      ...byokPolicySchema.shape,
      turbosmtp: z.boolean().optional(),
      maxMailDomains: z.number().int().min(0).max(1000).optional(),
    })
    .strict(),
  z.object({ action: z.literal("byok.list"), customerID: id }).strict(),
  z
    .object({
      action: z.literal("byok.register"),
      customerID: id,
      name,
      provider: z.enum(["k3s", "kubernetes"]),
      region: name,
      locationEvidence: z.string().min(1).max(2000),
      idempotencyKey: key,
    })
    .strict(),
  z.object({ action: z.literal("byok.cluster"), clusterID: id }).strict(),
  z.object({ action: z.literal("byok.enrollment"), clusterID: id }).strict(),
  z
    .object({
      action: z.literal("byok.revoke"),
      clusterID: id,
      revision: nonnegative,
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.inventory"),
      view: z.enum(["applications", "all"]).default("all"),
      clusterID: id,
      namespace: z.string().max(63).optional(),
      kind: resourceKindSchema.optional(),
      search: z.string().max(160).default(""),
      page: z.number().int().min(1).max(100).default(1),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.scan"),
      clusterID: id,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.workload"),
      clusterID: id,
      uid: z.string().min(1).max(128),
      resourceVersion: z.string().min(1).max(128),
      operation: z.enum([
        "scale",
        "stop",
        "start",
        "restart",
        "logs",
        "suspend",
        "resume",
      ]),
      replicas: z.number().int().min(0).max(100).optional(),
      idempotencyKey: key,
    })
    .strict(),
  z.object({ action: z.literal("byok.vercel.begin"), customerID: id }).strict(),
  z
    .object({
      action: z.literal("byok.vercel.finish"),
      state: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
      code: z.string().min(1).max(512),
      configurationID: z.string().regex(/^icfg_[A-Za-z0-9_-]+$/),
      teamID: z.string().regex(/^team_[A-Za-z0-9_-]+$/),
    })
    .strict(),
  z
    .object({ action: z.literal("byok.vercel.projects"), customerID: id })
    .strict(),
  z
    .object({
      action: z.literal("byok.vercel.select"),
      customerID: id,
      revision: nonnegative,
      projects: z.array(z.string().regex(/^prj_[A-Za-z0-9_-]+$/)).max(100),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.vercel.resources"),
      customerID: id,
      projectID: z.string().regex(/^prj_[A-Za-z0-9_-]+$/),
    })
    .strict(),
  z
    .object({
      action: z.literal("byok.vercel.disconnect"),
      customerID: id,
      revision: nonnegative,
    })
    .strict(),
] as const;
