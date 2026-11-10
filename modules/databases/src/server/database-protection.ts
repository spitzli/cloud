import type { DatabaseRuntimePorts } from "./ports";
import { HostingError } from "@spitzli/hosting-contracts";
export type Dependencies = {
  env: DatabaseRuntimePorts["env"];
};

export function createHostingDatabaseProtection(dependencies: Dependencies) {
  const offlineActions = new Set([
    "pause",
    "restart",
    "resize",
    "restore",
    "delete",
    "purge",
    "rotateCredentials",
    "requestCancellation",
    "upgrade",
  ]);
  /** The control plane cannot safely stop or move the database that owns its own job ledger. */
  function assertDatabaseMaintenanceAllowed(
    databaseID: string,
    action: string,
    protectedID = dependencies.env.WEBDOCK_CONTROL_DATABASE_ID,
  ) {
    if (
      protectedID &&
      databaseID === protectedID &&
      offlineActions.has(action.replace(/^databases\./, ""))
    )
      throw new HostingError(
        409,
        "This database hosts the Webdock control plane. Use the reviewed offline maintenance procedure.",
      );
  }
  return { assertDatabaseMaintenanceAllowed };
}
