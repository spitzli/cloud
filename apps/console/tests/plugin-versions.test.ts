import assert from "node:assert/strict";
import { test } from "node:test";
import { releaseHistorySchema, updateRequestSchema, rollbackRequestSchema, readsReservedSchema } from "../src/plugins/version-contracts";

await test("version commands preserve string revisions and refuse private or placement inputs", () => {
  const common = { reviewSha256: "a".repeat(64), expectedRevision: "9007199254740993", approvedCapabilities: ["plugin.workloads"], idempotencyKey: "same-reviewed-request" };
  const update = { ...common, version: "0.2.0" };
  assert.deepEqual(updateRequestSchema.parse(update), update);
  const rollback = { ...common, releaseGeneration: "9007199254740993" };
  assert.deepEqual(rollbackRequestSchema.parse(rollback), rollback);
  for (const extra of [{ image: "foreign" }, { privateKey: "private" }, { namespace: "foreign" }, { targetID: "other" }, { configuration: {} }]) {
    assert.equal(updateRequestSchema.safeParse({ ...update, ...extra }).success, false);
    assert.equal(rollbackRequestSchema.safeParse({ ...rollback, ...extra }).success, false);
  }
  assert.equal(rollbackRequestSchema.safeParse({ ...rollback, releaseGeneration: 1 }).success, false);
});
await test("version review uses the reserved logical schema ceiling and fails closed on missing evidence", () => {
  assert.equal(readsReservedSchema({ schemaVersion: 1, readableSchema: { min: 1, max: 1 } }, 1), true);
  assert.equal(readsReservedSchema({ schemaVersion: 1, readableSchema: { min: 1, max: 1 } }, 2), false);
  assert.equal(readsReservedSchema({ schemaVersion: 2, readableSchema: { min: 1, max: 1 } }, 1), false);
  assert.equal(readsReservedSchema({ schemaVersion: 2, readableSchema: { min: 1, max: 2 } }, 1), true);
  assert.equal(readsReservedSchema({}, 1), false);
  assert.equal(readsReservedSchema({ schemaVersion: 1, readableSchema: { min: 1, max: 1 } }, null), false);
});
await test("version review rejects a future-only reader before advancing the retained schema ceiling", () => {
  assert.equal(readsReservedSchema({ schemaVersion: 2, readableSchema: { min: 2, max: 2 } }, 1), false);
});
await test("release history separates active and candidate generations and carries rollback blockers", () => {
  const history = { pluginID: "postgres", currentGeneration: "1", candidateGeneration: "2", previousGeneration: null, schemaHighWater: 2, targetID: "eu", releases: [{ generation: "1", version: "0.1.0", imageDigest: "image@sha256:hash", manifestSha256: "a".repeat(64), status: "stopped", operationID: "11", schemaVersion: 1, readableSchema: { min: 1, max: 1 }, rollbackEligible: false, rollbackBlocker: "schema_incompatible" }] };
  assert.deepEqual(releaseHistorySchema.parse(history), history);
});
