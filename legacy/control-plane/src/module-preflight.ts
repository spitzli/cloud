import { withDatabaseScope } from "@webdock/database-runtime";
import { assertModuleCanDisable } from "./module-preflight-check";
import { coreLegacyRetentionChecks } from "./db/module-preflight-tables";
type InventoryReader = { query(sql: string, values?: unknown[]): Promise<{rows: Record<string, unknown>[]}> };
export async function assertCoreInventoryEmpty(db: InventoryReader) {
  const authorized = (await db.query("SELECT webdock_rls.worker(ARRAY['installation-preflight']::text[]) AS authorized")).rows[0];
  if (authorized?.authorized !== true) throw Error("Core inventory requires verified installation preflight read authority");
  for (const [module, checks] of Object.entries(coreLegacyRetentionChecks))
    await assertModuleCanDisable(db, module, checks);
}
async function verifyCoreInventory(_env: Record<string, string | undefined>) {
  const { workerDatabase } = await import("./db/runtime");
  await withDatabaseScope({kind:"system",purpose:"installation-preflight"}, () => assertCoreInventoryEmpty(workerDatabase));
}
let verify = verifyCoreInventory;
export function configureModulePreflight(check: typeof verify) { verify = check; }
export function verifyModuleShutdown(env: Record<string, string | undefined> = process.env) { return verify(env); }
