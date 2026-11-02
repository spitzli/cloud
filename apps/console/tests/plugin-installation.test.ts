import assert from "node:assert/strict";
import { test } from "node:test";
import { catalogSchema, installRequestSchema, installationPhase } from "../src/plugins/installation-contracts";

const release = {
  catalogID: "postgres", pluginID: "postgres", version: "0.1.0", manifestSha256: "a".repeat(64),
  image: `registry.example.test/postgres@sha256:${"b".repeat(64)}`, registryURL: "https://registry.example.test", signerKeyID: "owned-fixture",
  capabilities: ["plugin.workloads"], supportedFeatures: ["create-dedicated"],
  targets: [{ id: "eu", label: "EU cluster", reviewSha256: "c".repeat(64) }],
  configurationSchema: { type: "object", properties: {}, additionalProperties: false },
};
await test("installation review preserves exact release, target and request identities", () => {
  const [item] = catalogSchema.parse([release]);
  assert.ok(item);
  const [target] = item.targets;
  assert.ok(target);
  const request = { catalogID: item.catalogID, version: item.version, reviewSha256: target.reviewSha256,
    targetID: "eu", configuration: {}, approvedCapabilities: item.capabilities, idempotencyKey: "fixed-request" };
  assert.deepEqual(installRequestSchema.parse(request), request);
  for (const change of [{ reviewSha256: "unknown" }, { targetID: "" }, { idempotencyKey: "" }, { privateKey: "never-a-browser-field" }, { runtimeURL: "https://foreign.example.test" }]) {
    assert.equal(installRequestSchema.safeParse({ ...request, ...change }).success, false);
  }
});
await test("installation phases describe observed work without claiming premature success", () => {
  assert.equal(installationPhase("preparing_storage"), "Preparing private storage");
  assert.equal(installationPhase("verifying_runtime"), "Verifying runtime identity");
  assert.equal(installationPhase("unrecognized"), "Waiting for controller update");
});
await test("catalog accepts the complete native target label range", () => {
  for (const length of [120, 121, 160]) {
    assert.equal(catalogSchema.safeParse([{ ...release, targets: [{ ...release.targets[0], label: "L".repeat(length) }] }]).success, true);
  }
  assert.equal(catalogSchema.safeParse([{ ...release, targets: [{ ...release.targets[0], label: "L".repeat(161) }] }]).success, false);
});
