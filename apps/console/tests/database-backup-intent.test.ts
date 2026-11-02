import assert from "node:assert/strict";
import { test } from "node:test";
import { DatabaseBackupIntent, DatabaseBackupIntents, type BackupAttempt } from "../src/database-backup-intent";
import { backupReceiptExpectation, decodeBackupReceipt } from "../src/database-backup-receipt";
const attempt = (key: string): BackupAttempt => ({ key, digest: "nonsecret-fixture-digest", action: "databases.backupDestinations.register", publicCommand: null, uncertain: false, expected: { action: "databases.backupDestinations.register", policyID: null, revision: 0 } });
await test("refresh nonces, delivery generations and authority epochs fence stale transitions", () => {
  const cell = new DatabaseBackupIntent("actor/session", "customer/project/resource"); cell.attach();
  const first = cell.begin(attempt("attempt-a")); assert(first); cell.fail(first, "conflict");
  const refreshA = cell.beginRefresh(), refreshB = cell.beginRefresh(); assert(refreshA); assert(refreshB);
  assert.equal(cell.finishRefresh(refreshA, true), false); assert.equal(cell.canStart(), false);
  assert.equal(cell.finishRefresh(refreshB, true), true);
  const pending = cell.begin(attempt("attempt-c")); assert(pending);
  assert.equal(cell.finishRefresh(refreshB, true), false); assert.equal(cell.snapshot().attempt?.key, "attempt-c");
  const oldAuth = cell.beginRefresh(); assert(oldAuth); cell.fail(pending, "forbidden");
  assert.equal(cell.finishRefresh(oldAuth, true), false); assert.equal(cell.snapshot().state.status, "forbidden"); assert.equal(cell.canStart(), false);
  cell.retire(); assert.equal(cell.begin(attempt("retired")), null); assert.equal(cell.snapshot().attempt, null);
});
await test("same identity preserves intent; confirmed logout and changed resource retire it", () => {
  const owner = new DatabaseBackupIntents(), cell = owner.get("platform/user/session", "customer/project/resource"); owner.confirm(cell.identity); owner.activate(cell.key);
  const token = cell.begin(attempt("same-key")); assert(token); cell.fail(token, "uncertain");
  owner.confirm("platform/user/session"); assert.equal(owner.get(cell.identity, cell.key).snapshot().attempt?.key, "same-key");
  owner.confirm(null); assert.equal(cell.snapshot().retired, true); assert.equal(cell.snapshot().attempt, null);
  const next = owner.get("platform/user/session", "customer/project/resource"); owner.activate(next.key); owner.get(next.identity, "customer/project/other"); owner.activate("customer/project/other"); assert.equal(next.snapshot().retired, true);
});
await test("project selection compares the retained actual scope, preserving restoration but retiring a different project", () => {
  const identity = "platform/user/session", owner = new DatabaseBackupIntents();
  const key = JSON.stringify([identity, "10", "20", "1"]), cell = owner.get(identity, key); owner.confirm(identity); owner.activate(key);
  const token = cell.begin(attempt("retained-project-key")); assert(token); cell.fail(token, "uncertain");
  owner.selectProject(identity, "10", "20"); assert.equal(cell.snapshot().attempt?.key, "retained-project-key");
  owner.selectProject(identity, "10", "2"); assert.equal(cell.snapshot().retired, true); assert.equal(cell.snapshot().attempt, null);
});
await test("receipt codecs follow native grant/update/revoke/manual semantics and reject wrong actions", () => {
  const known = { id: "5", revision: 2, revoked: false }, receipt = { policyID: "5", revision: 3, operationID: "40", revocationEffective: false };
  const update = { action: "databases.backupPolicies.update", policyID: "5", revision: 3 } as const;
  assert.deepEqual(decodeBackupReceipt({ data: receipt }, update, known), receipt);
  for (const change of [{ policyID: "6" }, { revision: 2 }, { revision: 4 }, { revocationEffective: true }]) assert.throws(() => decodeBackupReceipt({ data: { ...receipt, ...change } }, update, known));
  assert.throws(() => decodeBackupReceipt({ data: { destinationID: "5" } }, update, known));
  assert.throws(() => decodeBackupReceipt({ data: receipt }, { action: "databases.backupDestinations.register", policyID: null, revision: 0 }, known));
  assert.throws(() => decodeBackupReceipt({ data: receipt }, { action: "databases.backupPolicies.grant", policyID: null, revision: 1 }, null));
  assert.equal(backupReceiptExpectation({ action: "databases.backupPolicies.revoke", databaseID: "1", revision: 4, policyRevision: 2, idempotencyKey: "fixture-revoke-key" }, { ...known, revoked: true }).revision, 2);
  const manual = { action: "databases.backup", policyID: "5", revision: 2 } as const;
  assert.throws(() => decodeBackupReceipt({ data: receipt }, manual, known));
  assert.deepEqual(decodeBackupReceipt({ data: receipt }, manual, { ...known, revision: 4 }), receipt);
  assert.throws(() => decodeBackupReceipt({ data: { ...receipt, revision: 5 } }, manual, { ...known, revision: 4 }));
});
