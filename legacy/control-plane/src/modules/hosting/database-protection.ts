import { createHostingDatabaseProtection } from "@spitzli/module-databases/server/database-protection";

export const { assertDatabaseMaintenanceAllowed } =
  createHostingDatabaseProtection({ env: process.env });
