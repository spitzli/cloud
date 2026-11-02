import { z } from "zod";
import { useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { SessionContext, useApi } from "../context";
import { sessionCacheScope } from "../session-cache";
import { invokePlugin } from "./views";
import type { PluginContext, ResourceProps } from "./sdk";

export const MAX_BINARY_BYTES = 512 * 1024 * 1024;
export const STORAGE_CONTENT_TYPES = ["application/octet-stream", "application/pdf", "application/zip", "application/gzip", "text/plain", "text/csv"] as const;
export const storageHashSchema = z.string().regex(/^[a-f0-9]{64}$/);
export const storageFilenameSchema = z.string().min(1).max(160).regex(/^[a-zA-Z0-9._ -]+$/).refine(value => value.trim().length > 0 && value !== "." && value !== "..");
export const storageKeySchema = z.string().min(1).max(512).regex(/^[a-zA-Z0-9/._-]+$/).refine(value => value.split("/").every(part => part !== "" && part !== "." && part !== ".."));
export const transferDescriptorSchema = z.object({
  version: z.literal(1), id: z.string().regex(/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/),
  path: z.string(), method: z.enum(["GET", "PUT"]), expiresAt: z.number().int().positive().max(Number.MAX_SAFE_INTEGER),
  exactBytes: z.number().int().min(0).max(MAX_BINARY_BYTES), expectedSha256: storageHashSchema.optional(),
  attachment: z.object({ filename: storageFilenameSchema, contentType: z.enum(STORAGE_CONTENT_TYPES) }).strict(),
}).strict();
export type TransferDescriptor = z.infer<typeof transferDescriptorSchema>;
export interface BinaryTransferOptions { signal: AbortSignal; isCurrent: () => boolean }
export class UploadUncertainError extends Error {
  constructor() { super("Upload outcome is uncertain. Inspect the object before starting a new upload. This upload will not be retried."); }
}
export function checkedTransfer(input: TransferDescriptor, method: "GET" | "PUT"): TransferDescriptor {
  const parsed = transferDescriptorSchema.safeParse(input); if (!parsed.success) throw new Error("Invalid storage transfer response.");
  const descriptor = parsed.data, seconds = descriptor.expiresAt - Math.floor(Date.now() / 1000);
  if (descriptor.method !== method || descriptor.path !== `/api/console/plugins/storage/${descriptor.id}` || seconds <= 0 || seconds > 300 || method === "PUT" && !descriptor.expectedSha256)
    throw new Error("Invalid or expired storage transfer.");
  return descriptor;
}
export function binaryGuard(options: BinaryTransferOptions, expiresAt = Date.now() + 300000) {
  const controller = new AbortController();
  const abort = () => { controller.abort(); };
  const assertCurrent = () => {
    if (options.signal.aborted || controller.signal.aborted || !options.isCurrent() || Date.now() >= expiresAt) {
      controller.abort(); throw new DOMException("Storage transfer cancelled or expired.", "AbortError");
    }
  };
  options.signal.addEventListener("abort", abort, { once: true });
  const timer = setTimeout(abort, Math.max(0, expiresAt - Date.now()));
  const poll = setInterval(() => { if (!options.isCurrent()) abort(); }, 100);
  return {
    signal: controller.signal, check: assertCurrent,
    async wait<T>(promise: Promise<T>): Promise<T> {
      assertCurrent(); let stop: (() => void) | undefined;
      const cancelled = new Promise<never>((_resolve, reject) => { stop = () => { reject(new DOMException("Storage transfer cancelled.", "AbortError")); }; controller.signal.addEventListener("abort", stop, { once: true }); });
      try { const result = await Promise.race([promise, cancelled]); assertCurrent(); return result; }
      finally { if (stop) controller.signal.removeEventListener("abort", stop); }
    },
    dispose() { controller.abort(); clearTimeout(timer); clearInterval(poll); options.signal.removeEventListener("abort", abort); },
  };
}
export async function hashStorageBlob(blob: Blob, options: BinaryTransferOptions): Promise<string> {
  if (!Number.isSafeInteger(blob.size) || blob.size < 0 || blob.size > MAX_BINARY_BYTES) throw new Error("Files must be at most 512 MiB.");
  const guard = binaryGuard(options);
  try {
    guard.check();
    const bytes = await guard.wait(blob.arrayBuffer());
    const digest = await guard.wait(crypto.subtle.digest("SHA-256", bytes));
    return [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("");
  } finally { guard.dispose(); }
}
export const objectMetadataSchema = z.object({
  key: storageKeySchema, exact_bytes: z.number().int().min(0).max(MAX_BINARY_BYTES),
  version_id: z.string().min(1).max(512).regex(/^[a-zA-Z0-9/+=._-]+$/), if_match: z.string().regex(/^"[a-fA-F0-9-]{1,128}"$/),
  filename: storageFilenameSchema, content_type: z.enum(STORAGE_CONTENT_TYPES), expected_sha256: storageHashSchema.optional(),
}).strict();
export type ObjectMetadata = z.infer<typeof objectMetadataSchema>;

/** A changed authenticated principal, API, project or resource retires every old continuation. */
export function usePluginScope(context: PluginContext & { resource?: { id: string } }, extra = "") {
  const api = useApi(), session = useContext(SessionContext);
  const key = JSON.stringify([api.config.apiOrigin, sessionCacheScope(session), context.plugin, context.projectID, context.resource?.id, extra]);
  const scope = useMemo(() => ({ api, key }), [api, key]);
  const latest = useRef<typeof scope | null>(null);
  useLayoutEffect(() => { latest.current = scope; return () => { if (latest.current === scope) latest.current = null; }; }, [scope]);
  return { api, key, scope, isCurrent: () => latest.current === scope && session !== null && Date.parse(session.session.expiresAt) > Date.now() };
}
type BinaryStatus = "idle" | "hashing" | "planning" | "uploading" | "observing" | "downloading" | "ready" | "uploaded" | "uncertain" | "error";
export function usePluginStorage(context: ResourceProps) {
  const boundary = usePluginScope(context, JSON.stringify([context.writable, context.features, context.actions]));
  interface State { scope: typeof boundary.scope; status: BinaryStatus; error?: string; observation?: ObjectMetadata; url?: string; filename?: string; hash?: string }
  const empty = (): State => ({ scope: boundary.scope, status: "idle" });
  const [state, setState] = useState<State>(empty);
  const active = useRef<{ scope: typeof boundary.scope; controller: AbortController; upload: boolean } | null>(null);
  const objectURL = useRef<{ scope: typeof boundary.scope; url: string } | null>(null);
  const uploaded = useRef<{ scope: typeof boundary.scope; key: string; hash: string; etag: string | null | undefined } | null>(null);
  if (state.scope !== boundary.scope) setState(empty());
  const revokeURL = () => { if (objectURL.current) URL.revokeObjectURL(objectURL.current.url); objectURL.current = null; };
  useEffect(() => () => {
    if (active.current?.scope === boundary.scope) { active.current.controller.abort(); active.current = null; }
    if (objectURL.current?.scope === boundary.scope) { URL.revokeObjectURL(objectURL.current.url); objectURL.current = null; }
    if (uploaded.current?.scope === boundary.scope) uploaded.current = null;
  }, [boundary.scope]);
  const reset = (keepObservation = false) => {
    const uncertain = active.current?.upload === true;
    active.current?.controller.abort(); active.current = null; revokeURL();
    setState({ scope: boundary.scope, status: uncertain ? "uncertain" : "idle", ...(keepObservation && state.scope === boundary.scope && state.observation ? { observation: state.observation } : {}), ...(uncertain ? { error: new UploadUncertainError().message } : {}) });
  };
  const start = (action: string, write: boolean) => {
    // Discovery's feature list is empty for project readers; indexed-resource read actions
    // still go through Core's current feature/grant checks. Writes need the write feature.
    if (!boundary.isCurrent() || !context.actions?.includes(action) || write && (!context.writable || !context.features.includes("manage-buckets"))) throw new Error("Storage access is unavailable.");
    active.current?.controller.abort(); revokeURL();
    const request = { scope: boundary.scope, controller: new AbortController(), upload: false }; active.current = request;
    const options = { signal: request.controller.signal, isCurrent: () => boundary.isCurrent() && active.current === request };
    return { request, options };
  };
  const fail = (request: NonNullable<typeof active.current>, error: unknown) => {
    if (active.current !== request) return;
    active.current = null;
    if (!boundary.isCurrent()) { setState({ scope: request.scope, status: request.upload ? "uncertain" : "error", error: request.upload ? new UploadUncertainError().message : "Session or resource changed. Sign in or inspect again." }); return; }
    setState({ scope: boundary.scope, status: request.upload ? "uncertain" : "error", error: request.upload ? new UploadUncertainError().message : error instanceof z.ZodError ? "Check the object key, filename, type and optional SHA-256." : error instanceof Error ? error.message : "Storage request failed." });
  };
  const upload = async (file: File, key: string) => {
    let task: ReturnType<typeof start> | undefined;
    try {
      task = start("objects.upload", true); const { request, options } = task;
      storageKeySchema.parse(key); storageFilenameSchema.parse(file.name);
      const normalizedType = file.type.split(";")[0]?.trim().toLowerCase() ?? "";
      const contentType = z.enum(STORAGE_CONTENT_TYPES).parse(normalizedType.length > 0 ? normalizedType : "application/octet-stream");
      setState({ scope: boundary.scope, status: "hashing" }); const hash = await hashStorageBlob(file, options);
      if (!options.isCurrent() || options.signal.aborted) throw new DOMException("Storage transfer cancelled.", "AbortError");
      const input = Object.freeze({ resource_id: context.resource.id, key, exact_bytes: file.size, expected_sha256: hash, filename: file.name, content_type: contentType, expires_at: Math.floor(Date.now() / 1000) + 300 });
      setState({ scope: boundary.scope, status: "planning" });
      const descriptor = checkedTransfer(transferDescriptorSchema.parse(await invokePlugin(boundary.api, context.plugin, context.projectID, "objects.upload", input, options.signal, crypto.randomUUID())), "PUT");
      if (!options.isCurrent()) return;
      if (descriptor.exactBytes !== file.size || descriptor.expectedSha256 !== hash || descriptor.attachment.filename !== file.name || descriptor.attachment.contentType !== contentType) throw new Error("Upload descriptor differs from the selected file.");
      request.upload = true; setState({ scope: boundary.scope, status: "uploading" });
      const receipt = await boundary.api.uploadBinary(descriptor, file, options);
      if (!options.isCurrent()) return;
      uploaded.current = { scope: boundary.scope, key, hash, etag: receipt.etag };
      active.current = null; setState({ scope: boundary.scope, status: "uploaded", hash });
    } catch (error) { if (task) fail(task.request, error); else setState({ ...empty(), status: "error", error: "Storage access is unavailable." }); }
  };
  const observe = async (key: string) => {
    let task: ReturnType<typeof start> | undefined;
    try {
      task = start("objects.observe", false); const { options } = task; storageKeySchema.parse(key);
      setState({ scope: boundary.scope, status: "observing" });
      const observation = objectMetadataSchema.parse(await invokePlugin(boundary.api, context.plugin, context.projectID, "objects.observe", Object.freeze({ resource_id: context.resource.id, key }), options.signal, crypto.randomUUID()));
      if (!options.isCurrent()) return; if (observation.key !== key) throw new Error("Object identity changed.");
      if (!observation.expected_sha256 && uploaded.current?.scope === boundary.scope && uploaded.current.key === key && uploaded.current.etag === observation.if_match) observation.expected_sha256 = uploaded.current.hash;
      active.current = null; setState({ ...empty(), observation });
    } catch (error) { if (task) fail(task.request, error); }
  };
  const download = async (expectedHash?: string) => {
    let task: ReturnType<typeof start> | undefined;
    try {
      const observation = state.scope === boundary.scope ? state.observation : undefined; if (!observation) throw new Error("Inspect the object first.");
      const hash = expectedHash ?? observation.expected_sha256; if (hash) storageHashSchema.parse(hash);
      task = start("objects.download", false); const { options } = task;
      setState({ scope: boundary.scope, status: "planning", observation });
      const input = Object.freeze({ resource_id: context.resource.id, ...observation, ...(hash ? { expected_sha256: hash } : {}), expires_at: Math.floor(Date.now() / 1000) + 300 });
      const descriptor = checkedTransfer(transferDescriptorSchema.parse(await invokePlugin(boundary.api, context.plugin, context.projectID, "objects.download", input, options.signal, crypto.randomUUID())), "GET");
      if (!options.isCurrent()) return;
      if (descriptor.exactBytes !== observation.exact_bytes || descriptor.expectedSha256 !== hash || descriptor.attachment.filename !== observation.filename || descriptor.attachment.contentType !== observation.content_type) throw new Error("Download descriptor differs from the observed object.");
      setState({ scope: boundary.scope, status: "downloading", observation });
      const blob = await boundary.api.downloadBinary(descriptor, options); if (!options.isCurrent()) return;
      const url = URL.createObjectURL(blob); objectURL.current = { scope: boundary.scope, url }; active.current = null;
      setState({ scope: boundary.scope, status: "ready", observation, url, filename: descriptor.attachment.filename, ...(hash ? { hash } : {}) });
    } catch (error) { if (task) fail(task.request, error); }
  };
  const visible = state.scope === boundary.scope ? state : empty();
  return { ...visible, busy: ["hashing", "planning", "uploading", "observing", "downloading"].includes(visible.status), upload, observe, download, reset };
}
