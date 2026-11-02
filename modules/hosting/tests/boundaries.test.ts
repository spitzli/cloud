import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
function files(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory()
      ? files(resolve(directory, entry.name))
      : entry.name.endsWith(".ts")
        ? [resolve(directory, entry.name)]
        : [],
  );
}
test("Hosting runtime imports neither application code nor concrete adapters or foreign module implementations", () => {
  for (const file of files(resolve(root, "modules/hosting/src/server"))) {
    const source = readFileSync(file, "utf8");
    assert.doesNotMatch(
      source,
      /(?:from\s+|import\s*\()["'][^"']*(?:apps\/|@webdock\/api|@webdock\/adapter-|@webdock\/module-mail|@webdock\/mail-core|hosting-contracts\/vercel)/,
      file,
    );
    assert.doesNotMatch(source, /\bprocess\.env\b/, file);
  }
});
test("Hosting provider adapters depend on public contracts, never the Hosting runtime", () => {
  for (const adapter of ["hosting/github", "hosting/vercel"]) {
    const manifest = JSON.parse(
      readFileSync(resolve(root, "adapters", adapter, "package.json"), "utf8"),
    );
    assert(
      !Object.hasOwn(manifest.dependencies ?? {}, "@webdock/module-hosting"),
    );
    for (const file of files(resolve(root, "adapters", adapter, "src")))
      assert.doesNotMatch(
        readFileSync(file, "utf8"),
        /@webdock\/module-hosting/,
        file,
      );
  }
});
test("Hosting control-plane compatibility paths contain composition instead of SQL service logic", () => {
  for (const file of files(
    resolve(root, "legacy/control-plane/src/modules/hosting"),
  )) {
    const source = readFileSync(file, "utf8");
    assert.doesNotMatch(
      source,
      /\b(?:SELECT|INSERT INTO|UPDATE|DELETE FROM|CREATE TABLE|ALTER TABLE)\b/,
      file,
    );
  }
});
