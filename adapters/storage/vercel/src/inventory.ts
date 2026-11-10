import { list } from "@vercel/blob";
import { StorageUsageError } from "@spitzli/module-storage/server/errors";
import { normalizeStoragePrefix } from "@spitzli/module-storage/server/validation";
import type { StorageDependencies } from "@spitzli/module-storage/server/ports";
const fail = (message: string): never => {
  throw new StorageUsageError(message);
};
const measurementError =
  "Storage inventory is unavailable or incomplete. Previous values are stale; retry the measurement.";
export function validateStoreToken(storeID: string, token: string) {
  // Blob SDK 2.3.1 extracts the store identifier from the fourth underscore segment.
  const match = /^vercel_blob_rw_([a-zA-Z0-9]+)_([a-zA-Z0-9]+)$/.exec(token);
  if (
    !/^store_[a-zA-Z0-9]+$/.test(storeID) ||
    token.length > 512 ||
    !match ||
    `store_${match[1]}`.toLowerCase() !== storeID.toLowerCase()
  )
    fail("Provide a Blob store ID and its matching read/write token.");
  return match![1].toLowerCase();
}
/** Inventory metadata only. A complete bounded traversal is required before replacing a snapshot. */
export async function measureStorage(
  storeID: string,
  token: string,
  prefix: string,
  dependencies: StorageDependencies = {},
) {
  prefix = normalizeStoragePrefix(prefix);
  const store = validateStoreToken(storeID, token);
  const controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      controller.abort();
      reject(new StorageUsageError(measurementError));
    }, 30_000);
  });
  try {
    return await Promise.race([
      timeout,
      (async () => {
        let bytes = 0,
          objects = 0,
          metadataBytes = 0,
          cursor: string | undefined;
        const cursors = new Set<string>(),
          paths = new Set<string>();
        for (let page = 0; page < 100; page++) {
          controller.signal.throwIfAborted();
          const result = await (dependencies.list || list)({
            token,
            prefix,
            cursor,
            limit: 1000,
            mode: "expanded",
            abortSignal: controller.signal,
          });
          controller.signal.throwIfAborted();
          if (
            !Array.isArray(result.blobs) ||
            result.blobs.length > 1000 ||
            typeof result.hasMore !== "boolean"
          )
            fail(measurementError);
          metadataBytes += Buffer.byteLength(JSON.stringify(result));
          if (metadataBytes > 32 * 1024 * 1024) fail(measurementError);
          for (const blob of result.blobs) {
            const url = new URL(blob.url);
            if (
              typeof blob.pathname !== "string" ||
              !blob.pathname.startsWith(prefix) ||
              paths.has(blob.pathname) ||
              !Number.isSafeInteger(blob.size) ||
              blob.size < 0 ||
              url.protocol !== "https:" ||
              url.host !== `${store}.public.blob.vercel-storage.com`
            )
              fail(measurementError);
            paths.add(blob.pathname);
            bytes += blob.size;
            objects++;
            if (!Number.isSafeInteger(bytes)) fail(measurementError);
          }
          if (!result.hasMore) return { bytes, objects };
          if (
            !result.cursor ||
            result.cursor.length > 4096 ||
            cursors.has(result.cursor)
          )
            fail(measurementError);
          cursors.add(result.cursor!);
          cursor = result.cursor;
        }
        return fail(measurementError);
      })(),
    ]);
  } catch {
    return fail(measurementError);
  } finally {
    clearTimeout(timer);
    controller.abort();
  }
}
