import test from "node:test";
import assert from "node:assert/strict";
import evidence from "../../../docs/verification/2026-10-25-native-postgresql-backup-transitions.json";
import nativeCommands from "../../../crates/databases/src/managed-command.schema.json";
import { databaseViewSchema, databaseCommandSchema, databaseReadActions, databaseBackupPolicyViewSchema } from "./databases";

const view = {
  id: "1", projectID: "2", name: "DB", revision: 1, observedRevision: 1,
  state: "ready", planRevisionID: "3", kind: "dedicated", postgresMajor: 18,
  cpuMillicores: 1000, memoryBytes: 2147483648, volumeBytes: 21474836480,
  logicalStorageAllowanceBytes: null, storageEnforcement: "bounded-filesystem",
  databaseName: "db_1", endpoint: null, operationID: null, observedAt: null,
  usage: null, cancellationRequestedAt: null,
};
const captured = evidence.connectedEvidence.observationFailure;
const noPolicy = {
  status: "not-configured", lastSuccessAt: null, recoverableUntil: null,
  lastRestoreTestAt: null, observedControls: null, publicRecoveryAvailable: false,
  disabledReason: "production-qualification-required", rpoSeconds: null, rtoSeconds: null,
};

test("current native backup views parse in detail and list, including captured observation loss", () => {
  for (const backup of [noPolicy, captured.before, captured.after,
    { ...captured.before, status: "suspension-pending" },
    { ...captured.before, status: "suspended" },
  ]) {
    const candidate = { ...view, backup };
    assert.equal(databaseViewSchema.safeParse(candidate).success, true, backup.status);
    assert.equal(databaseViewSchema.array().safeParse([candidate]).success, true, backup.status);
  }
});

test("backup views cannot enable public recovery, invent commitments or carry provider secrets", () => {
  for (const change of [
    { publicRecoveryAvailable: true }, { disabledReason: "ready" }, { rpoSeconds: 30 },
    { rtoSeconds: 60 }, { recoverableUntil: "2026-10-25T12:00:00Z" },
    { lastRestoreTestAt: "2026-10-25T12:00:00Z" }, { secretAccessKey: "not-a-provider-key" },
    { observedControls: { ...captured.before.observedControls, secretAccessKey: "not-a-provider-key" } },
  ]) assert.equal(databaseViewSchema.safeParse({ ...view, backup: { ...captured.before, ...change } }).success, false);
});

test("existing native backup actions and restore fields are strict and revision fenced in the client", () => {
  const target = { databaseID: "1", revision: 1, idempotencyKey: "backup_contract_test_001" };
  const policy = { ...target, schedule: "0 15 2 * * *", retentionDays: 30, retentionDelete: false };
  const commands = [
    { action: "databases.backupPolicies.get", databaseID: "1" },
    { action: "databases.backupPolicies.grant", ...policy, destinationID: "2" },
    { action: "databases.backupPolicies.update", ...policy, policyRevision: 2, suspended: true, walEnabled: false },
    { action: "databases.backupPolicies.revoke", ...target, policyRevision: 2 },
    { action: "databases.backup", ...target },
    { action: "databases.backupDestinations.register", endpointURL: "https://s3.example.test", region: "eu-test", bucket: "owned-backup-test", keyRecoveryID: "3", idempotencyKey: target.idempotencyKey,
      writeCredentials: { accessKeyId: "fixture-write", secretAccessKey: "fixture-only-write-material", readOnly: false },
      recoveryCredentials: { accessKeyId: "fixture-recovery", secretAccessKey: "fixture-only-recovery-material", readOnly: true } },
    { action: "databases.restore", sourceDatabaseID: "1", targetProjectID: "2", name: "Fresh target", restorePointID: "3", targetOrderID: "4", revision: 1,
      targetTime: "2026-10-25T12:00:00+00:00", timeline: 1, credentialRevision: 2, verificationSHA256: "a".repeat(64), idempotencyKey: target.idempotencyKey },
  ];
  for (const command of commands) {
    assert.equal(databaseCommandSchema.safeParse(command).success, true, command.action);
    const native = nativeCommands.oneOf.find(row => row.properties.action.const === command.action);
    assert(native, "The action must already exist in native Core.");
    for (const field of native.required) {
      const missing: Record<string, unknown> = { ...command }; delete missing[field];
      assert.equal(databaseCommandSchema.safeParse(missing).success, false, command.action + ":" + field);
    }
    assert.equal(databaseCommandSchema.safeParse({ ...command, privateOverride: true }).success, false);
  }
  assert(new Set<string>(databaseReadActions).has("databases.backupPolicies.get"));
  for (const command of commands.slice(1)) assert.equal(new Set<string>(databaseReadActions).has(command.action), false);
});
test("policy history accepts actual Core spec and a grant awaiting its first observation, while rejecting material", () => {
  const policy = evidence.connectedEvidence.resize.policy;
  assert(databaseBackupPolicyViewSchema.safeParse({ policy, backups: [] }).success);
  const pending = { ...policy, observedRevision: 0, observedSuspended: null, observedAt: null, observedControls: null };
  assert(databaseBackupPolicyViewSchema.safeParse({ policy: pending, backups: [] }).success);
  for (const extra of [{ credentials: { secretAccessKey: "fixture-only" } }, { spec: { ...policy.spec, destination: { ...policy.spec.destination, secretAccessKey: "fixture-only" } } }]) assert.equal(databaseBackupPolicyViewSchema.safeParse({ policy: { ...policy, ...extra }, backups: [] }).success, false);
});
