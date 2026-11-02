import { createHostingDatabases } from "@webdock/module-databases/server/databases";
import { withDatabaseScope } from "@webdock/database-runtime";
import { assertDatabaseMaintenanceAllowed } from "./database-protection";
import { migrationMaintenance } from "./database-migration-maintenance";
import { databaseMaintenance } from "./database-maintenance";
import {
  acceptDatabaseOrder,
  createDatabaseBackend,
  enqueueDatabase,
  freshDatabaseCluster,
} from "./database-orders";
import { authorizeHosting } from "./authorization";
import { transaction, audit, clusterLock } from "./db";
import { replay, recordResult } from "./operations";
export const { databaseView, databaseAccess, executeDatabaseCommand } =
  createHostingDatabases({
    get withDatabaseScope() {
      return withDatabaseScope;
    },
    get assertDatabaseMaintenanceAllowed() {
      return assertDatabaseMaintenanceAllowed;
    },
    get migrationMaintenance() {
      return migrationMaintenance;
    },
    get databaseMaintenance() {
      return databaseMaintenance;
    },
    get acceptDatabaseOrder() {
      return acceptDatabaseOrder;
    },
    get createDatabaseBackend() {
      return createDatabaseBackend;
    },
    get enqueueDatabase() {
      return enqueueDatabase;
    },
    get freshDatabaseCluster() {
      return freshDatabaseCluster;
    },
    get authorizeHosting() {
      return authorizeHosting;
    },
    get transaction() {
      return transaction;
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
    env: process.env,
  });
