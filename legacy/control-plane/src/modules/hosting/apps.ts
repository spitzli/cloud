import { createHostingApps } from "@webdock/module-hosting/server/apps";
import { applicationCapabilities } from "./setup-capabilities";
import { revokeBindingMail } from "./mail-binding";
import { bindingRuntime } from "./binding-runtime";
import { authorizeHosting } from "./authorization";
import { transaction, planLock, clusterLock, audit } from "./db";
import { checkDemand } from "./reservations";
import { replay, recordResult } from "./operations";
import {
  openEnvironment,
  patchEnvironment,
  sealEnvironment,
} from "./environment";
export const { safeApp, executeAppCommand } = createHostingApps({
  get applicationCapabilities() {
    return applicationCapabilities;
  },
  get revokeBindingMail() {
    return revokeBindingMail;
  },
  get bindingRuntime() {
    return bindingRuntime;
  },
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
  get checkDemand() {
    return checkDemand;
  },
  get replay() {
    return replay;
  },
  get recordResult() {
    return recordResult;
  },
  get openEnvironment() {
    return openEnvironment;
  },
  get patchEnvironment() {
    return patchEnvironment;
  },
  get sealEnvironment() {
    return sealEnvironment;
  },
});
