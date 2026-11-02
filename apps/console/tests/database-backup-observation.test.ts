import assert from "node:assert/strict";
import { test } from "node:test";
import { databaseBackupPolicyViewSchema, databaseBackupStatusSchema } from "@webdock/hosting-contracts";
import { backupObservationCurrent } from "../src/database-backup-observation";
import { backup, history } from "./support/database-backup-fixture";
await test("observation freshness uses the documented strict 90-second fence conservatively without assuming clock synchronization", () => {
  const view = databaseBackupStatusSchema.parse(backup), policy = databaseBackupPolicyViewSchema.parse(history).policy;
  assert(policy); policy.observedAt = "2026-10-25T10:00:00Z";
  for (const [clock, expected] of [["2026-10-25T09:59:59.999Z", false], ["2026-10-25T10:00:00Z", true], ["2026-10-25T10:01:29.999Z", true], ["2026-10-25T10:01:30Z", false]] as const) assert.equal(backupObservationCurrent(view, policy, Date.parse(clock), null), expected);
});
await test("current proof requires matching controls, resource evidence and effective revocation independently", () => {
  const view = databaseBackupStatusSchema.parse(backup), policy = databaseBackupPolicyViewSchema.parse(history).policy;
  assert(policy); assert(policy.observedControls); policy.observedAt = "2026-10-25T10:00:00Z";
  const clock = Date.parse("2026-10-25T10:00:01Z");
  assert.equal(backupObservationCurrent(view, { ...policy, observedControls: { ...policy.observedControls, retentionDelete: true } }, clock, null), false);
  assert.equal(backupObservationCurrent(view, { ...policy, observedControls: { ...policy.observedControls, objectStoreUID: "foreign" } }, clock, null), false);
  const suspended = { ...policy, spec: { ...policy.spec, suspended: true }, observedSuspended: true, revokedAt: "2026-10-25T10:00:00Z" };
  const target = { policyID: "5", revision: 2, revocation: true };
  assert.equal(backupObservationCurrent({ ...view, status: "suspended" }, suspended, clock, target), false);
  assert.equal(backupObservationCurrent({ ...view, status: "suspended" }, { ...suspended, revocationEffective: true }, clock, target), true);
});
