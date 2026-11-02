"use server";

import { revalidatePath } from "next/cache";
import { databaseCommand, type DatabaseLaunch } from "@webdock/database-contracts";
import { HostingError } from "@webdock/hosting-contracts";
import { databaseCall } from "./database-client";

export async function saveDatabase(_state: { error?: string; message?: string }, form: FormData): Promise<{ error?: string; message?: string }> {
  const value = (name: string) => typeof form.get(name) === "string" ? String(form.get(name)) : "";
  const action = value("action"), bindingID = value("bindingID");
  const input = action === "create" ? { action, engine: value("engine") || "postgresql", projectID: value("projectID"), name: value("name"), environment: value("environment") } :
    action === "grant" ? { action, bindingID, subject: value("subject"), profile: value("profile"), runtimeOrigin: value("runtimeOrigin"),
      connectionID: value("connectionID"), proxySecret: value("proxySecret"), isolationVerified: value("isolationVerified") === "on", databaseRoleVerified: value("databaseRoleVerified") === "on" } :
    action === "revoke" ? { action, bindingID, subject: value("subject") } : { action, bindingID };
  const parsed = databaseCommand.safeParse(input);
  if (!parsed.success || ["get", "list", "open", "openManaged"].includes(parsed.data.action)) return { error: "Check the database configuration fields." };
  try {
    await databaseCall(parsed.data);
    revalidatePath("/tenants", "layout");
    return { message: "Database configuration saved." };
  } catch (error) { return { error: error instanceof HostingError ? error.message : "Database configuration could not be saved." }; }
}

export async function launchDatabase(bindingID: string): Promise<{ launch?: DatabaseLaunch; error?: string }> {
  try { return { launch: await databaseCall<DatabaseLaunch>({ action: "open", bindingID }) }; }
  catch (error) { return { error: error instanceof HostingError ? error.message : "Database access is unavailable." }; }
}

export async function launchManagedDatabase(databaseID: string): Promise<{ launch?: DatabaseLaunch; error?: string }> {
  try { return { launch: await databaseCall<DatabaseLaunch>({ action: "openManaged", databaseID }) }; }
  catch (error) { return { error: error instanceof HostingError ? error.message : "Database access is unavailable." }; }
}
