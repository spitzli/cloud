import test from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
test("packed contracts load outside the monorepo", { timeout: 60_000 }, () => {
  const dir = mkdtempSync(join(tmpdir(), "webdock-plugin-pack-"));
  try {
    const root = resolve(import.meta.dirname, "../../..");
    const sdk = resolve(import.meta.dirname, "..");
    execFileSync(process.execPath, ["run", "build"], { cwd: sdk, stdio: "pipe" });
    const tarball = resolve(
      dir,
      execFileSync(
        process.execPath,
        ["pm", "pack", "--ignore-scripts", "--destination", dir, "--quiet"],
        { cwd: sdk, encoding: "utf8" },
      ).trim(),
    );
    const files = execFileSync("tar", ["-tzf", tarball], { encoding: "utf8" })
      .split("\n")
      .filter(Boolean);
    assert.ok(files.includes("package/dist/contracts/index.d.ts"));
    assert.ok(files.every((path) => !path.startsWith("package/src/")));
    // Unpack beside copies of its runtime dependencies only: no registry, no dependency scripts.
    const installed = join(dir, "node_modules", "@webdock", "plugin-sdk");
    mkdirSync(installed, { recursive: true });
    execFileSync("tar", ["-xzf", tarball, "-C", installed, "--strip-components=1"]);
    const manifest = JSON.parse(readFileSync(join(installed, "package.json"), "utf8"));
    for (const name of Object.keys(manifest.dependencies ?? {}))
      symlinkSync(join(root, "node_modules", name), join(dir, "node_modules", name));
    writeFileSync(
      join(dir, "package.json"),
      '{"private":true,"type":"module"}',
    );
    const result = execFileSync(
      process.execPath,
      [
        "-e",
        "import {parsePluginManifest} from '@webdock/plugin-sdk/contracts'; try {parsePluginManifest({})} catch(e) {if(e.code==='invalid_input') process.exit(0)} process.exit(1)",
      ],
      { cwd: dir },
    );
    assert.equal(result.length, 0);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
