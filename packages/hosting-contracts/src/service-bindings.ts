import { z } from "zod";
import { resourceID, resourceRevision, idempotencyKey } from "./validation";
import { environmentNameSchema } from "./environment";
export const serviceSelectionSchema = z.discriminatedUnion("kind", [
  z
    .object({
      kind: z.literal("postgres"),
      databaseID: resourceID,
      variable: environmentNameSchema.default("DATABASE_URL"),
    })
    .strict(),
  z
    .object({
      kind: z.literal("mail"),
      connectionID: resourceID,
      domainID: resourceID,
      variable: environmentNameSchema.default("SPITZLI_MAIL_API_KEY"),
    })
    .strict(),
]);
export const serviceCommands = [
  z
    .object({
      action: z.literal("services.list"),
      projectID: resourceID,
      appID: resourceID.optional(),
    })
    .strict(),
  z
    .object({
      action: z.literal("services.bind"),
      projectID: resourceID,
      appID: resourceID,
      service: serviceSelectionSchema,
      revision: resourceRevision,
      idempotencyKey,
    })
    .strict(),
  z
    .object({
      action: z.literal("services.disconnect"),
      projectID: resourceID,
      bindingID: resourceID,
      revision: resourceRevision,
      idempotencyKey,
    })
    .strict(),
  z
    .object({
      action: z.literal("services.rotate"),
      projectID: resourceID,
      bindingID: resourceID,
      revision: resourceRevision,
      idempotencyKey,
    })
    .strict(),
  z
    .object({
      action: z.literal("services.reconcile"),
      projectID: resourceID,
      bindingID: resourceID,
      revision: resourceRevision,
      idempotencyKey,
    })
    .strict(),
] as const;
export const serviceCommandSchema = z.discriminatedUnion(
  "action",
  serviceCommands,
);
export type ServiceSelection = z.infer<typeof serviceSelectionSchema>;
export type ServiceBindingView = {
  id: string;
  projectID: string;
  appID: string;
  service: ServiceSelection;
  revision: number;
  observedRevision: number;
  credentialRevision: number | null;
  state: "pending" | "connected" | "blocked" | "disconnecting" | "disconnected";
  reason: string | null;
};
const postgresRuntimeSchema = z
  .object({
    id: resourceID,
    revision: resourceRevision,
    backendID: resourceID,
    caCertificate: z.string().min(1).max(16384),
  })
  .strict();
export const serviceRuntimeSchema = z.union([
  postgresRuntimeSchema,
  z
    .object({
      id: resourceID,
      revision: resourceRevision,
      kind: z.literal("mail"),
      egressCIDRs: z.array(z.string().max(64)).min(1).max(8),
    })
    .strict(),
]);
export type ServiceRuntime = z.infer<typeof serviceRuntimeSchema>;
