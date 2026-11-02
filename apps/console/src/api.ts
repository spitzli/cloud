import {readSse} from "./sse";
import { z } from "zod";
import { binaryGuard, checkedTransfer, hashStorageBlob, UploadUncertainError, type BinaryTransferOptions, type TransferDescriptor } from "./plugins/storage";
const origin = z.url().refine((value) => {
  const url = new URL(value);
  return (
    ["https:", "http:"].includes(url.protocol) &&
    !url.username &&
    !url.password &&
    url.pathname === "/" &&
    !url.search &&
    !url.hash
  );
}, "Expected an HTTP(S) origin");
export const runtimeConfigSchema = z
  .object({
    apiOrigin: origin,
    workspaceApiOrigin: origin.optional(),
    realtimeUrl: z.url().refine(value=>{const u=new URL(value);return u.protocol==="wss:"&&u.hostname.endsWith(".neon.tech")&&!u.username&&!u.password&&!u.port&&u.pathname==="/"&&!u.search&&!u.hash},"Expected an exact Neon realtime origin").optional(),
    authOrigin: origin,
    enabledFeatures: z.array(z.string().max(80)).max(64),
    databaseOrderingEnabled: z.boolean().optional(),
    browserFacade: z.boolean().optional(),
    consoleOrigin: origin.optional(),
    surface: z.enum(["account", "console"]).optional(),
    sso: z.boolean().optional(),
    mailGatewayOrigin: origin.refine((value) => value.startsWith("https://")).optional(),
    localePreference: z.enum(["system", "en", "de"]).optional(),
  })
  .strict();
export type RuntimeConfig = z.infer<typeof runtimeConfigSchema>;
export const sessionSchema = z.object({
  data: z
    .object({
      user: z.object({
        id: z.string(),
        name: z.string(),
        email: z.string(),
        role: z.string().nullish(),
        emailVerified: z.boolean(),
        twoFactorEnabled: z.boolean().optional(),
        mustChangePassword: z.boolean().optional(),
        preview: z.boolean().optional(),
        acting: z.boolean().optional(),
        customerScope: z.string().nullish(),
      }),
      session: z.object({
        id: z.string(),
        userId: z.string(),
        expiresAt: z.string(),
      }),
    })
    .nullable(),
});
export type Session = NonNullable<z.infer<typeof sessionSchema>["data"]>;
export const recordSchema = z.object({
  id: z.string().regex(/^[1-9][0-9]{0,18}$/),
  name: z.string().optional(),
  status: z.string().optional(),
  customer: z.string().optional(),
  customerType: z.string().nullish(),
  contactEmail: z.string().nullish(),
  firstName: z.string().nullish(),
  lastName: z.string().nullish(),
  companyName: z.string().nullish(),
  contactName: z.string().nullish(),
  addressLine1: z.string().nullish(),
  addressLine2: z.string().nullish(),
  postalCode: z.string().nullish(),
  city: z.string().nullish(),
  region: z.string().nullish(),
  country: z.string().nullish(),
  phone: z.string().nullish(),
  url: z.string().nullish(),
  repositoryURL: z.string().nullish(),
  notes: z.string().nullish(),
  summary: z.string().optional(),
  actor: z.string().optional(),
  changedFields: z.string().nullish(),
  action: z.string().optional(),
  targetCollection: z.string().optional(),
  targetID: z.string().optional(),
  createdAt: z.string().optional(),
  updatedAt: z.string().optional(),
});
export type RegistryRecord = z.infer<typeof recordSchema>;
export const listSchema = z.object({
  docs: z.array(recordSchema).max(50),
  page: z.number().int(),
  totalPages: z.number().int(),
  totalDocs: z.number().int(),
  hasNextPage: z.boolean(),
  hasPrevPage: z.boolean(),
});
export type Collection = "customers" | "projects" | "audit-events";
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export class ApiClient {
  constructor(
    public config: RuntimeConfig,
    private fetcher: typeof fetch = fetch,
  ) {}
  async uploadBinary(input: TransferDescriptor, file: Blob, options: BinaryTransferOptions): Promise<{ bytes: number; sha256: string; etag?: string | null | undefined }> {
    const descriptor = checkedTransfer(input, "PUT");
    if (file.size !== descriptor.exactBytes) throw new Error("Upload size differs from the selected file.");
    const guard = binaryGuard(options, descriptor.expiresAt * 1000);
    let attempted = false;
    try {
      const hash = await guard.wait(hashStorageBlob(file, { signal: guard.signal, isCurrent: options.isCurrent }));
      if (hash !== descriptor.expectedSha256) throw new Error("Upload SHA-256 differs from the selected file.");
      const url = this.binaryURL(descriptor), fetcher = this.fetcher;
      guard.check(); attempted = true;
      const response = await guard.wait(fetcher(url, { method: "PUT", body: file, signal: guard.signal, credentials: "include", cache: "no-store", redirect: "error", headers: { "Content-Type": descriptor.attachment.contentType, Accept: "application/json" } }));
      if (!response.ok) throw new ApiError(response.status, errorMessage(response.status, false));
      const bytes = await readBinaryResponse(response, 4096, guard, false);
      const receipt = z.object({ bytes: z.number().int().min(0), sha256: z.string(), etag: z.string().nullable().optional() }).strict().parse(JSON.parse(new TextDecoder().decode(bytes)));
      if (receipt.bytes !== descriptor.exactBytes || receipt.sha256 !== hash) throw new Error("Upload receipt differs from the selected file.");
      guard.check(); return receipt;
    } catch (error) { if (attempted) throw new UploadUncertainError(); throw error; }
    finally { guard.dispose(); }
  }
  async downloadBinary(input: TransferDescriptor, options: BinaryTransferOptions): Promise<Blob> {
    const descriptor = checkedTransfer(input, "GET"), guard = binaryGuard(options, descriptor.expiresAt * 1000);
    try {
      const fetcher = this.fetcher;
      guard.check();
      const response = await guard.wait(fetcher(this.binaryURL(descriptor), { method: "GET", signal: guard.signal, credentials: "include", cache: "no-store", redirect: "error", headers: { Accept: descriptor.attachment.contentType } }));
      if (!response.ok) throw new ApiError(response.status, errorMessage(response.status, false));
      const contentType = response.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase();
      if (contentType !== descriptor.attachment.contentType || ![null, "identity"].includes(response.headers.get("content-encoding"))) throw new Error("Unsupported binary response.");
      const bytes = await readBinaryResponse(response, descriptor.exactBytes, guard, true);
      const blob = new Blob([bytes], { type: descriptor.attachment.contentType });
      if (descriptor.expectedSha256 && await guard.wait(hashStorageBlob(blob, { signal: guard.signal, isCurrent: options.isCurrent })) !== descriptor.expectedSha256) throw new Error("Download SHA-256 verification failed.");
      guard.check(); return blob;
    } finally { guard.dispose(); }
  }
  private binaryURL(descriptor: TransferDescriptor): URL {
    const base = new URL(runtimeConfigSchema.parse(this.config).apiOrigin), url = new URL(descriptor.path, base);
    if (url.origin !== base.origin || url.pathname !== descriptor.path || url.search || url.hash) throw new Error("Invalid binary API path.");
    return url;
  }
  async stream<T>(path:string,schema:z.ZodType<T>,onEvent:(value:T,cursor:string)=>void|Promise<void>,options:{signal:AbortSignal;lastEventID?:string}):Promise<void> {
    if(!path.startsWith("/")||path.startsWith("//")||path.includes("\\"))throw new Error("Invalid API path");
    if(this.config.browserFacade)path=facadePath(path,false);
    const base=new URL(this.config.apiOrigin),url=new URL(path,base);
    if(url.origin!==base.origin)throw new Error("Invalid API path");
    const headers=new Headers({Accept:"text/event-stream"});
    if(options.lastEventID!==undefined){if(options.lastEventID.length>128)throw new Error("Invalid event cursor.");headers.set("Last-Event-ID",options.lastEventID);}
    const fetcher=this.fetcher;
    const response=await fetcher(url,{method:"GET",credentials:"include",cache:"no-store",redirect:"error",signal:options.signal,headers});
    if(!response.ok)throw new ApiError(response.status,errorMessage(response.status,false));
    if(response.headers.get("content-type")?.split(";")[0]?.trim()!=="text/event-stream"||!response.body)throw new Error("Unsupported build log stream.");
    await readSse(response.body,async event=>{
      if(event.event==="unavailable")throw new ApiError(403,"Build log access is no longer available.");
      if(event.event!=="logs")return;
      let value:T;try{value=schema.parse(JSON.parse(event.data));}catch{throw new Error("The service returned an unsupported log event.");}
      await onEvent(value,event.id);
    },options.signal);
  }
  async request<T>(
    path: string,
    schema: z.ZodType<T>,
    options: RequestInit = {},
    auth = false,
  ): Promise<T> {
    if (!path.startsWith("/") || path.startsWith("//") || path.includes("\\"))
      throw new Error("Invalid API path");
    if (this.config.browserFacade) path = facadePath(path, auth);
    const base = new URL(
      auth && !this.config.browserFacade
        ? this.config.authOrigin
        : this.config.apiOrigin,
    );
    const url = new URL(path, base);
    if (url.origin !== base.origin) throw new Error("Invalid API path");
    const fetcher = this.fetcher;
    const headers = new Headers(options.headers);
    if (!headers.has("Accept")) headers.set("Accept", "application/json");
    if (options.body && !headers.has("Content-Type"))
      headers.set("Content-Type", "application/json");
    const response = await fetcher(url, {
      ...options,
      credentials: "include",
      cache: "no-store",
      redirect: "error",
      headers,
    });
    if (!response.ok)
      throw new ApiError(response.status, response.status === 409 || (response.status === 400 && !auth) ? await serverMessage(response) : errorMessage(response.status, auth));
    const text = await response.text();
    if (text.length > 1048576)
      throw new Error("The response exceeds the supported size.");
    try {
      return schema.parse(JSON.parse(text));
    } catch {
      throw new Error("The service returned an unsupported response.");
    }
  }
  get<T>(path: string, schema: z.ZodType<T>, signal?: AbortSignal) {
    return this.request(path, schema, { signal: signal ?? null });
  }
  write<T>(
    path: string,
    method: "POST" | "PUT" | "PATCH",
    data: unknown,
    schema: z.ZodType<T>,
    auth = false,
  ) {
    return this.request(
      path,
      schema,
      { method, body: JSON.stringify(data) },
      auth,
    );
  }
}
async function readBinaryResponse(response: Response, maximum: number, guard: ReturnType<typeof binaryGuard>, exact: boolean): Promise<Uint8Array<ArrayBuffer>> {
  const length = response.headers.get("content-length");
  if (length !== null && (!/^(0|[1-9][0-9]*)$/.test(length) || !Number.isSafeInteger(Number(length)) || (exact ? Number(length) !== maximum : Number(length) > maximum))) throw new Error("Binary response size mismatch.");
  const reader = response.body?.getReader();
  if (!reader) { if (exact && maximum !== 0) throw new Error("Binary response is incomplete."); return new Uint8Array(0); }
  const chunks: Uint8Array[] = []; let size = 0;
  try {
    for (;;) {
      guard.check();
      const part = await guard.wait(reader.read()); if (part.done) break;
      size += part.value.byteLength; if (size > maximum) throw new Error("Binary response exceeds its declared size.");
      chunks.push(part.value);
    }
    if (exact && size !== maximum) throw new Error("Binary response is incomplete.");
    const bytes = new Uint8Array(size); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    guard.check(); return bytes;
  } finally { void reader.cancel().catch(() => undefined); reader.releaseLock(); }
}
export async function loadRuntimeConfig(): Promise<RuntimeConfig> {
  const response = await fetch("/console-config.json", {
    credentials: "same-origin",
    cache: "no-store",
    redirect: "error",
  });
  if (!response.ok)
    throw new Error(
      "Console configuration is unavailable. Contact the installation operator.",
    );
  return runtimeConfigSchema.parse(await response.json());
}

function facadePath(path: string, auth: boolean): string {
  if (path === "/v1/auth/session") return "/api/console/session";
  if (path.startsWith("/v1/operator/registry/"))
    return path.replace("/v1/operator/registry/", "/api/console/registry/");
  if (auth && path.startsWith("/api/auth/"))
    return path.replace("/api/auth/", "/api/console/auth/");
  return path;
}
/** Conflicts carry a fixed, human-written reason from the server (for example why a delete is refused). */
/** The service's own words for a conflict or invalid input, such as a plan limit. */
async function serverMessage(response: Response): Promise<string> {
  const body = z.object({ error: z.object({ message: z.string().min(1).max(300) }) }).safeParse(await response.json().catch(() => null));
  return body.success ? body.data.error.message : errorMessage(response.status, false);
}
function errorMessage(status: number, auth: boolean): string {
  switch (status) {
    case 401:
      return auth
        ? "Sign-in verification failed. Check your credentials and try again."
        : "Your session has expired. Sign in again.";
    case 403:
      return "You do not have access to this action.";
    case 409:
      return "This record changed or has linked resources. Refresh and review it before trying again.";
    case 404:
      return "This resource is unavailable.";
    case 400:
    case 422:
      return "Check the entered values and try again.";
    case 429:
      return "Too many requests. Please wait before trying again.";
    default:
      return "The service could not complete this request. Try again.";
  }
}

/** The preview marker also scopes writable owner acting; only the old mode is read-only. */
export function isReadOnlyPreview(session: Session): boolean {
  return session.user.preview === true && session.user.acting !== true;
}
