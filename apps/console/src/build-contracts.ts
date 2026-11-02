import { z } from "zod";
import {msgid} from "@webdock/i18n";

export const buildID = z.string().regex(/^[1-9][0-9]{0,18}$/).refine((value) => /^[1-9][0-9]{0,18}$/.test(value) && BigInt(value) <= 9223372036854775807n);
export const logCursor = z.union([z.literal("0"), buildID]);
const revision = z.number().int().nonnegative().max(2147483646);
const date = z.iso.datetime({offset:true}).nullable();
const sha = z.string().regex(/^[a-f0-9]{40}$/);
const key = z.string().regex(/^[A-Za-z0-9_-]{16,128}$/);
const path = z.string().min(1).max(240).regex(/^[A-Za-z0-9_./-]+$/).refine((value) => value.split("/").every((part) => part !== "" && part !== "." && part !== ".."));
const directory = z.union([z.literal("."), path]);
export const strategySchema = z.enum(["auto", "dockerfile", "railpack"]);
export const containerSettingsSchema = z.object({
  strategy: strategySchema,
  contextDirectory: directory,
  dockerfile: path,
  buildArgs: z.record(z.string().regex(/^[A-Za-z_][A-Za-z0-9_]{0,127}$/), z.string().max(4096)).refine((value) => Object.keys(value).length <= 32 && JSON.stringify(value).length <= 16000),
}).strict();
const defaultSettings = { strategy: "auto" as const, contextDirectory: ".", dockerfile: "Dockerfile", buildArgs: {} };
export const sourceSchema = z.object({
  id: buildID, projectID: buildID, targetID: buildID, connectionID: buildID, repositoryID: buildID,
  repositoryName: z.string().max(300).nullable().optional(), branch: z.string().min(1).max(200),
  rootDirectory: directory, recipe: z.enum(["dockerfile", "dockerfile-frontend", "vercel"]),
  revision, enabled: z.boolean(), autoPublish: z.boolean(),
  executionBackend: z.enum(["microvm", "kubernetes"]).default("microvm"), buildProvider: z.enum(["isolated","github-actions"]),
  healthPath: z.string().max(160).default("/"), workflowPath: z.string().max(300).default(".github/workflows/webdock.yml"),
  artifactPrefix: z.string().max(128).default("webdock"), containerBuild: containerSettingsSchema.default(defaultSettings),
  webhookEnabled:z.boolean().default(false), webhookTrigger:z.enum(["push","workflow_run"]).default("push"),
});
export type BuildSource = z.infer<typeof sourceSchema>;
export const sourcesSchema = z.object({ docs: z.array(sourceSchema).max(100) });
export const gitConnectionsSchema=z.object({docs:z.array(z.object({id:buildID,customerID:buildID,accountLogin:z.string().max(200),state:z.string().max(32)})).max(100)});
export const gitRepositoriesSchema=z.object({docs:z.array(z.object({repositoryID:buildID,fullName:z.string().max(300).nullish(),defaultBranch:z.string().min(1).max(200).nullish()})).max(1000)});
export const webhookReceiptSchema=z.object({sourceID:buildID,revision,webhookEnabled:z.boolean()});
export type WebhookReceipt=z.infer<typeof webhookReceiptSchema>;
export const sourceGetSchema = z.object({
  source: sourceSchema.nullable(), targets: z.array(z.object({ id: buildID, name: z.string().max(200) })).max(100),
  blockers: z.array(z.string().max(500)).max(20).default([]),
  canBuild: z.boolean().default(false), canConfigure: z.boolean().default(false), canApprove: z.boolean().default(false),
});
export type BuildSourceInfo=z.infer<typeof sourceGetSchema>;
export const buildStatusSchema = z.enum(["queued", "running", "artifact-ready", "succeeded", "failed", "cancelled"]);
export type BuildStatus = z.infer<typeof buildStatusSchema>;
export const buildSchema = z.object({
  id: buildID, projectID: buildID, sourceID: buildID, targetID: buildID, sourceSHA: sha,
  status: buildStatusSchema, revision, generation: revision, createdAt: date, finishedAt: date,
  logs: z.string().max(262144).default(""), failureCode: z.string().max(80).nullable(),
  executionBackend: z.enum(["microvm", "kubernetes"]).default("microvm"),
  phase: z.string().max(64).optional(), registryVerifiedAt: date.optional(),
  retryCount: z.number().int().min(0).max(2).default(0), nextEligibleAt: date.optional(),
  lastInfrastructureFailure: z.enum(["KUBERNETES_API_UNAVAILABLE", "KUBERNETES_NODE_UNAVAILABLE"]).nullable().optional(),
});
export type Build = z.infer<typeof buildSchema>;
export const releaseSchema = z.object({
  id: buildID, projectID: buildID, sourceID: buildID, targetID: buildID, artifactID: buildID, buildID:buildID.nullable(),
  status: z.enum(["awaiting-approval", "queued", "deploying", "ready", "failed", "superseded", "needs-reconciliation"]),
  revision, approvedBy: z.string().max(200).nullable(), operationID: buildID.nullable(), createdAt: date,
});
export type BuildRelease = z.infer<typeof releaseSchema>;
const paging = { page: z.number().int().positive(), limit: z.number().int().min(1).max(100), totalPages: z.number().int().nonnegative() };
export const buildsPageSchema = z.object({ ...paging, docs: z.array(buildSchema).max(100) });
export const releasesPageSchema = z.object({ ...paging, docs: z.array(releaseSchema).max(100) });
export const logPageSchema = z.object({
  entries: z.array(z.object({ cursor: buildID, generation: revision, phase: z.string().max(64), text: z.string().max(8192), createdAt: date })).max(100),
  cursor: logCursor, status: buildStatusSchema, phase: z.string().max(64), terminal: z.boolean(),
}).superRefine((value, context) => {
  let prior = 0n;
  for (const entry of value.entries) {
    if (BigInt(entry.cursor) <= prior || BigInt(entry.cursor) > BigInt(value.cursor)) context.addIssue({ code: "custom", message: "Invalid log cursor order" });
    prior = BigInt(entry.cursor);
  }
});
export type LogPage = z.infer<typeof logPageSchema>;
export interface LogWindow { cursor: string; entries: LogPage["entries"]; truncated: boolean }
export function emptyLogWindow(): LogWindow { return { cursor: "0", entries: [], truncated: false }; }
export function mergeLogPage(current: LogWindow, input: LogPage): LogWindow {
  const page = logPageSchema.parse(input);
  if (BigInt(page.cursor) < BigInt(current.cursor)) throw new Error(msgid("The log cursor moved backwards."));
  const entries = [...current.entries, ...page.entries.filter((entry) => BigInt(entry.cursor) > BigInt(current.cursor))];
  let size = entries.reduce((sum, entry) => sum + entry.text.length, 0);
  let removed = 0;
  while (entries.length - removed > 200 || size > 65536) { size -= entries[removed]?.text.length ?? 0; removed++; }
  return { cursor: page.cursor, entries: entries.slice(removed), truncated: current.truncated || removed > 0 };
}

const project = { projectID: buildID };
const pageInput = { ...project, sourceID: buildID.optional(), page: z.number().int().min(1), limit: z.number().int().min(1).max(100) };
const branchInput=z.string().min(1).max(200).regex(/^[A-Za-z0-9_][A-Za-z0-9_./-]*$/).refine(value=>!value.includes("..") && !value.includes("//") && !value.endsWith("/") && !value.endsWith(".lock"));
const configureCommandSchema = z.object({
  action: z.literal("git.source.configure"), ...project,
  targetID: buildID, connectionID: buildID, repositoryID: buildID, branch: branchInput, rootDirectory: directory,
  recipe: z.enum(["dockerfile", "dockerfile-frontend"]), revision, enabled: z.boolean(), autoPublish: z.boolean(),
  healthPath: z.string().max(160), workflowPath: z.string().max(300), artifactPrefix: z.string().max(128), webhookTrigger:z.enum(["push","workflow_run"]).optional(),
  executionBackend: z.enum(["microvm", "kubernetes"]), containerBuild: containerSettingsSchema.optional(),
}).strict().refine((value) => value.containerBuild === undefined || (value.executionBackend === "kubernetes" && value.recipe === "dockerfile")).refine((value)=>value.webhookTrigger!=="workflow_run" || value.executionBackend==="kubernetes");
export const buildCommandSchema = z.discriminatedUnion("action", [
  z.object({action:z.literal("git.connections.list"),customerID:buildID}).strict(),
  z.object({action:z.literal("git.repositories.list"),customerID:buildID,connectionID:buildID}).strict(),
  z.object({ action: z.literal("git.sources.list"), ...project }).strict(),
  z.object({ action: z.literal("git.source.get"), ...project, targetID: buildID.optional() }).strict(),
  configureCommandSchema,
  z.object({action:z.literal("git.source.webhook.configure"),...project,sourceID:buildID,revision,enabled:z.boolean()}).strict(),
  z.object({ action: z.literal("git.builds.list"), ...pageInput }).strict(),
  z.object({ action: z.literal("git.releases.list"), ...pageInput }).strict(),
  z.object({ action: z.literal("git.builds.get"), ...project, buildID }).strict(),
  z.object({ action: z.literal("git.builds.cancel"), ...project, buildID }).strict(),
  z.object({ action: z.literal("git.builds.request"), ...project, targetID: buildID, idempotencyKey: key }).strict(),
  z.object({ action: z.literal("git.releases.approve"), ...project, releaseID: buildID, revision, idempotencyKey: key }).strict(),
  z.object({ action: z.literal("git.releases.rollback"), ...project, releaseID: buildID, revision, idempotencyKey: key }).strict(),
]);
export type BuildCommand = z.infer<typeof buildCommandSchema>;
export interface SourceSettings { executionBackend: "microvm" | "kubernetes"; strategy: "auto" | "dockerfile" | "railpack"; contextDirectory: string; dockerfile: string }
export function sourceSettingsCommand(source: BuildSource, input: SourceSettings) {
  return configureCommandSchema.parse({
    action: "git.source.configure", projectID: source.projectID, targetID: source.targetID, connectionID: source.connectionID, repositoryID: source.repositoryID,
    branch: source.branch, rootDirectory: source.rootDirectory, recipe: source.recipe, revision: source.revision, enabled: source.enabled, autoPublish: source.autoPublish,
    healthPath: source.healthPath, workflowPath: source.workflowPath, webhookTrigger:source.webhookTrigger, artifactPrefix: source.artifactPrefix, executionBackend: input.executionBackend,
    ...(input.executionBackend === "kubernetes" && source.recipe === "dockerfile" ? { containerBuild: { strategy: input.strategy, contextDirectory: input.contextDirectory, dockerfile: input.dockerfile, buildArgs: source.containerBuild.buildArgs } } : {}),
  });
}
export function canCancelBuild(status: BuildStatus, canWrite: boolean, preview: boolean): boolean { return canWrite && !preview && ["queued", "running", "artifact-ready"].includes(status); }
export function buildOutcome(status: BuildStatus): string {
  const labels: Record<BuildStatus, string> = { queued: msgid("Queued"), running: msgid("Building"), "artifact-ready": msgid("Awaiting publication"), succeeded: msgid("Build succeeded"), failed: msgid("Build failed"), cancelled: msgid("Cancelled") };
  return labels[status];
}
export function buildPolling(status: BuildStatus | undefined, failed: boolean): number | false { return !failed && (status === "queued" || status === "running" || status === "artifact-ready") ? 1000 : false; }
