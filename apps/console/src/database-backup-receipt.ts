import { z } from "zod";
import { databaseViewSchema } from "@webdock/hosting-contracts";
import type { BackupMutation, BackupReceipt, KnownBackupPolicy, ReceiptExpectation } from "./database-backup-intent";
const id = databaseViewSchema.shape.id;
const destination = z.object({ destinationID: id }).strict();
const policy = z.object({ policyID: id, revision: z.number().int().min(1).max(2147483646), operationID: id, revocationEffective: z.literal(false) }).strict();
export function backupReceiptExpectation(command: BackupMutation, known: KnownBackupPolicy | null): ReceiptExpectation {
  if (command.action === "databases.backupDestinations.register") return { action: command.action, policyID: null, revision: 0 };
  if (command.action === "databases.backupPolicies.grant") return { action: command.action, policyID: known?.id ?? null, revision: 1 };
  if (!known) throw new Error("A current source policy is required");
  if (command.action === "databases.backup") return { action: command.action, policyID: known.id, revision: known.revision };
  return { action: command.action, policyID: known.id, revision: command.policyRevision + (command.action === "databases.backupPolicies.revoke" && known.revoked ? 0 : 1) };
}
/** Validate the dispatched action BEFORE resolving an ambiguous attempt. */
export function decodeBackupReceipt(value: unknown, expected: ReceiptExpectation, known: KnownBackupPolicy | null): BackupReceipt {
  if (expected.action === "databases.backupDestinations.register") return z.object({ data: destination }).strict().parse(value).data;
  const receipt = z.object({ data: policy }).strict().parse(value).data;
  const policyID = expected.policyID ?? known?.id;
  if (policyID !== undefined && receipt.policyID !== policyID) throw new Error("Receipt policy identity changed");
  // Manual backup has no policyRevision input: Core returns the live policy revision.
  // Bound a later revision by the same source's current read, never an unbounded wildcard.
  if (expected.action === "databases.backup") {
    if (known?.id !== receipt.policyID || receipt.revision < expected.revision || receipt.revision > known.revision) throw new Error("Receipt revision is not confirmed by the source");
    return receipt;
  }
  if (receipt.revision !== expected.revision) throw new Error("Receipt revision changed");
  return receipt;
}
