"use client";
import * as React from "react";
import { createContext, useContext } from "react";
import type { DatabaseOrderState, DatabaseCredentials } from "../ui-types";
import type { DatabaseLaunch } from "@webdock/database-contracts";
type FormState = { error?: string; message?: string };
export interface DatabaseUIActions {
  prepareDatabaseOffer(
    previous: DatabaseOrderState,
    form: FormData,
  ): Promise<DatabaseOrderState>;
  acceptDatabaseOffer(
    previous: DatabaseOrderState,
    form: FormData,
  ): Promise<DatabaseOrderState>;
  createDatabasePlan(previous: FormState, form: FormData): Promise<FormState>;
  revealDatabaseCredentials(
    id: string,
  ): Promise<{ error?: string; credentials?: DatabaseCredentials }>;
  saveDatabase(previous: FormState, form: FormData): Promise<FormState>;
  launchDatabase(
    id: string,
  ): Promise<{ launch?: Pick<DatabaseLaunch, "code" | "gatewayOrigin">; error?: string }>;
  launchManagedDatabase(
    id: string,
  ): Promise<{ launch?: Pick<DatabaseLaunch, "code" | "gatewayOrigin">; error?: string }>;
}
const Actions = createContext<DatabaseUIActions | null>(null);
export function DatabaseActionsProvider({
  actions,
  children,
}: {
  actions: DatabaseUIActions;
  children: React.ReactNode;
}) {
  return <Actions.Provider value={actions}>{children}</Actions.Provider>;
}
export function useDatabaseActions() {
  const actions = useContext(Actions);
  if (!actions) throw Error("Database UI action boundary is required");
  return actions;
}
