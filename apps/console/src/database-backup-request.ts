import { useLayoutEffect, useRef } from "react";
import { z } from "zod";
import { databaseCommandSchema } from "@webdock/hosting-contracts";
import { ApiError } from "./api";
import { useApi } from "./context";
import type { BackupRequestMemory } from "./database-backup-intents";
import type { BackupMutation, BackupAttempt, DeliveryToken, KnownBackupPolicy, PolicyExpectation } from "./database-backup-intent";
import { backupReceiptExpectation, decodeBackupReceipt } from "./database-backup-receipt";
export type { BackupRequestMemory } from "./database-backup-intents";
function validateDestination(command: Extract<BackupMutation, { action: "databases.backupDestinations.register" }>) {
        const endpoint = new URL(command.endpointURL);
        if (!/^https:\/\/[a-zA-Z0-9.-]+(?::[0-9]+)?\/?$/.test(command.endpointURL) || endpoint.host.length > 253 || endpoint.username || endpoint.password || endpoint.search || endpoint.hash || endpoint.pathname !== "/" || !/^[a-zA-Z0-9-]{1,63}$/.test(command.region) || !/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(command.bucket) || command.writeCredentials.accessKeyId === command.recoveryCredentials.accessKeyId) throw new Error("Invalid destination fields");
}

function parseCommand(input: unknown, key: string): BackupMutation {
  const command = databaseCommandSchema.parse({ ...(input as Record<string, unknown>), idempotencyKey: key });
  if (command.action === "databases.backupDestinations.register") { validateDestination(command); return command; }
  if (command.action === "databases.backupPolicies.grant" || command.action === "databases.backupPolicies.update" || command.action === "databases.backupPolicies.revoke") return command;
  if (command.action === "databases.backup") return { ...command, action: "databases.backup" };
  throw new Error("Unsupported backup action");
}
function errorKind(error: unknown) {
  if (error instanceof ApiError) {
    if (error.status === 401 || error.status === 403) return "forbidden";
    if (error.status === 409) return "conflict";
    if (error.status === 400) return "fields";
  }
  return "uncertain";
}
async function prepareAttempt(input: unknown, prior: BackupAttempt | null, known: KnownBackupPolicy | null, allowed: (target: PolicyExpectation | null) => boolean, target: PolicyExpectation | null) {
  const key = prior?.key ?? crypto.randomUUID(), command = parseCommand(input, key);
  if (command.action === "databases.backup" && !allowed(target)) throw new Error("Backup observation is not current");
  const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(JSON.stringify(command))))].map(b => b.toString(16).padStart(2, "0")).join("");
  const metadata: BackupAttempt = { key, digest, action: command.action, publicCommand: command.action === "databases.backupDestinations.register" ? null : command, uncertain: prior?.uncertain ?? false, expected: prior?.expected ?? backupReceiptExpectation(command, known) };
  return { command, metadata };
}
/** A disposable view/transport attached to application-lifetime NONSECRET intent. */
export function useBackupRequest(memory: BackupRequestMemory, writable: boolean, expiresAt: string, observed: () => void, manualAllowed: (target: PolicyExpectation | null) => boolean, known: KnownBackupPolicy | null) {
  const api = useApi(), { cell, snapshot } = memory;
  const activeRef = useRef<{ controller: AbortController; token: DeliveryToken | null } | null>(null), mountedRef = useRef(false);
  const manualAdmissionRef = useRef(manualAllowed);
  useLayoutEffect(() => { manualAdmissionRef.current = manualAllowed; }, [manualAllowed]);
  useLayoutEffect(() => {
    mountedRef.current = true; cell.attach();
    return () => {
      mountedRef.current = false;
      if (activeRef.current?.token) cell.abort(activeRef.current.token);
      activeRef.current?.controller.abort(); cell.detach();
    };
  }, [cell]);
  useLayoutEffect(() => { const active = activeRef.current; if (active && (!writable || active.token && !cell.current(active.token))) active.controller.abort(); }, [cell, writable, snapshot.generation, snapshot.epoch]);
  const send = async (input: unknown) => {
    if (!mountedRef.current || !writable || Date.parse(expiresAt) <= Date.now() || activeRef.current || !cell.canStart()) return;
    const active = { controller: new AbortController(), token: null as DeliveryToken | null }; activeRef.current = active;
    const attached = () => mountedRef.current && activeRef.current === active && !active.controller.signal.aborted && Date.parse(expiresAt) > Date.now() && !cell.snapshot().retired;
    active.controller.signal.addEventListener("abort", () => { if (active.token) cell.abort(active.token); }, { once: true });
    try {
      const { command, metadata } = await prepareAttempt(input, cell.snapshot().attempt, known, manualAllowed, cell.snapshot().policyTarget);
      if (!attached()) return;
      if (command.action === "databases.backup" && !manualAdmissionRef.current(cell.snapshot().policyTarget)) throw new Error("Backup observation is no longer current");
      active.token = cell.begin(metadata);
      if (!active.token) return;
      const value = await api.request("/api/console/databases", z.unknown(), { method: "POST", body: JSON.stringify(command), signal: active.controller.signal });
      if (!attached() || !cell.current(active.token)) return;
      const expected = cell.snapshot().attempt?.expected;
      if (!expected) return;
      const receipt = decodeBackupReceipt(value, expected, known);
      cell.accept(active.token, receipt); observed();
    } catch (error) {
      if (!attached()) return;
      if (active.token) cell.fail(active.token, errorKind(error)); else cell.localFailure("fields");
    } finally { if (activeRef.current === active) activeRef.current = null; }
  };
  return { state: snapshot.state, policyTarget: snapshot.policyTarget, refreshPending: snapshot.refreshPending, send,
    blocked: snapshot.retired || snapshot.readDenied || snapshot.writeDenied || snapshot.refreshPending || ["pending", "uncertain", "rejected", "forbidden"].includes(snapshot.state.status),
    retry() { const command = cell.snapshot().attempt?.publicCommand; if (command) void send(command); },
    beginRefresh: (automatic = false) => cell.beginRefresh(automatic),
    refreshCurrent: (token: Parameters<typeof cell.refreshCurrent>[0]) => cell.refreshCurrent(token),
    finishRefresh: (token: Parameters<typeof cell.finishRefresh>[0], valid: boolean) => cell.finishRefresh(token, valid),
  };
}
