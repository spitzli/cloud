import { createContext, useContext, useLayoutEffect, useMemo, useState, useSyncExternalStore, type ReactNode } from "react";
import type { Session } from "./api";
import { useApi, useSession } from "./context";
import { DatabaseBackupIntents } from "./database-backup-intent";
const IntentContext = createContext<DatabaseBackupIntents | null>(null);
export function DatabaseBackupIntentProvider({ children }: { children: ReactNode }) {
  const [intents] = useState(() => new DatabaseBackupIntents());
  return <IntentContext.Provider value={intents}>{children}</IntentContext.Provider>;
}
function useIntents() { const value = useContext(IntentContext); if (!value) throw new Error("Database backup intent provider is unavailable"); return value; }
function identity(platform: string, session: Session): string { return JSON.stringify([platform, session.user.id, session.session.userId, session.session.id]); }
/** Undefined is loading/unknown; null is confirmed logout. Role/UI versions are not identity. */
export function useBackupIntentSession(session: Session | null | undefined) {
  const intents = useIntents(), api = useApi();
  const key = session === undefined ? undefined : session === null ? null : identity(api.config.apiOrigin, session);
  useLayoutEffect(() => { if (key !== undefined) intents.confirm(key); }, [intents, key]);
}
export function useBackupRequestMemory(customer: string, project: string, resource: string) {
  const intents = useIntents(), session = useSession(), api = useApi();
  const principal = identity(api.config.apiOrigin, session);
  const key = JSON.stringify([principal, customer, project, resource]);
  const cell = useMemo(() => intents.get(principal, key), [intents, principal, key]);
  useLayoutEffect(() => { intents.confirm(principal); intents.activate(key); }, [intents, principal, key]);
  const snapshot = useSyncExternalStore(cell.subscribe, cell.snapshot, cell.snapshot);
  return { cell, snapshot };
}
export type BackupRequestMemory = ReturnType<typeof useBackupRequestMemory>;
export function useRetireBackupResource() { const intents = useIntents(); return () => { intents.retireActive(); }; }
export function useBackupProjectSelection() {
  const intents = useIntents(), session = useSession(), api = useApi();
  const principal = identity(api.config.apiOrigin, session);
  return (customer: string, project: string) => { intents.selectProject(principal, customer, project); };
}
