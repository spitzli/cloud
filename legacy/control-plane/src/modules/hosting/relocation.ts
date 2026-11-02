import { createHostingRelocation } from "@webdock/module-hosting/server/relocation";
import { authorizeHosting } from "./authorization";
import { transaction, planLock, clusterLock, audit } from "./db";
import { clusterView } from "./clusters";
import { replay, recordResult } from "./operations";
export const { relocateProject } = createHostingRelocation({
  get authorizeHosting() {
    return authorizeHosting;
  },
  get transaction() {
    return transaction;
  },
  get planLock() {
    return planLock;
  },
  get clusterLock() {
    return clusterLock;
  },
  get audit() {
    return audit;
  },
  get clusterView() {
    return clusterView;
  },
  get replay() {
    return replay;
  },
  get recordResult() {
    return recordResult;
  },
});
