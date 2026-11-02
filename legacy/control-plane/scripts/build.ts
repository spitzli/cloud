import { cp, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
/** FSR imports modules at runtime: preserve the complete tree instead of a lossy bundle. */
export async function buildArtifact(destination = root + "dist") {
  await rm(destination, { recursive: true, force: true });
  await cp(root + "src", destination, { recursive: true });
}
if (import.meta.main) await buildArtifact();
