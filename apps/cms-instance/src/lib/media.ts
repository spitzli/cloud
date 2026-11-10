import sharp from "sharp";
import { msgid } from "@spitzli/i18n";
import { randomUUID } from "node:crypto";
import { CMSInputError } from "@spitzli/cms-ui/schema";
import { MEDIA_BYTES } from "@spitzli/cms-bridge/types";
type FileInput = { data: Buffer; name: string; mimetype: string; size: number };
const formats: Record<string, { mime: string; extension: string }> = {
  jpeg: { mime: "image/jpeg", extension: "jpg" },
  png: { mime: "image/png", extension: "png" },
  webp: { mime: "image/webp", extension: "webp" },
  avif: { mime: "image/avif", extension: "avif" },
};
export async function validateRaster(file: FileInput): Promise<FileInput> {
  try {
    if (
      !Buffer.isBuffer(file.data) ||
      file.data.length !== file.size ||
      file.size < 1 ||
      file.size > MEDIA_BYTES
    )
      throw Error("size");
    const image = sharp(file.data, {
      limitInputPixels: 40_000_000,
      animated: false,
      failOn: "warning",
    });
    const metadata = await image.metadata();
    const format =
      metadata.format === "heif" && metadata.compression === "av1"
        ? formats.avif
        : metadata.format === "heif"
          ? undefined
          : metadata.format && formats[metadata.format];
    if (
      !format ||
      file.mimetype !== format.mime ||
      !metadata.width ||
      !metadata.height ||
      metadata.width * metadata.height > 40_000_000 ||
      (metadata.pages ?? 1) !== 1
    )
      throw Error("format");
    // Full decoding catches corrupt raster payloads that only have a valid header.
    await image.stats();
    return {
      ...file,
      name: `${randomUUID()}.${format.extension}`,
      mimetype: format.mime,
    };
  } catch {
    throw new CMSInputError(
      msgid(
        "Upload a valid PNG, JPEG, WebP or AVIF image (maximum 10 MiB and 40 megapixels).",
      ),
      400,
    );
  }
}
