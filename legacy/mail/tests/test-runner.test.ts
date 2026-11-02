import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, writeFile, readFile, rm, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const runner = fileURLToPath(new URL("../../../scripts/test-mail.mjs", import.meta.url));
async function dockerFixture() {
  const directory = await mkdtemp(join(tmpdir(), "webdock-mail-runner-"));
  const calls = join(directory, "calls.jsonl");
  await writeFile(join(directory, "docker"), `#!${process.execPath}
import { appendFileSync } from 'node:fs';
const args = process.argv.slice(2);
if (args[0] === 'context' && args[1] === 'inspect') {
  console.log(process.env.DOCKER_CONTEXT === 'remote-fixture' ? 'ssh://fixture.invalid' : 'unix:///fixture/docker.sock');
} else {
  appendFileSync(process.env.MAIL_DOCKER_CALLS, JSON.stringify({ args, host: process.env.DOCKER_HOST, context: process.env.DOCKER_CONTEXT }) + '\\n');
  process.exit(78); // Stop before the real test suite can run; never contact Docker.
}
`, { mode: 0o700 });
  return { directory, calls, env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, MAIL_DOCKER_CALLS: calls } };
}

test("a remote Docker context overrides a local DOCKER_HOST and must be rejected before Compose", async () => {
  const fixture = await dockerFixture();
  try {
    const result = spawnSync(process.execPath, [runner], { encoding: "utf8", env: { ...fixture.env, DOCKER_HOST: "unix:///var/run/docker.sock", DOCKER_CONTEXT: "remote-fixture" } });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /refuse remote Docker engines/);
    await assert.rejects(access(fixture.calls));
  } finally { await rm(fixture.directory, { recursive: true, force: true }); }
});

test("pins the resolved local context for setup and cleanup instead of inheriting a conflicting host", async () => {
  const fixture = await dockerFixture();
  try {
    spawnSync(process.execPath, [runner], { encoding: "utf8", env: { ...fixture.env, DOCKER_HOST: "ssh://ignored.invalid", DOCKER_CONTEXT: "local-fixture" } });
    const calls = (await readFile(fixture.calls, "utf8")).trim().split("\n").map(line => JSON.parse(line));
    assert.ok(calls.some(call => call.args.includes("up")));
    assert.ok(calls.some(call => call.args.includes("down")));
    for (const call of calls) {
      assert.equal(call.host, "unix:///fixture/docker.sock");
      assert.equal(call.context, undefined);
    }
  } finally { await rm(fixture.directory, { recursive: true, force: true }); }
});
