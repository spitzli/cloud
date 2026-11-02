// Independent of platform bootstrap: adding S3 must not reseed accounts or databases.
import { randomBytes } from "node:crypto";
import { mkdirSync, writeFileSync, lstatSync } from "node:fs";
import { fileURLToPath } from "node:url";

const directory = fileURLToPath(new URL("../../.dev-local/", import.meta.url));
mkdirSync(directory, { recursive: true, mode: 0o700 });
const path = directory + "minio.env";
try {
  writeFileSync(path, `MINIO_ROOT_USER=webdock-${randomBytes(10).toString("hex")}\nMINIO_ROOT_PASSWORD=${randomBytes(32).toString("hex")}\n`, { mode: 0o600, flag: "wx" });
} catch (error) {
  if (error.code !== "EEXIST") throw error;
}
const stat = lstatSync(path);
if (!stat.isFile() || stat.nlink !== 1 || stat.uid !== process.getuid() || (stat.mode & 0o077) !== 0) {
  throw new Error("MinIO credentials must be an owned private regular file: .dev-local/minio.env");
}
