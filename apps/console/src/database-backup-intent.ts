import type { DatabaseCommand } from "@webdock/hosting-contracts";
type ManualBackup = Extract<DatabaseCommand, { databaseID: string; revision: number; idempotencyKey: string }> & { action: "databases.backup" };
export type BackupMutation = Extract<DatabaseCommand, { action: "databases.backupDestinations.register" | "databases.backupPolicies.grant" | "databases.backupPolicies.update" | "databases.backupPolicies.revoke" }> | ManualBackup;
export type PublicBackupCommand = Exclude<BackupMutation, { action: "databases.backupDestinations.register" }>;
export interface KnownBackupPolicy { id: string; revision: number; revoked: boolean }
export type BackupReceipt = { destinationID: string } | { policyID: string; revision: number; operationID: string; revocationEffective: false };
export interface PolicyExpectation { policyID: string; revision: number; revocation: boolean }
export interface ReceiptExpectation { action: BackupMutation["action"]; policyID: string | null; revision: number }
export interface BackupAttempt { key: string; digest: string; action: BackupMutation["action"]; publicCommand: PublicBackupCommand | null; uncertain: boolean; expected: ReceiptExpectation }
export interface BackupIntentState { status: "idle" | "pending" | "accepted" | "uncertain" | "rejected" | "forbidden"; receipt?: BackupReceipt; error?: "fields" | "changed" | "conflict" | "forbidden" | "uncertain"; key?: string; action?: BackupMutation["action"] }
export interface DeliveryToken { generation: number; epoch: number; key: string }
export interface RefreshToken { nonce: number; generation: number; epoch: number; key: string | null }
export interface IntentSnapshot { state: BackupIntentState; attempt: BackupAttempt | null; policyTarget: PolicyExpectation | null; generation: number; epoch: number; refreshNonce: number; refreshPending: boolean; readDenied: boolean; writeDenied: boolean; needsReattach: boolean; retired: boolean }
type IntentError = NonNullable<BackupIntentState["error"]>;

/** One provider-owned resource's NONSECRET state machine. No transports or request material. */
export class DatabaseBackupIntent {
  private value: IntentSnapshot = { state: { status: "idle" }, attempt: null, policyTarget: null, generation: 0, epoch: 0, refreshNonce: 0, refreshPending: false, readDenied: false, writeDenied: false, needsReattach: false, retired: false };
  private readonly listeners = new Set<() => void>();
  private views = 0;
  constructor(readonly identity: string, readonly key: string) {}
  readonly snapshot = () => this.value;
  readonly subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(change: Partial<IntentSnapshot>) { this.value = { ...this.value, ...change }; for (const listener of this.listeners) listener(); }
  attach() { this.views++; }
  detach() {
    this.views = Math.max(0, this.views - 1);
    if (this.views || this.value.retired) return;
    this.denyRead(true);
  }
  retire() {
    this.update({ state: { status: "idle" }, attempt: null, policyTarget: null, generation: this.value.generation + 1, epoch: this.value.epoch + 1, refreshPending: false, readDenied: true, writeDenied: true, needsReattach: false, retired: true });
  }
  denyRead(reattach = false) {
    if (this.value.retired) return;
    const { state, attempt } = this.value;
    const pending = state.status === "pending";
    this.update({ generation: this.value.generation + 1, epoch: this.value.epoch + 1, readDenied: true, refreshPending: false, needsReattach: reattach,
      attempt: pending && attempt ? { ...attempt, uncertain: true } : attempt,
      state: pending ? { ...state, status: "uncertain", error: "uncertain" } : state });
  }
  canStart(): boolean {
    const v = this.value;
    return !v.retired && !v.readDenied && !v.writeDenied && !v.refreshPending && !["pending", "rejected", "forbidden"].includes(v.state.status);
  }
  begin(attempt: BackupAttempt): DeliveryToken | null {
    if (!this.canStart()) return null;
    const old = this.value.attempt;
    if (old && (old.key !== attempt.key || old.digest !== attempt.digest)) { this.localFailure("changed"); return null; }
    const generation = this.value.generation + 1;
    this.update({ generation, attempt: old ?? attempt, state: { status: "pending", key: attempt.key, action: attempt.action } });
    return { generation, epoch: this.value.epoch, key: attempt.key };
  }
  current(token: DeliveryToken): boolean { const v = this.value; return !v.retired && token.generation === v.generation && token.epoch === v.epoch && token.key === v.attempt?.key; }
  abort(token: DeliveryToken) { if (this.current(token)) this.fail(token, "uncertain"); }
  fail(token: DeliveryToken, error: IntentError) {
    if (!this.current(token)) return;
    const attempt = this.value.attempt;
    const uncertain = attempt?.uncertain === true || error === "uncertain";
    const forbidden = error === "forbidden";
    this.update({ generation: this.value.generation + 1, epoch: this.value.epoch + (forbidden ? 1 : 0), refreshPending: false,
      writeDenied: this.value.writeDenied || forbidden,
      attempt: attempt && (uncertain || !forbidden) ? { ...attempt, uncertain } : null,
      state: { ...this.value.state, status: forbidden ? "forbidden" : uncertain ? "uncertain" : "rejected", error } });
  }
  localFailure(error: IntentError) {
    if (this.value.retired || this.value.refreshPending || this.value.state.status === "pending") return;
    this.update({ generation: this.value.generation + 1, state: { ...this.value.state, status: this.value.attempt?.uncertain ? "uncertain" : "rejected", error: this.value.attempt?.uncertain ? "changed" : error } });
  }
  accept(token: DeliveryToken, receipt: BackupReceipt) {
    if (!this.current(token)) return;
    const attempt = this.value.attempt;
    if (!attempt) return;
    let target = this.value.policyTarget;
    if (attempt.action.startsWith("databases.backupPolicies.") && "policyID" in receipt && (!target || target.policyID === receipt.policyID && target.revision <= receipt.revision)) target = { policyID: receipt.policyID, revision: receipt.revision, revocation: attempt.action === "databases.backupPolicies.revoke" || target?.revocation === true };
    this.update({ generation: this.value.generation + 1, attempt: null, policyTarget: target, refreshPending: false, state: { status: "accepted", key: token.key, action: attempt.action, receipt } });
  }
  beginRefresh(automatic = false): RefreshToken | null {
    const v = this.value;
    if (v.retired || automatic && v.refreshPending) return null;
    const nonce = v.refreshNonce + 1;
    this.update({ refreshNonce: nonce, refreshPending: true, needsReattach: false });
    return { nonce, generation: v.generation, epoch: v.epoch, key: v.attempt?.key ?? null };
  }
  refreshCurrent(token: RefreshToken): boolean {
    const v = this.value;
    return !v.retired && v.refreshPending && token.nonce === v.refreshNonce && token.generation === v.generation && token.epoch === v.epoch && token.key === (v.attempt?.key ?? null);
  }
  finishRefresh(token: RefreshToken, valid: boolean): boolean {
    if (!this.refreshCurrent(token)) {
      if (token.nonce === this.value.refreshNonce && this.value.refreshPending) this.update({ refreshPending: false });
      return false;
    }
    if (!valid) { this.update({ refreshPending: false }); return false; }
    const { state, attempt } = this.value;
    const clear = !attempt?.uncertain && ["rejected", "forbidden"].includes(state.status);
    const next: BackupIntentState = attempt?.uncertain ? { ...state, status: "uncertain", error: "uncertain" } : clear ? { status: "idle" } : state;
    this.update({ refreshPending: false, readDenied: false, writeDenied: false, state: next, attempt: clear ? null : attempt });
    return true;
  }
}

/** This map exists only inside one mounted application provider. */
export class DatabaseBackupIntents {
  private readonly cells = new Map<string, DatabaseBackupIntent>();
  private active: string | null = null;
  get(identity: string, key: string): DatabaseBackupIntent {
    let cell = this.cells.get(key);
    if (!cell || cell.snapshot().retired) { cell = new DatabaseBackupIntent(identity, key); this.cells.set(key, cell); }
    return cell;
  }
  confirm(identity: string | null) {
    for (const [key, cell] of this.cells) if (cell.identity !== identity) { cell.retire(); this.cells.delete(key); if (this.active === key) this.active = null; }
  }
  activate(key: string) {
    if (this.active && this.active !== key) { this.cells.get(this.active)?.retire(); this.cells.delete(this.active); }
    this.active = key;
  }
  selectProject(identity: string, customer: string, project: string) {
    const prefix = JSON.stringify([identity, customer, project]).slice(0, -1) + ",";
    // A gate remount resets the dropdown, not the retained resource's actual scope.
    if (this.active && !this.active.startsWith(prefix)) this.retireActive();
  }
  retireActive() { if (this.active) { this.cells.get(this.active)?.retire(); this.cells.delete(this.active); } this.active = null; }
}
