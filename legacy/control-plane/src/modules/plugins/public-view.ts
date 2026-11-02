import type { Pool } from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import type { ModuleView } from "@webdock/module-sdk";
import { readInstalledPlugins } from "./store";
/** Public metadata only. Call admission at the actual durable SQL boundary. */
export async function readPublicInstallation(
  db?: Pick<Pool, "query">,
): Promise<ModuleView> {
  if (!db) {
    const { workerDatabase } = await import("../../db/runtime");
    return withDatabaseScope(
      { kind: "system", purpose: "plugin-public-view" },
      () => readPublicInstallation(workerDatabase),
    );
  }
  const installed = await readInstalledPlugins(db);
  return {
    revision: JSON.stringify(installed.map((p) => [p.id, p.revision, p.state])),
    modules: Object.fromEntries(
      installed.filter((p) => p.kind === "builtin").map((p) => [p.id, p.state]),
    ),
  };
}
