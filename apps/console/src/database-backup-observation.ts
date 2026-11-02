import type { DatabaseBackupPolicyView, DatabaseBackupStatus } from "@webdock/hosting-contracts";

type Policy = DatabaseBackupPolicyView["policy"];
export interface BackupObservationTarget { policyID: string; revision: number; revocation: boolean }
/** Mirrors Core's 90s freshness fence conservatively on the client clock, never a UTC/SLA guarantee. */
export function backupObservationCurrent(view: DatabaseBackupStatus, policy: Policy | undefined, now: number, target: BackupObservationTarget | null): boolean {
  if (!policy?.observedAt || !policy.observedControls || !view.observedControls) return false;
  const age = now - Date.parse(policy.observedAt);
  if (!Number.isFinite(age) || age < 0 || age >= 90000) return false;
  if (policy.id !== view.policyID || policy.revision !== view.revision || policy.observedRevision !== policy.revision || view.observedRevision !== policy.observedRevision) return false;
  if (target && (target.policyID !== policy.id || policy.observedRevision < target.revision || target.revocation && !policy.revocationEffective)) return false;
  return controlsMatch(view, policy);
}
function controlsMatch(view: DatabaseBackupStatus, policy: NonNullable<Policy>): boolean {
  const observed = policy.observedControls;
  if (!observed || policy.observedSuspended !== policy.spec.suspended) return false;
  if (policy.revokedAt !== null && (!policy.revocationEffective || !policy.observedSuspended)) return false;
  if (view.status !== (policy.observedSuspended ? "suspended" : "configured")) return false;
  if (observed.schedule !== policy.spec.schedule || observed.walEnabled !== policy.spec.walEnabled || observed.retentionDelete !== policy.spec.retentionDelete || observed.retentionDays !== policy.spec.retentionDays) return false;
  return Object.entries(observed).every(([key, value]) => view.observedControls?.[key as keyof typeof observed] === value);
}
