import {
  nativeClusterDemand,
  clusterCapacityDemand,
  databasePoolCapacity,
} from "@spitzli/mail-core/capacity";
import { createHostingDatabaseOrders } from "@spitzli/module-databases/server/database-orders";
import { assertDatabaseMaintenanceAllowed } from "./database-protection";
import { authorizeHosting } from "./authorization";
import { clusterLock, audit } from "./db";
import { replay, recordResult } from "./operations";
import { encryptMailSecret } from "../platform/platform";
export const {
  freshDatabaseCluster,
  databaseCredentials,
  enqueueDatabase,
  createDatabaseBackend,
  acceptDatabaseOrder,
} = createHostingDatabaseOrders({
  get nativeClusterDemand() {
    return nativeClusterDemand;
  },
  get clusterCapacityDemand() {
    return clusterCapacityDemand;
  },
  get databasePoolCapacity() {
    return databasePoolCapacity;
  },

  get assertDatabaseMaintenanceAllowed() {
    return assertDatabaseMaintenanceAllowed;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get clusterLock() {
    return clusterLock;
  },
  get audit() {
    return audit;
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
  env: process.env,
});
