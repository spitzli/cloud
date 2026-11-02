import {
  nativeClusterDemand,
  clusterCapacityDemand,
} from "@webdock/mail-core/capacity";
import { createHostingReservations } from "@webdock/module-hosting/server/reservations";
import { authorizeHosting } from "./authorization";
import { transaction, audit, planLock, clusterLock } from "./db";
import { replay } from "./operations";
export const { reserveHosting, checkDemand } = createHostingReservations({
  get nativeClusterDemand() {
    return nativeClusterDemand;
  },
  get clusterCapacityDemand() {
    return clusterCapacityDemand;
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
  get planLock() {
    return planLock;
  },
  get clusterLock() {
    return clusterLock;
  },
  get replay() {
    return replay;
  },
  env: process.env,
});
