import test from "node:test";
import assert from "node:assert/strict";
import {
  parsePluginConfiguration,
  gitPluginSource,
  encodePluginChunk,
  shouldPollPluginImports,
  shouldRetryPluginActivation,
  validatePluginArchive,
} from "../src/lib/plugin-source-ui";

test("configuration approval requires a JSON object", () => {
  assert.deepEqual(parsePluginConfiguration('{"region":"eu","count":2}'), {
    region: "eu",
    count: 2,
  });
  for (const value of ["null", "[]", "true", "invalid"])
    assert.throws(() => parsePluginConfiguration(value), /JSON object/);
});
test("Git credentials are separate from HTTPS URLs and optional refs are trimmed", () => {
  assert.deepEqual(
    gitPluginSource(" https://github.com/acme/plugin.git ", " main ", "", ""),
    { kind: "git", url: "https://github.com/acme/plugin.git", ref: "main" },
  );
  assert.deepEqual(
    gitPluginSource("https://git.example/plugin.git", "", "alice", "secret"),
    {
      kind: "git",
      url: "https://git.example/plugin.git",
      credential: { username: "alice", token: "secret" },
    },
  );
  for (const url of [
    "http://example.com/x",
    "ssh://example.com/x",
    "https://user:secret@example.com/x",
    "https://example.com/x?token=secret",
  ])
    assert.throws(() => gitPluginSource(url, "", "", ""), /HTTPS URL/);
  assert.throws(
    () => gitPluginSource("https://example.com/x", "", "alice", ""),
    /both/,
  );
});
test("ZIP chunks fit the studio transport limit and preserve binary bytes", () => {
  const bytes = Uint8Array.from({ length: 65536 }, (_, i) => i % 256);
  const encoded = encodePluginChunk(bytes);
  assert.ok(
    JSON.stringify({
      operation: "plugins.sources",
      args: [{ action: "chunk", id: "job", offset: 0, data: encoded }],
    }).length <
      128 * 1024,
  );
  assert.deepEqual(new Uint8Array(Buffer.from(encoded, "base64")), bytes);
  assert.throws(() => encodePluginChunk(new Uint8Array(65537)), /64 KiB/);
});
test("ZIP selection rejects empty, non-ZIP and over-limit archives", () => {
  assert.doesNotThrow(() =>
    validatePluginArchive({ name: "plugin.zip", size: 32 * 1024 * 1024 }),
  );
  for (const file of [
    { name: "plugin.tar", size: 1 },
    { name: "plugin.zip", size: 0 },
    { name: "plugin.zip", size: 32 * 1024 * 1024 + 1 },
  ])
    assert.throws(() => validatePluginArchive(file));
});
test("polling includes ready imports for automatic activation and stops at review, completion, failure, and ten minutes", () => {
  for (const status of [
    "queued",
    "building",
    "approved",
    "deploying",
    "ready",
  ] as const)
    assert.equal(shouldPollPluginImports([{ status }], 199), true);
  for (const status of ["uploading", "review", "enabled", "failed"] as const)
    assert.equal(shouldPollPluginImports([{ status }], 0), false);
  assert.equal(shouldPollPluginImports([{ status: "building" }], 200), false);
  assert.equal(shouldPollPluginImports([], 0), false);
});

test("activation retries only temporary readiness failures and stops after twenty attempts", () => {
  assert.equal(shouldRetryPluginActivation(503, 1), true);
  assert.equal(shouldRetryPluginActivation(503, 19), true);
  assert.equal(shouldRetryPluginActivation(503, 20), false);
  for (const status of [undefined, 400, 401, 403, 404, 409, 500])
    assert.equal(shouldRetryPluginActivation(status, 1), false);
});
