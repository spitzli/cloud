import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
  timingSafeEqual,
} from "node:crypto";
import { z } from "zod";
import { parsePluginManifest } from "@webdock/plugin-sdk/contracts";
import type { PluginImport } from "@webdock/api-client/plugin-sources";

export const sourceLimit = 32 * 1024 * 1024;
export const chunkLimit = 65536;
export type SourceRow = Record<string, any>;
export class PluginSourceError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}
export function sourceFail(code: string, message: string, status = 409): never {
  throw new PluginSourceError(status, code, message);
}
const id = z.string().regex(/^[1-9][0-9]{0,19}$/);
const digest = z.string().regex(/^[a-f0-9]{64}$/);
const ref = z
  .string()
  .regex(/^[A-Za-z0-9_][A-Za-z0-9_./-]{0,254}$/)
  .refine(
    (v) =>
      !v.includes("..") &&
      !v.endsWith("/") &&
      !v.endsWith(".") &&
      !v.split("/").some((p) => !p || p.startsWith(".") || p.endsWith(".lock")),
  )
  .default("HEAD");
const gitURL = z
  .string()
  .max(2048)
  .refine((raw) => {
    try {
      const u = new URL(raw);
      return (
        u.protocol === "https:" &&
        !u.username &&
        !u.password &&
        (!u.port || u.port === "443") &&
        !/[?#%\\\s]/.test(raw) &&
        u.pathname.length > 1 &&
        !u.pathname
          .split("/")
          .slice(1)
          .some((p) => p === "." || p === "..")
      );
    } catch {
      return false;
    }
  });
const source = z.discriminatedUnion("kind", [
  z.strictObject({
    kind: z.literal("zip"),
    name: z
      .string()
      .min(1)
      .max(200)
      .regex(/^[^\x00-\x1f\x7f/\\]+\.zip$/i),
    size: z.number().int().positive().max(sourceLimit),
  }),
  z.strictObject({
    kind: z.literal("git"),
    url: gitURL,
    ref,
    credential: z
      .strictObject({
        username: z
          .string()
          .min(1)
          .max(256)
          .regex(/^[^:\x00-\x1f\x7f]+$/),
        token: z
          .string()
          .min(1)
          .max(4096)
          .regex(/^[^\x00-\x1f\x7f]+$/),
      })
      .optional(),
  }),
  z.strictObject({
    kind: z.literal("github"),
    installation: z.number().int().positive().safe(),
    installationPage: z.number().int().min(1).max(10000),
    page: z.number().int().min(1).max(10000),
    repository: z.number().int().positive().safe(),
    ref,
  }),
]);
const commandSchema = z.discriminatedUnion("action", [
  z.strictObject({ action: z.literal("list"), projectID: id }),
  z.strictObject({
    action: z.literal("create"),
    projectID: id,
    source,
    mode: z.literal("build"),
    idempotencyKey: z.string().min(1).max(128),
  }),
  z.strictObject({
    action: z.literal("chunk"),
    id,
    offset: z.number().int().min(0).max(sourceLimit),
    data: z.string().min(1).max(87384),
  }),
  z.strictObject({ action: z.literal("submit"), id }),
  z.strictObject({ action: z.literal("discard"), id }),
  z.strictObject({
    action: z.literal("approve"),
    id,
    manifestSha256: digest,
    capabilities: z.array(z.string().max(100)).max(32),
    configuration: z.record(z.string(), z.unknown()),
  }),
  z.strictObject({ action: z.literal("activate"), id }),
]);
export function parseSourceCommand(input: unknown) {
  const parsed = commandSchema.safeParse(input);
  if (!parsed.success)
    sourceFail(
      "invalid_source_input",
      "Check source details. Upload a ZIP up to 32 MiB or use an HTTPS Git URL; only isolated builds are supported.",
      400,
    );
  return parsed.data;
}
export function decodeSourceChunk(data: string): Buffer {
  if (
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      data,
    ) ||
    !data ||
    data.length > 87384
  )
    sourceFail("invalid_chunk", "Use a base64 chunk of at most 64 KiB.", 400);
  const bytes = Buffer.from(data, "base64");
  if (
    !bytes.length ||
    bytes.length > chunkLimit ||
    bytes.toString("base64") !== data
  )
    sourceFail("invalid_chunk", "Invalid upload chunk.", 400);
  return bytes;
}
export function installerConfigured(env: NodeJS.ProcessEnv = process.env) {
  return (
    !!env.WEBDOCK_PLUGIN_RUNTIME_FILE &&
    !!env.WEBDOCK_PLUGIN_INSTALLER_TOKEN &&
    env.WEBDOCK_PLUGIN_INSTALLER_TOKEN.length >= 32
  );
}
export function installerAuthorized(
  header: string | null,
  env: NodeJS.ProcessEnv = process.env,
) {
  if (!installerConfigured(env) || !header || header.length > 8192)
    return false;
  const actual = Buffer.from(header),
    expected = Buffer.from("Bearer " + env.WEBDOCK_PLUGIN_INSTALLER_TOKEN);
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}
function secretKey(token: string) {
  if (token.length < 32)
    sourceFail(
      "installer_unconfigured",
      "Configure the isolated plugin installer before importing sources.",
      503,
    );
  return createHash("sha256")
    .update("webdock-plugin-source-secrets-v1\0")
    .update(token)
    .digest();
}
export function sealSourceSecret(
  value: unknown,
  token: string,
  context: string,
): string {
  const nonce = randomBytes(12),
    cipher = createCipheriv("aes-256-gcm", secretKey(token), nonce);
  cipher.setAAD(Buffer.from(context));
  const bytes = Buffer.concat([
    cipher.update(JSON.stringify(value), "utf8"),
    cipher.final(),
  ]);
  return Buffer.concat([nonce, cipher.getAuthTag(), bytes]).toString("base64");
}
export function openSourceSecret(
  value: string,
  token: string,
  context: string,
): any {
  try {
    const bytes = Buffer.from(value, "base64"),
      cipher = createDecipheriv(
        "aes-256-gcm",
        secretKey(token),
        bytes.subarray(0, 12),
      );
    cipher.setAAD(Buffer.from(context));
    cipher.setAuthTag(bytes.subarray(12, 28));
    return JSON.parse(
      Buffer.concat([
        cipher.update(bytes.subarray(28)),
        cipher.final(),
      ]).toString("utf8"),
    );
  } catch {
    sourceFail(
      "source_secret_unavailable",
      "The source credential is unavailable. Create a new import.",
      503,
    );
  }
}
export const sourceHash = (value: string | Buffer) =>
  createHash("sha256").update(value).digest("hex");
export const sourceRequestHash = (value: unknown, token: string) =>
  createHmac("sha256", secretKey(token))
    .update(JSON.stringify(value))
    .digest("hex");
export function parseSourceManifest(raw: string) {
  if (Buffer.byteLength(raw) > 262144)
    sourceFail("invalid_manifest", "Plugin manifest exceeds 256 KiB.", 400);
  try {
    // JSON.parse otherwise silently replaces duplicate manifest properties.
    const stack: Array<{ object: boolean; keys: Set<string>; key: boolean }> =
      [];
    for (const token of raw.match(/"(?:\\.|[^"\\])*"|[{}\[\],:]/g) ?? []) {
      if (token === "{")
        stack.push({ object: true, keys: new Set(), key: true });
      else if (token === "[")
        stack.push({ object: false, keys: new Set(), key: false });
      else if (token === "}" || token === "]") stack.pop();
      else if (token === ",") {
        if (stack.at(-1)?.object) stack.at(-1)!.key = true;
      } else if (token.startsWith('"') && stack.at(-1)?.key) {
        const key = JSON.parse(token);
        if (stack.at(-1)!.keys.has(key)) throw Error();
        stack.at(-1)!.keys.add(key);
        stack.at(-1)!.key = false;
      }
    }
    const manifest = parsePluginManifest(JSON.parse(raw));
    if (manifest.image.endsWith("@sha256:" + "0".repeat(64))) throw Error();
    if (
      manifest.dependencies.length ||
      manifest.tables.length ||
      manifest.migrations.length ||
      manifest.jobs.length
    )
      sourceFail(
        "unsupported_lifecycle",
        "This installer supports action plugins with scoped documents. Remove jobs, tables, migrations and dependencies before importing.",
        400,
      );
    return manifest;
  } catch (error) {
    if (error instanceof PluginSourceError) throw error;
    sourceFail(
      "invalid_manifest",
      "Supply a valid plugin manifest with a published immutable image digest.",
      400,
    );
  }
}
export function approveSourceManifest(
  row: SourceRow,
  hash: string,
  capabilities: string[],
  configuration: Record<string, unknown>,
) {
  if (row.manifest_sha256 !== hash || sourceHash(row.manifest_raw) !== hash)
    sourceFail(
      "manifest_changed",
      "The reviewed manifest changed. Reload the import.",
    );
  const manifest = parseSourceManifest(row.manifest_raw);
  if (
    new Set(capabilities).size !== capabilities.length ||
    JSON.stringify([...capabilities].sort()) !==
      JSON.stringify([...manifest.capabilities].sort())
  )
    sourceFail(
      "capability_mismatch",
      "Approve exactly the permissions declared by this manifest.",
      400,
    );
  if (
    Buffer.byteLength(JSON.stringify(configuration)) > 65536 ||
    !z.fromJSONSchema(manifest.configuration as any).safeParse(configuration)
      .success
  )
    sourceFail(
      "invalid_configuration",
      "Configuration does not match the plugin schema.",
      400,
    );
  return manifest;
}
export const runtimeBindingSchema = z.strictObject({
  namespace: z.string().regex(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/),
  service: z.string().regex(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/),
  addresses: z
    .array(z.union([z.ipv4(), z.ipv6()]))
    .min(1)
    .max(16),
  credential: z.string().min(32).max(4096),
  ca: z.string().min(1).max(32768),
  uiOrigin: z
    .string()
    .url()
    .max(500)
    .refine((raw) => {
      try {
        const u = new URL(raw);
        return (
          u.protocol === "https:" &&
          !u.username &&
          !u.password &&
          u.pathname === "/" &&
          !u.search &&
          !u.hash
        );
      } catch {
        return false;
      }
    })
    .optional(),
  generation: z.literal(1),
  projects: z.strictObject({}),
});
export function requireSourceLease(row: SourceRow, token: string) {
  if (
    !["building", "deploying"].includes(row.status) ||
    row.claim_token !== token ||
    new Date(row.lease_until).getTime() <= Date.now() ||
    new Date(row.expires_at).getTime() <= Date.now()
  )
    sourceFail(
      "lease_lost",
      "Installer claim expired or no longer owns this import.",
    );
}
const errors: Record<string, string> = {
  discarded: "Import discarded.",
  expired: "This import expired. Create a new import.",
  lease_expired:
    "Installer lease expired. Inspect the existing build or deployment before creating another import.",
  build_failed:
    "The isolated build failed. Check the source and installer diagnostics.",
  deploy_failed:
    "Runtime deployment failed. Inspect the installer before retrying.",
  authority_revoked:
    "Operator or project access changed. Create a new authorized import.",
  invalid_manifest:
    "The build did not produce a valid immutable plugin manifest.",
  unsupported_lifecycle:
    "This installer supports action plugins with scoped documents. Jobs, tables, migrations and dependencies require the full lifecycle controller.",
};
export function sourceView(row: SourceRow): PluginImport {
  const manifest = row.manifest_raw
    ? parseSourceManifest(row.manifest_raw)
    : undefined;
  return {
    id: row.id,
    projectID: row.project_id,
    status: row.status,
    source: {
      kind: row.source_kind,
      label: row.source_label,
      ...(row.source_commit ? { commit: row.source_commit } : {}),
      ...(row.source_sha256 ? { sha256: row.source_sha256 } : {}),
    },
    mode: row.mode,
    uploadedBytes: Number(row.uploaded_bytes),
    expectedBytes: Number(row.expected_bytes),
    ...(manifest
      ? {
          manifest: {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            image: manifest.image,
            capabilities: manifest.capabilities,
            configuration: manifest.configuration,
          },
          manifestSha256: row.manifest_sha256,
        }
      : {}),
    ...(row.error_code
      ? {
          error:
            errors[row.error_code] ??
            "The installer could not complete this import. Inspect its state before retrying.",
        }
      : {}),
    createdAt: new Date(row.created_at).toISOString(),
  };
}
