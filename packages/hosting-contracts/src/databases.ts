import { z } from "zod";
import {
  resourceID as id,
  idempotencyKey as key,
  resourceRevision as revision,
} from "./validation";
export const databaseKindSchema = z.enum(["shared", "dedicated"]);
export const databaseStateSchema = z.enum([
  "provisioning",
  "ready",
  "pausing",
  "paused",
  "resuming",
  "updating",
  "restoring",
  "degraded",
  "deleting",
  "retained",
  "purged",
  "needs-reconciliation",
]);
const name = z.string().trim().min(1).max(160);
const bytes = z.number().int().min(67108864).max(1099511627776);
const nullableBytes = bytes.nullable();
const cpu = z.number().int().min(100).max(64000).nullable();
const resourceShape = {
  kind: databaseKindSchema,
  postgresMajor: z.literal(18),
  cpuMillicores: cpu,
  memoryBytes: nullableBytes,
  volumeBytes: nullableBytes,
  logicalStorageAllowanceBytes: nullableBytes,
};
function validResources(v: {
  kind: string;
  cpuMillicores: number | null;
  memoryBytes: number | null;
  volumeBytes: number | null;
  logicalStorageAllowanceBytes: number | null;
}) {
  return v.kind === "shared"
    ? v.cpuMillicores === null &&
        v.memoryBytes === null &&
        v.volumeBytes === null &&
        v.logicalStorageAllowanceBytes !== null
    : v.cpuMillicores !== null &&
        v.memoryBytes !== null &&
        v.volumeBytes !== null &&
        v.logicalStorageAllowanceBytes === null;
}
export const databasePlanSchema = z
  .object({
    ...resourceShape,
    name,
    monthlyPriceCents: z.number().int().min(0).max(1000000),
    sponsoredCustomerID: id.optional(),
    currency: z.literal("EUR"),
    connectionLimit: z.number().int().min(2).max(100),
    terms: z.string().trim().min(40).max(6000),
  })
  .strict()
  .refine(validResources, "Resources must match the database kind.")
  .refine(
    (plan) =>
      plan.sponsoredCustomerID
        ? plan.monthlyPriceCents === 0
        : plan.monthlyPriceCents > 0,
    "Zero-price plans require a sponsorship customer.",
  );
export const databaseBackupStatusSchema = z.object({
  status: z.enum(["not-configured", "configuration-pending", "configured", "suspension-pending", "suspended"]),
  policyID: id.optional(),
  revision: revision.min(1).optional(),
  observedRevision: z.number().int().nonnegative().optional(),
  walEnabled: z.boolean().optional(),
  retentionDelete: z.boolean().optional(),
  observedControls: z.object({
    schedule: z.string().regex(/^0 [0-5]?[0-9] (?:[01]?[0-9]|2[0-3]) \* \* \*$/),
    walEnabled: z.boolean(),
    retentionDelete: z.boolean(),
    retentionDays: z.number().int().min(1).max(3650),
    scheduledBackupResourceVersion: z.string().min(1).max(256),
    clusterResourceVersion: z.string().min(1).max(256),
    objectStoreUID: z.string().min(1).max(256),
    objectStoreResourceVersion: z.string().min(1).max(256),
  }).strict().nullable(),
  lastSuccessAt: z.iso.datetime({ offset: true }).nullable(),
  recoverableUntil: z.null(),
  lastRestoreTestAt: z.null(),
  publicRecoveryAvailable: z.literal(false),
  disabledReason: z.literal("production-qualification-required"),
  rpoSeconds: z.null(),
  rtoSeconds: z.null(),
}).strict();
export type DatabaseBackupStatus = z.infer<typeof databaseBackupStatusSchema>;
const backupResourcesSchema = z.object({ cpuMillicores: z.number().int().min(100).max(64000), memoryBytes: z.number().int().min(134217728).max(1099511627776), ephemeralBytes: z.number().int().min(134217728).max(1099511627776) }).strict();
export const databaseBackupPolicyViewSchema = z.object({
  policy: z.object({
    id, revision: revision.min(1), revokedAt: z.iso.datetime({ offset: true }).nullable(),
    observedRevision: revision, observedSuspended: z.boolean().nullable(), observedAt: z.iso.datetime({ offset: true }).nullable(),
    observedControls: databaseBackupStatusSchema.shape.observedControls,
    revocationEffective: z.boolean(),
    spec: z.object({
      id, installationID: id, customerID: id, projectID: id, sourceCnpgUID: z.string().min(1).max(256),
      revision: revision.min(1), resourceRevision: revision.min(1),
      destination: z.object({ id, endpointURL: z.string().max(512), region: z.string().max(63), bucket: z.string().max(63), archivePath: z.string().max(512), serverName: z.string().max(256), credentialID: id, credentialRevision: revision.min(1), recoveryCredentialID: id, recoveryCredentialRevision: revision.min(1), keyRecoveryID: id }).strict(),
      schedule: z.string().regex(/^0 [0-5]?[0-9] (?:[01]?[0-9]|2[0-3]) \* \* \*$/),
      suspended: z.boolean(), walEnabled: z.boolean(), retentionDays: z.number().int().min(1).max(3650), retentionDelete: z.boolean(),
      sidecar: backupResourcesSchema, recoveryJob: backupResourcesSchema,
    }).strict(),
  }).strict().nullable(),
  backups: z.array(z.object({ id, policyID: id, baseBackupID: z.string().min(1).max(256), completedAt: z.iso.datetime({ offset: true }) }).strict()).max(100),
}).strict();
export type DatabaseBackupPolicyView = z.infer<typeof databaseBackupPolicyViewSchema>;
export const databaseViewSchema = z
  .object({
    ...resourceShape,
    id,
    projectID: id,
    name,
    revision,
    observedRevision: revision,
    state: databaseStateSchema,
    planRevisionID: id,
    storageEnforcement: z.enum(["measured-logical", "bounded-filesystem"]),
    databaseName: z.string().regex(/^db_[0-9]+$/),
    endpoint: z
      .object({
        host: z.string(),
        directPort: z.number().int().min(1).max(65535),
        pooledPort: z.number().int().min(1).max(65535).nullable(),
      })
      .strict()
      .nullable(),
    backup: databaseBackupStatusSchema,
    operationID: id.nullable(),
    observedAt: z.iso.datetime().nullable(),
    usage: z
      .object({
        logicalBytes: z.number().int().nonnegative(),
        filesystemBytes: z.number().int().nonnegative().nullable(),
        observedAt: z.iso.datetime(),
      })
      .strict()
      .nullable(),
    cancellationRequestedAt: z.iso.datetime().nullable(),
  })
  .strict()
  .refine(
    (v) =>
      validResources(v) &&
      v.storageEnforcement ===
        (v.kind === "shared" ? "measured-logical" : "bounded-filesystem"),
  );
const target = { databaseID: id, revision, idempotencyKey: key };
const copyTarget = {
  sourceDatabaseID: id,
  targetProjectID: id,
  name,
  restorePointID: id,
  targetOrderID: id,
  revision,
  idempotencyKey: key,
};
const backupSchedule = z.string().regex(/^0 [0-5]?[0-9] (?:[01]?[0-9]|2[0-3]) \* \* \*$/);
const backupPolicy = {
  ...target,
  revision: revision.min(1),
  schedule: backupSchedule,
  retentionDays: z.number().int().min(1).max(3650),
  retentionDelete: z.boolean(),
};
const backupCredentials = z.object({
  accessKeyId: z.string().min(3).max(128),
  secretAccessKey: z.string().min(16).max(512),
  readOnly: z.boolean(),
}).strict();
export const databaseCommands = [
  z.object({ action: z.literal("databases.backupPolicies.get"), databaseID: id }).strict(),
  z.object({ action: z.literal("databases.backupPolicies.grant"), ...backupPolicy, destinationID: id }).strict(),
  z.object({
    action: z.literal("databases.backupPolicies.update"), ...backupPolicy,
    policyRevision: revision.min(1), suspended: z.boolean(), walEnabled: z.boolean(),
  }).strict(),
  z.object({
    action: z.literal("databases.backupPolicies.revoke"), ...target,
    revision: revision.min(1), policyRevision: revision.min(1),
  }).strict(),
  z.object({
    action: z.literal("databases.backupDestinations.register"),
    endpointURL: z.string().max(512), region: z.string().max(63), bucket: z.string().max(63),
    keyRecoveryID: id, writeCredentials: backupCredentials, recoveryCredentials: backupCredentials,
    idempotencyKey: key,
  }).strict(),
  z
    .object({ action: z.literal("databases.migrations.list"), databaseID: id })
    .strict(),
  z
    .object({
      action: z.literal("databases.migrationPurgePreview"),
      migrationID: id,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.purgeMigrationSource"),
      migrationID: id,
      revision,
      confirmName: name,
      planHash: z.string().regex(/^[a-f0-9]{64}$/),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({ action: z.literal("databases.projects.list"), customerID: id })
    .strict(),
  z.object({ action: z.literal("databases.list"), projectID: id }).strict(),
  z
    .object({ action: z.literal("databases.permissions"), projectID: id })
    .strict(),
  z.object({ action: z.literal("databases.get"), databaseID: id }).strict(),
  z
    .object({
      action: z.literal("databases.plans.list"),
      projectID: id.optional(),
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.plans.create"),
      plan: databasePlanSchema,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.plans.setAvailability"),
      planRevisionID: id,
      availability: z.enum(["disabled", "pilot", "public"]),
      revision,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.offers.create"),
      projectID: id,
      planRevisionID: id,
      name,
      databaseID: id.optional(),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.acceptOrder"),
      offerID: id,
      projectID: id,
      confirmInterruption: z.boolean().optional(),
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.create"),
      projectID: id,
      name,
      planRevisionID: id,
      orderID: id,
      kind: databaseKindSchema,
      idempotencyKey: key,
    })
    .strict(),
  ...(
    [
      "pause",
      "resume",
      "restart",
      "backup",
      "rotateCredentials",
      "requestCancellation",
      "reconcile",
    ] as const
  ).map((action) =>
    z.object({ action: z.literal(`databases.${action}`), ...target }).strict(),
  ),
  z.object({
    action: z.literal("databases.restore"), ...copyTarget,
    targetTime: z.iso.datetime({ offset: true }),
    timeline: revision.min(1), credentialRevision: revision.min(1),
    verificationSHA256: z.string().regex(/^[a-f0-9]{64}$/),
  }).strict(),
  z.object({ action: z.literal("databases.clone"), ...copyTarget }).strict(),
  z
    .object({
      action: z.literal("databases.resize"),
      ...target,
      targetPlanRevisionID: id,
      targetOrderID: id,
    })
    .strict(),
  ...(["deletionPreview", "purgePreview"] as const).map((action) =>
    z
      .object({ action: z.literal(`databases.${action}`), databaseID: id })
      .strict(),
  ),
  ...(["delete", "purge"] as const).map((action) =>
    z
      .object({
        action: z.literal(`databases.${action}`),
        ...target,
        confirmName: name,
        planHash: z.string().regex(/^[a-f0-9]{64}$/),
      })
      .strict(),
  ),
  z
    .object({
      action: z.literal("databases.pools.register"),
      clusterID: id,
      node: z.string().regex(/^[a-z0-9][a-z0-9.-]{0,252}$/),
      poolID: z.uuid(),
      revision,
      idempotencyKey: key,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.installations.list"),
      clusterID: id,
    })
    .strict(),
  z
    .object({
      action: z.literal("databases.installations.create"),
      clusterID: id,
      node: z.string().regex(/^[a-z0-9][a-z0-9.-]{0,252}$/),
      poolID: z.uuid(),
      profile: z.literal("shared"),
      idempotencyKey: key,
    })
    .strict(),
] as const;
export const databaseCommandSchema = z.discriminatedUnion(
  "action",
  databaseCommands,
);
export const databaseReadActions = [
  "databases.backupPolicies.get",
  "databases.projects.list",
  "databases.migrations.list",
  "databases.migrationPurgePreview",
  "databases.permissions",
  "databases.list",
  "databases.get",
  "databases.plans.list",
  "databases.installations.list",
  "databases.deletionPreview",
  "databases.purgePreview",
] as const;
export type DatabaseCommand = z.infer<typeof databaseCommandSchema>;
export type DatabaseView = z.infer<typeof databaseViewSchema>;
export type DatabasePlan = z.infer<typeof databasePlanSchema>;
export type DatabaseKind = z.infer<typeof databaseKindSchema>;
export type DatabaseState = z.infer<typeof databaseStateSchema>;

export type DatabaseCreate = Extract<
  DatabaseCommand,
  { action: "databases.create" }
>;
export type DatabaseAction = Extract<
  DatabaseCommand,
  {
    action:
      | "databases.pause"
      | "databases.resume"
      | "databases.restart"
      | "databases.backup"
      | "databases.rotateCredentials";
  }
>;
export type DatabaseCopy = Extract<
  DatabaseCommand,
  { action: "databases.restore" | "databases.clone" }
>;
export type DatabaseResize = Extract<
  DatabaseCommand,
  { action: "databases.resize" }
>;
export type DatabaseOrderAccept = Extract<
  DatabaseCommand,
  { action: "databases.acceptOrder" }
>;


export function databasePlanAvailableTo(
  plan: DatabasePlan,
  customerID: string,
  availability: string,
): boolean {
  return (
    !plan.sponsoredCustomerID ||
    (availability === "pilot" && plan.sponsoredCustomerID === customerID)
  );
}
