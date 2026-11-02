import { createHash } from "node:crypto";
import pg from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { s3Storage } from "@payloadcms/storage-s3";
import type { StorageAdapter } from "payload";
import type { CMSConfig } from "./config.ts";
import { MEDIA_BYTES } from "@webdock/cms-bridge/types";

export interface MediaBinding {
  version: 1;
  instance: string;
  project: string;
  environment: string | null;
  kind: "local" | "s3";
  resource: string | null;
  endpoint: string | null;
  region: string | null;
  bucket: string | null;
  prefix: string | null;
  sha256: string;
}
function fingerprint(value: Omit<MediaBinding, "sha256">) {
  return createHash("sha256")
    .update(
      JSON.stringify([
        value.version,
        value.instance,
        value.project,
        value.environment,
        value.kind,
        value.resource,
        value.endpoint,
        value.region,
        value.bucket,
        value.prefix,
      ]),
    )
    .digest("hex");
}
export function mediaBinding(config: CMSConfig): MediaBinding {
  const storage = config.mediaStorage;
  const value = {
    version: 1 as const,
    instance: config.instance,
    project: config.project,
    environment: config.environment,
    kind: storage.kind,
    resource: storage.kind === "s3" ? storage.resource : null,
    endpoint: storage.kind === "s3" ? storage.endpoint : null,
    region: storage.kind === "s3" ? storage.region : null,
    bucket: storage.kind === "s3" ? storage.bucket : null,
    prefix: storage.kind === "s3" ? storage.prefix : null,
  };
  return { ...value, sha256: fingerprint(value) };
}
export function checkMediaBinding(
  stored: MediaBinding | null,
  mediaCount: number,
  wanted: MediaBinding,
) {
  if (!Number.isSafeInteger(mediaCount) || mediaCount < 0)
    throw Error("Invalid media inventory");
  if (!stored) {
    if (wanted.kind === "s3" && mediaCount > 0)
      throw Error("Existing media requires an explicit checked migration");
    return;
  }
  if (
    stored.version !== 1 ||
    stored.sha256 !== fingerprint(stored) ||
    stored.sha256 !== wanted.sha256
  )
    throw Error(
      "CMS media storage binding changed; an explicit migration is required",
    );
}
/** DML only. Migration initialization is explicit; runtime cannot adopt a new S3 destination. */
export async function verifyMediaBinding(
  config: CMSConfig,
  initialize = false,
) {
  const wanted = mediaBinding(config);
  if (initialize && !config.migrating)
    throw Error("Migration authority required");
  const client = new pg.Client({
    ...verifiedPostgresConfig(config.databaseURL, config.databaseCA),
    connectionTimeoutMillis: 5000,
    query_timeout: 10000,
  });
  try {
    await client.connect();
    await client.query("BEGIN");
    await client.query(
      "SELECT pg_advisory_xact_lock(hashtext('webdock-cms-media-storage-v1'))",
    );
    const existing = await client.query<{ binding: MediaBinding }>(
      "SELECT binding FROM app.cms_media_storage WHERE singleton=true FOR UPDATE",
    );
    const stored = existing.rows[0]?.binding ?? null;
    const count = await client.query<{ count: string }>(
      "SELECT count(*)::text AS count FROM app.media",
    );
    checkMediaBinding(stored, Number(count.rows[0]?.count), wanted);
    if (!stored) {
      if (wanted.kind === "s3" && !initialize)
        throw Error("Explicit new-instance media initialization is required");
      await client.query(
        "INSERT INTO app.cms_media_storage(singleton,binding) VALUES(true,$1::jsonb)",
        [JSON.stringify(wanted)],
      );
    }
    await client.query("COMMIT");
  } catch {
    await client.query("ROLLBACK").catch(() => {});
    throw Error(
      "CMS media storage requires explicit initialization or migration reconciliation",
    );
  } finally {
    await client.end().catch(() => {});
  }
}
export function mediaStorageAdapter(config: CMSConfig): StorageAdapter {
  const storage = config.mediaStorage;
  const enabled = storage.kind === "s3" && !config.migrating;
  const credentials = storage.kind === "s3" ? storage.credentials : null;
  const key = createHash("sha256")
    .update(JSON.stringify([mediaBinding(config).sha256, credentials]))
    .digest("hex");
  const adapter = s3Storage({
    enabled,
    bucket: storage.kind === "s3" ? storage.bucket : "disabled-local-media",
    clientCacheKey: `webdock-cms-${key}`,
    clientUploads: false,
    signedDownloads: false,
    useCompositePrefixes: true,
    config:
      storage.kind === "s3"
        ? {
            endpoint: storage.endpoint,
            region: storage.region,
            ...(credentials ? { credentials } : {}),
            forcePathStyle: true,
            requestChecksumCalculation: "WHEN_REQUIRED",
            responseChecksumValidation: "WHEN_REQUIRED",
            maxAttempts: 2,
            requestHandler: {
              connectionTimeout: 5000,
              requestTimeout: 15000,
              throwOnRequestTimeout: true,
              httpAgent: { maxSockets: 4 },
              httpsAgent: { maxSockets: 4, rejectUnauthorized: true },
            },
          }
        : {},
    collections: {
      media: {
        prefix: storage.kind === "s3" ? storage.prefix : "",
        generateFileURL: ({ filename }) =>
          `${config.origin}/api/media/file/${encodeURIComponent(filename)}`,
      },
    },
  });
  return {
    ...adapter,
    init: async (incoming) => {
      const configured = await adapter.init(incoming);
      for (const collection of configured.collections ?? []) {
        if (collection.slug !== "media") continue;
        for (const field of collection.fields) {
          if (
            field.type !== "text" ||
            !["prefix", "_objectKey"].includes(field.name)
          )
            continue;
          field.access = {
            ...field.access,
            create: () => false,
            update: () => false,
          };
        }
      }
      return configured;
    },
  };
}
export function checkedMediaBytes(
  bytes: Buffer,
  expected: number,
  sha256: string | null,
) {
  if (
    !Buffer.isBuffer(bytes) ||
    expected < 1 ||
    expected > MEDIA_BYTES ||
    bytes.length !== expected ||
    (sha256 !== null &&
      (!/^[a-f0-9]{64}$/.test(sha256) ||
        createHash("sha256").update(bytes).digest("hex") !== sha256))
  )
    throw Error("Media integrity mismatch");
  return bytes;
}
export async function checkedMediaResponse(
  response: Response,
  expected: number,
  sha256: string,
) {
  if (
    response.status !== 200 ||
    !response.body ||
    Number(response.headers.get("content-length")) !== expected ||
    expected < 1 ||
    expected > MEDIA_BYTES
  ) {
    await response.body?.cancel();
    throw Error("Media response mismatch");
  }
  const reader = response.body.getReader(),
    chunks: Buffer[] = [];
  let size = 0;
  try {
    for (;;) {
      const result = await reader.read();
      if (result.done) break;
      size += result.value.byteLength;
      if (size > expected) throw Error("Media response exceeded metadata");
      chunks.push(Buffer.from(result.value));
    }
    return checkedMediaBytes(Buffer.concat(chunks, size), expected, sha256);
  } finally {
    await reader.cancel().catch(() => {});
  }
}
