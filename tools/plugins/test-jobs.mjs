import { mkdirSync, mkdtempSync, rmSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import assert from "node:assert/strict";
const root = resolve(import.meta.dirname, "../.."),
  temp = mkdtempSync(join(tmpdir(), "webdock-plugin-e2e-"));
const name = "webdock-jobs-" + randomUUID();
function run(command, args, options = {}) {
  const r = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 10 * 1024 * 1024,
    ...options,
  });
  if (r.status !== 0)
    throw Error(
      `${command} ${args.join(" ")} failed\n${r.stdout}\n${r.stderr}`,
    );
  return r.stdout.trim();
}
try {
  run(process.execPath, ["run", "--filter", "@webdock/plugin-sdk", "build"]);
  run("docker", [
    "run",
    "-d",
    "--name",
    name,
    "-p",
    "127.0.0.1::6379",
    "redis:7.4-alpine@sha256:858f009f9709ce576febc734aa78b8f6d624b82571f9ddb6bda4377c833b3499",
    "redis-server",
    "--appendonly",
    "yes",
    "--maxmemory-policy",
    "noeviction",
  ]);
  const port = run("docker", ["port", name, "6379"]).split(":").at(-1);
  for (let i = 0; i < 30; i++) {
    const r = spawnSync("docker", ["exec", name, "redis-cli", "ping"], {
      encoding: "utf8",
    });
    if (r.stdout.trim() === "PONG") break;
    if (i === 29) throw Error("Redis startup failed");
    await new Promise((r) => setTimeout(r, 100));
  }
  const env = { ...process.env, WEBDOCK_TEST_REDIS_PORT: port };
  console.log(
    run(
      process.execPath,
      [
        "test",
        "./packages/core/tests/jobs/jobs.test.ts",
        "./packages/core/tests/jobs/integration.test.ts",
        "./packages/core/tests/jobs/core.integration.test.ts",
        "./packages/core/tests/jobs/schedules.integration.test.ts",
      ],
      { env },
    ),
  );
  run(process.execPath, ["pm", "pack", "--destination", temp], { cwd: join(root, "packages/plugin-sdk") });
  run(process.execPath, ["pm", "pack", "--destination", temp], { cwd: join(root, "packages/plugin-cli") });
  const sdk = join(temp, "webdock-plugin-sdk-0.1.0.tgz"),
    cli = join(temp, "webdock-cli-0.1.0.tgz"),
    authoring = join(temp, "authoring"),
    plugin = join(temp, "plugin");
  mkdirSync(authoring);
  writeFileSync(join(authoring, "package.json"), "{}\n");
  run(process.execPath, ["add", "--ignore-scripts", sdk, cli], { cwd: authoring });
  const command = join(authoring, "node_modules/@webdock/cli/src/cli.js");
  run(process.execPath, [
    command,
    "plugin",
    "create",
    "job-demo",
    "--dir",
    plugin,
    "--json",
  ]);
  run(process.execPath, ["add", "--ignore-scripts", "--no-save", sdk, cli], { cwd: plugin });
  run(process.execPath, [command, "plugin", "validate", "--dir", plugin]);
  run(process.execPath, [command, "plugin", "build", "--dir", plugin]);
  // Packing fixture only: never represent this made-up image as a published runtime.
  run(process.execPath, [
    command,
    "plugin",
    "pack",
    "--dir",
    plugin,
    "--image",
    "ghcr.io/spitzli/job-demo@sha256:" + "a".repeat(64),
  ]);
  assert.equal(
    readFileSync(join(plugin, "dist/tabularium.json"), "utf8"),
    readFileSync(join(plugin, "dist/job-demo-universal.json"), "utf8"),
  );
  console.log(
    run(
      process.execPath,
      ["tools/plugins/test-job-demo.ts", plugin],
      { env },
    ),
  );
  console.log(
    "Packed SDK + packed CLI → empty directory → build/validate/pack → separate plugin process: PASS",
  );
} finally {
  spawnSync("docker", ["rm", "-f", name], { stdio: "ignore" });
  rmSync(temp, { recursive: true, force: true });
}
