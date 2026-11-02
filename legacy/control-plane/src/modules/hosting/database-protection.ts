import { createHostingDatabaseProtection } from "@webdock/module-databases/server/database-protection";

export const { assertDatabaseMaintenanceAllowed } =
  createHostingDatabaseProtection({ env: process.env });
