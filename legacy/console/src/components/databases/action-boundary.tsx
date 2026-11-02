"use client";
import { createContext, useContext, type ReactNode } from "react";
import { DatabaseActionsProvider } from "@webdock/module-databases/ui/actions";
import {
  prepareDatabaseOffer,
  acceptDatabaseOffer,
  createDatabasePlan,
  revealDatabaseCredentials,
} from "@/lib/managed-database-actions";
import {
  saveDatabase,
  launchDatabase,
  launchManagedDatabase,
} from "@/lib/database-actions";
import { launchDatabaseWorkspace } from "@/lib/database-launch";
const AdapterDatabaseLaunch = createContext(false);
export function DatabaseLaunchProvider({
  adapter,
  children,
}: {
  adapter: boolean;
  children: ReactNode;
}) {
  return (
    <AdapterDatabaseLaunch value={adapter}>{children}</AdapterDatabaseLaunch>
  );
}
const actions = {
  prepareDatabaseOffer,
  acceptDatabaseOffer,
  createDatabasePlan,
  revealDatabaseCredentials,
  saveDatabase,
  launchDatabase: (id: string) => launchDatabaseWorkspace(id),
  launchManagedDatabase: (id: string) => launchDatabaseWorkspace(id, true),
};
export function DatabaseActionBoundary({ children }: { children: ReactNode }) {
  const adapter = useContext(AdapterDatabaseLaunch);
  return (
    <DatabaseActionsProvider
      actions={
        adapter
          ? { ...actions, launchDatabase, launchManagedDatabase }
          : actions
      }
    >
      {children}
    </DatabaseActionsProvider>
  );
}
