import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
const cli = new URL("../src/cli.js", import.meta.url);
test("scaffold refuses overwrite; validation and pack produce identical descriptors", async () => {
  const parent = await mkdtemp(join(tmpdir(), "webdock-cli-"));
  const dir = join(parent, "demo");
  const run = (...args) =>
    spawnSync(
      process.execPath,
      [cli.pathname, "plugin", ...args, "--dir", dir, "--json"],
      { encoding: "utf8" },
    );
  try {
    assert.equal(run("create", "job-demo").status, 0);
    assert.notEqual(run("create", "job-demo").status, 0);
    assert.equal(run("validate").status, 0);
    assert.notEqual(run("pack").status, 0);
    assert.equal(
      run("pack", "--image", "ghcr.io/spitzli/demo@sha256:" + "a".repeat(64))
        .status,
      0,
    );
    assert.equal(
      await readFile(join(dir, "dist/tabularium.json"), "utf8"),
      await readFile(join(dir, "dist/job-demo-universal.json"), "utf8"),
    );
    assert.equal(
      JSON.parse(await readFile(join(dir, "package.json"), "utf8"))
        .dependencies["@spitzli/plugin-sdk"],
      "0.1.0",
    );
  } finally {
    await rm(parent, { recursive: true, force: true });
  }
});

test('actions scaffold includes a vendored SDK and declares no unsupported services', async () => {
  const parent = await mkdtemp(join(tmpdir(), 'webdock-actions-'));
  const dir = join(parent, 'demo');
  try {
    const result = spawnSync(process.execPath, [cli.pathname, 'plugin', 'create', 'echo-demo', '--template', 'actions', '--dir', dir, '--json'], {encoding:'utf8'});
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(await readFile(join(dir, 'tabularium.json'), 'utf8'));
    assert.deepEqual(manifest.capabilities, []);
    assert.deepEqual(manifest.services, {required:[],optional:[]});
    assert.deepEqual(manifest.jobs, []);
    assert.deepEqual(manifest.actions, [{name:'demo.echo',intent:'read',capabilities:[]}]);
    const pkg = JSON.parse(await readFile(join(dir, 'package.json'), 'utf8'));
    assert.equal(pkg.dependencies['@spitzli/plugin-sdk'], 'file:vendor/plugin-sdk');
    const sdk = JSON.parse(await readFile(join(dir, 'vendor/plugin-sdk/package.json'), 'utf8'));
    assert.equal(sdk.name, '@spitzli/plugin-sdk');
    assert.ok(sdk.dependencies.zod);
    assert.equal(sdk.scripts, undefined, "vendored compiled SDK must not rerun source build hooks");
    assert.match(await readFile(join(dir, 'vendor/plugin-sdk/LICENSE'), 'utf8'), /Apache License/);
    assert.match(await readFile(join(dir, 'vendor/plugin-sdk/NOTICE'), 'utf8'), /Spitzli/);
    assert.match(await readFile(join(dir, '.gitignore'), 'utf8'), /^\/dist\/$/m);
    assert.ok(await readFile(join(dir, 'vendor/plugin-sdk/dist/server/index.js'), 'utf8'));
    assert.match(await readFile(join(dir, 'Dockerfile'), 'utf8'), /EXPOSE 3443/);
    const validation = spawnSync(process.execPath, [cli.pathname, 'plugin', 'validate', '--dir', dir, '--json'], {encoding:'utf8'});
    assert.equal(validation.status, 0, validation.stderr);
  } finally {await rm(parent, {recursive:true,force:true});}
});
