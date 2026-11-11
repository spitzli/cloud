import { build } from "esbuild";
import { copyFile, lstat, mkdir, readlink, symlink } from "node:fs/promises";
import path from "node:path";
import { nodeFileTrace } from "@vercel/nft";
await build({
  entryPoints: ["src/migrate.ts"],
  outfile: "dist/migrate.mjs",
  bundle: true,
  platform: "node",
  target: "node24",
  format: "esm",
  external: ["payload", "pg", "sharp", "@payloadcms/db-postgres"],
  logLevel: "info",
});
// Vinext bundles Payload for route handlers; the standalone migration's external
// imports need their own traced production closure, not the entire monorepo.
const root = path.resolve("../..");
const { fileList } = await nodeFileTrace(
  [path.resolve("dist/migrate.mjs")],
  { base: root, processCwd: root, mixedModules: true },
);
for (const file of fileList) {
  if (file === "apps/cms-instance/dist/migrate.mjs") continue;
  const source = path.join(root, file),
    destination = path.resolve("dist/standalone", file);
  await mkdir(path.dirname(destination), { recursive: true });
  if ((await lstat(source)).isSymbolicLink()) {
    try {
      await symlink(await readlink(source), destination);
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
    }
  } else await copyFile(source, destination);
}
console.log(`Traced ${fileList.size} migration runtime files.`);
