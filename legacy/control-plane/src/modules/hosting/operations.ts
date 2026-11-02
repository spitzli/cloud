import { pluginOperationAuthority } from "./plugin-provisioning-store";
import { createHostingOperations } from "@webdock/module-hosting/server/operations";
import { revokeBindingMail } from "./mail-binding";
import { openEnvironment, redactValues } from "./environment";
import { transaction } from "./db";
import { verifyAgent } from "./clusters";
export const {
  transition,
  replay,
  recordResult,
  claimOperation,
  guardOperation,
  completeOperation,
} = createHostingOperations({
  operationAuthority: pluginOperationAuthority,
  get revokeBindingMail() {
    return revokeBindingMail;
  },
  get openEnvironment() {
    return openEnvironment;
  },
  get redactValues() {
    return redactValues;
  },
  get transaction() {
    return transaction;
  },
  get verifyAgent() {
    return verifyAgent;
  },
  env: process.env,
});
