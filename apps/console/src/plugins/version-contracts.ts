import { z } from "zod";
const revision = z.string().regex(/^[1-9][0-9]{0,18}$/);
const schemaVersion = z.number().int().positive();
const readableSchema = z.object({ min: schemaVersion, max: schemaVersion });
const common = {
  reviewSha256: z.string().regex(/^[a-f0-9]{64}$/), expectedRevision: revision,
  approvedCapabilities: z.array(z.string().min(1).max(128)).max(64), idempotencyKey: z.string().min(1).max(128),
};
export const updateRequestSchema = z.strictObject({ ...common, version: z.string().min(1).max(40) });
export const rollbackRequestSchema = z.strictObject({ ...common, releaseGeneration: revision });
export type VersionCommand = { kind: "update"; body: z.infer<typeof updateRequestSchema> } | { kind: "rollback"; body: z.infer<typeof rollbackRequestSchema> };
export const releaseHistorySchema = z.object({
  pluginID: z.string(), currentGeneration: revision.nullable(), candidateGeneration: revision.nullable(), previousGeneration: revision.nullable(),
  schemaHighWater: schemaVersion.nullable(), targetID: z.string().nullable(),
  releases: z.array(z.object({
    generation: revision, version: z.string(), imageDigest: z.string(), manifestSha256: z.string().regex(/^[a-f0-9]{64}$/),
    status: z.string(), operationID: z.string().nullable(), schemaVersion, readableSchema,
    rollbackEligible: z.boolean(), rollbackBlocker: z.string().nullable(),
  })).max(1000),
});
export type ReleaseHistory = z.infer<typeof releaseHistorySchema>;
export function readsReservedSchema(release: { schemaVersion?: number | undefined; readableSchema?: { min: number; max: number } | undefined }, highWater: number | null): boolean {
  if (highWater === null || release.schemaVersion === undefined || !release.readableSchema) return false;
  const reserved = Math.max(highWater, release.schemaVersion);
  return release.readableSchema.min <= highWater && highWater <= release.readableSchema.max
    && release.readableSchema.min <= reserved && reserved <= release.readableSchema.max;
}
