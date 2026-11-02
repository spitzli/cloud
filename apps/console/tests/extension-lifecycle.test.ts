import assert from "node:assert/strict";
import { test } from "node:test";
import {
  installedSchema,
  stateRequest,
  extensionPolling,
} from "../src/plugins/lifecycle-contracts";
const row = {
  id: "example-extension",
  kind: "external",
  revision: "5",
  desiredState: "enabled",
  state: "enabled",
  version: "1.0.0",
  imageDigest: null,
  runtime: "ready",
  blockers: [],
  dependencies: [],
};
await test("extension lifecycle keeps availability separate from process health and retained state", () => {
  const installed = installedSchema.parse([row]);
  assert.equal(installed[0]?.runtime, "ready");
  assert.equal(installed[0].state, "enabled");
  const request = stateRequest(
    row.id,
    row.revision,
    "disabled",
    "request-identity-123",
  );
  assert.deepEqual(request, {
    action: "state",
    pluginID: row.id,
    expectedRevision: "5",
    desired: "disabled",
    idempotencyKey: "request-identity-123",
  });
  assert.throws(() =>
    stateRequest(row.id, "not-a-revision", "disabled", "request-identity-123"),
  );
});
await test("blocked extension operations require review and do not poll as if running", () => {
  assert.equal(extensionPolling("pending", false), 2000);
  assert.equal(extensionPolling("running", false), 2000);
  for (const status of ["blocked", "failed", "succeeded"])
    assert.equal(extensionPolling(status, false), false);
  assert.equal(extensionPolling("running", true), false);
});
