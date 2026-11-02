import { createHostingByok } from "@webdock/module-hosting/server/byok";
import { executeByokVercel } from "./byok-vercel";
import { executeByokMail } from "./byok-mail";
import { authorizeHosting } from "./authorization";
import { transaction, audit, planLock, clusterLock } from "./db";
import { clusterView, verifyAgent } from "./clusters";
import { replay, recordResult } from "./operations";
export const {
  byokPolicy,
  byokAccess,
  byokCluster,
  executeByok,
  saveInventory,
  byokAgentConfig,
  claimByokOperation,
  completeByokOperation,
} = createHostingByok({
  get executeByokVercel() {
    return executeByokVercel;
  },
  get executeByokMail() {
    return executeByokMail;
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
  get clusterView() {
    return clusterView;
  },
  get verifyAgent() {
    return verifyAgent;
  },
  get replay() {
    return replay;
  },
  get recordResult() {
    return recordResult;
  },
});
