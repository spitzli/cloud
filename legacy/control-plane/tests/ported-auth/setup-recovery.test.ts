import test from "node:test";
import assert from "node:assert/strict";
import { resetGitSetupRetry } from "../../src/modules/hosting/setup-recovery";
const state = () => ({
  resources: {
    appID: "1",
    databaseID: "2",
    bindingIDs: ["3"],
    sourceID: "4",
    buildID: "5",
    releaseID: "6",
  },
  steps: [
    "application",
    "database",
    "bindings",
    "source",
    "build",
    "release",
    "readiness",
  ].map((name) => ({
    name,
    state:
      name === "release"
        ? "blocked"
        : name === "readiness"
          ? "pending"
          : "succeeded",
    operationID: null,
    reason: null,
  })),
  preview: { sourceSHA: "a".repeat(40) },
});
test("terminal Git publication retries rebuild the reviewed commit and keep provisioned resources", () => {
  for (const status of ["failed", "superseded"]) {
    const row = state();
    resetGitSetupRetry(row as never, status);
    assert.equal(row.resources.buildID, undefined);
    assert.equal(row.resources.releaseID, undefined);
    assert.equal(row.resources.appID, "1");
    assert.equal(row.resources.databaseID, "2");
    assert.deepEqual(row.resources.bindingIDs, ["3"]);
    assert.equal(row.preview.sourceSHA, "a".repeat(40));
    assert.equal(row.steps[4].state, "pending");
    assert.equal((row.steps[4] as { attempt?: number }).attempt, 1);
    assert.equal(row.steps[5].state, "pending");
  }
});
test("uncertain Git publication cannot authorize a replacement build", () => {
  const row = state();
  assert.throws(
    () => resetGitSetupRetry(row as never, "needs-reconciliation"),
    /reconcil/i,
  );
  assert.equal(row.resources.buildID, "5");
});
