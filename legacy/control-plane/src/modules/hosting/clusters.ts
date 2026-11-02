import { createHostingClusters } from "@webdock/module-hosting/server/clusters";
import { authDatabase } from "../../db/runtime";
import { withHostingDatabase } from "../../db/database-context";
import { byokCluster, byokPolicy } from "./byok";
import { authorizeHosting } from "./authorization";
import { transaction, audit, clusterLock } from "./db";
export type { Agent } from "@webdock/module-hosting/server/clusters";
export const {
  verifyAgent,
  authenticateAgent,
  issueEnrollment,
  consumeEnrollment,
  reportCluster,
  clusterView,
} = createHostingClusters({
  get authDatabase() {
    return authDatabase;
  },
  get withHostingDatabase() {
    return withHostingDatabase;
  },
  get byokCluster() {
    return byokCluster;
  },
  get byokPolicy() {
    return byokPolicy;
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
  env: process.env,
});
