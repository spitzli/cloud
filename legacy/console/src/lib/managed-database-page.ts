import "server-only";
import { cache } from "react";
import type { DatabaseView } from "@webdock/hosting-contracts";
import { hostingPageCall } from "./hosting-client";
import { getHostingProject } from "./hosting-project";

export const getManagedDatabasePage = cache(async (databaseID: string) => {
  const path = `/hosting/databases/${databaseID}`;
  const database = await hostingPageCall<DatabaseView>(
    { action: "databases.get", databaseID },
    path,
  );
  const [access, project] = await Promise.all([
    hostingPageCall<{
      canWrite: boolean;
      operator: boolean;
    }>({ action: "databases.permissions", projectID: database.projectID }, path),
    getHostingProject(database.projectID),
  ]);
  return { database, access, project, path };
});
