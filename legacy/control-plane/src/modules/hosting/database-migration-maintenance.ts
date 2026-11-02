import { createHostingDatabaseMigrationMaintenance } from "@webdock/module-databases/server/database-migration-maintenance";
import { authorizeHosting } from "./authorization";
import { databaseAccess } from "./databases";
import { audit, clusterLock } from "./db";
import { replay, recordResult } from "./operations";
import { enqueueDatabase } from "./database-orders";
export const { migrationMaintenance } =
  createHostingDatabaseMigrationMaintenance({
    get authorizeHosting() {
      return authorizeHosting;
    },
    get databaseAccess() {
      return databaseAccess;
    },
    get audit() {
      return audit;
    },
    get clusterLock() {
      return clusterLock;
    },
    get replay() {
      return replay;
    },
    get recordResult() {
      return recordResult;
    },
    get enqueueDatabase() {
      return enqueueDatabase;
    },
  });
