import { createHostingDatabaseMaintenance } from "@webdock/module-databases/server/database-maintenance";
import { assertDatabaseMaintenanceAllowed } from "./database-protection";
import { databaseAccess, databaseView } from "./databases";
import { clusterLock, audit } from "./db";
import {
  enqueueDatabase,
  databaseCredentials,
  freshDatabaseCluster,
} from "./database-orders";
import { replay, recordResult } from "./operations";
import { encryptMailSecret } from "../platform/platform";
export const { databaseMaintenance } = createHostingDatabaseMaintenance({
  get assertDatabaseMaintenanceAllowed() {
    return assertDatabaseMaintenanceAllowed;
  },
  get databaseAccess() {
    return databaseAccess;
  },
  get databaseView() {
    return databaseView;
  },
  get clusterLock() {
    return clusterLock;
  },
  get audit() {
    return audit;
  },
  get enqueueDatabase() {
    return enqueueDatabase;
  },
  get databaseCredentials() {
    return databaseCredentials;
  },
  get freshDatabaseCluster() {
    return freshDatabaseCluster;
  },
  get replay() {
    return replay;
  },
  get recordResult() {
    return recordResult;
  },
  get encryptSecret() {
    return encryptMailSecret;
  },
});
