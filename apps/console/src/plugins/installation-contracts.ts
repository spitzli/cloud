import { z } from "zod";
const id = z.string().regex(/^[a-z][a-z0-9-]{0,63}$/);
const digest = z.string().regex(/^[a-f0-9]{64}$/);
export const catalogSchema = z.array(z.object({
  catalogID: id, pluginID: id, version: z.string().min(1).max(40), manifestSha256: digest,
  image: z.string().min(1).max(512), registryURL: z.url(), signerKeyID: z.string().min(1).max(128),
  capabilities: z.array(z.string().max(128)).max(64), supportedFeatures: z.array(z.string().max(64)).max(64),
  targets: z.array(z.object({ id: z.string().min(1).max(64), label: z.string().min(1).max(160), reviewSha256: digest })).max(100),
  configurationSchema: z.record(z.string(), z.unknown()),
  schemaVersion: z.number().int().positive().optional(),
  readableSchema: z.object({ min: z.number().int().positive(), max: z.number().int().positive() }).optional(),
})).max(1000);
export type CatalogRelease = z.infer<typeof catalogSchema>[number];
export const installRequestSchema = z.strictObject({
  catalogID: id, version: z.string().min(1).max(40), reviewSha256: digest,
  targetID: z.string().min(1).max(64), configuration: z.record(z.string(), z.unknown()),
  approvedCapabilities: z.array(z.string().max(128)).max(64), idempotencyKey: z.string().min(1).max(128),
});
export type InstallRequest = z.infer<typeof installRequestSchema>;
const phaseNames: Record<string, string> = {
  requested: "Installation requested", preparing_storage: "Preparing private storage",
  preparing_configuration: "Preparing runtime configuration", deploying: "Starting runtime",
  suspending_runtime: "Stopping previous runtime",
  verifying_runtime: "Verifying runtime identity", complete: "Installation complete",
  blocked: "Waiting for operator action", failed: "Installation did not complete",
};
export function installationPhase(phase: string) { return phaseNames[phase] ?? "Waiting for controller update"; }
