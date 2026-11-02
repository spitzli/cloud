import { z } from "zod";
const pluginID = z.string().regex(/^[a-z][a-z0-9-]{0,63}$/);
const revision = z.string().regex(/^(0|[1-9][0-9]{0,18})$/);
export const availability = z.enum(["enabled", "draining", "disabled"]);
export type Availability = z.infer<typeof availability>;
export const installationSchema = z.object({
  id: pluginID,
  kind: z.enum(["builtin", "external"]),
  revision,
  desiredState: availability,
  state: availability,
  version: z.string(),
  imageDigest: z.string().nullable(),
  runtime: z.enum(["absent", "starting", "ready", "stopped", "failed"]),
  blockers: z
    .array(
      z.union([
        z.string(),
        z.object({
          code: z.string().optional(),
          message: z.string().optional(),
        }),
      ]),
    )
    .max(100),
  dependencies: z.array(z.object({ id: pluginID })).max(100),
  operationID: z.string().nullable().optional(),
  installRecovery: z.boolean().default(false),
  managed: z.boolean().default(false),
});
export type Installation = z.infer<typeof installationSchema>;
export const installedSchema = z.array(installationSchema).max(1000);
const stateSchema = z
  .object({
    action: z.literal("state"),
    pluginID,
    desired: availability,
    expectedRevision: revision,
    idempotencyKey: z.string().min(1).max(128),
  })
  .strict();
export type StateRequest = z.infer<typeof stateSchema>;
export const extensionReceipt = z.object({
  id: z.string().regex(/^[1-9][0-9]{0,18}$/),
  status: z.enum(["pending", "running", "succeeded", "failed", "blocked"]),
});
export const extensionOperation = extensionReceipt.extend({
  kind: z.enum(["state", "install", "update", "rollback"]).default("state"),
  pluginID,
  phase: z.string(),
  generation: revision,
  cursor: revision,
  condition: z.string().nullable(),
});
export function stateRequest(
  id: string,
  expectedRevision: string,
  desired: Availability,
  idempotencyKey: string,
): StateRequest {
  return stateSchema.parse({
    action: "state",
    pluginID: id,
    expectedRevision,
    desired,
    idempotencyKey,
  });
}
export function extensionPolling(
  status: string | undefined,
  failed: boolean,
): number | false {
  return !failed && (status === "pending" || status === "running")
    ? 2000
    : false;
}
export function blockerText(blocker: Installation["blockers"][number]): string {
  return typeof blocker === "string"
    ? blocker
    : (blocker.message ?? blocker.code ?? "Unrecognized blocker metadata");
}
