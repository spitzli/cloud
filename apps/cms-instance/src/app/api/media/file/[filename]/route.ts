import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import { getCMS } from "../../../../../lib/cms.ts";
import { MEDIA_BYTES } from "@webdock/cms-bridge/types";
import { createLocalReq } from "payload";
import { loadCMSConfig } from "../../../../../lib/config.ts";
import {
  checkedMediaBytes,
  checkedMediaResponse,
} from "../../../../../lib/media-storage.ts";
export const dynamic = "force-dynamic";
export async function GET(
  _request: Request,
  context: { params: Promise<{ filename: string }> },
) {
  const { filename } = await context.params;
  if (!/^[a-f0-9-]{36}\.(png|jpg|webp|avif)$/.test(filename))
    return new Response(null, { status: 404 });
  try {
    const cms = await getCMS(),
      config = loadCMSConfig();
    const result = await cms.find({
      collection: "media",
      where: { filename: { equals: filename } },
      limit: 1,
      depth: 0,
      overrideAccess: false,
      user: null,
      showHiddenFields: true,
    });
    if (!result.docs.length) return new Response(null, { status: 404 });
    const doc = result.docs[0],
      expected = doc.filesize ?? 0;
    let bytes: Buffer;
    if (config.mediaStorage.kind === "s3") {
      if (!doc.contentSha256 || doc.prefix || doc._objectKey)
        throw Error("Invalid media identity");
      const signal = AbortSignal.any([
        _request.signal,
        AbortSignal.timeout(30000),
      ]);
      const req = await createLocalReq(
        { req: { headers: new Headers(), signal } },
        cms,
      );
      let response: Response | undefined;
      for (const handler of cms.collections.media.config.upload.handlers ??
        []) {
        const candidate = await handler(req, {
          doc,
          params: { collection: "media", filename },
          headers: new Headers(),
        });
        if (candidate) {
          response = candidate;
          break;
        }
      }
      if (!response) throw Error("Media storage handler unavailable");
      bytes = await checkedMediaResponse(response, expected, doc.contentSha256);
    } else {
      const file = path.join(config.mediaDirectory, filename),
        info = await stat(file);
      if (!info.isFile() || info.size > MEDIA_BYTES || info.size !== expected)
        throw Error("Invalid media file");
      bytes = checkedMediaBytes(
        await readFile(file),
        expected,
        doc.contentSha256 ?? null,
      );
    }
    const mime: Record<string, string> = {
      png: "image/png",
      jpg: "image/jpeg",
      webp: "image/webp",
      avif: "image/avif",
    };
    return new Response(new Uint8Array(bytes), {
      headers: {
        "content-type": mime[filename.split(".").at(-1)!],
        "content-length": String(bytes.length),
        "cache-control": "public,max-age=31536000,immutable",
        "x-content-type-options": "nosniff",
        "content-security-policy": "default-src 'none'",
      },
    });
  } catch {
    return new Response(null, { status: 404 });
  }
}
