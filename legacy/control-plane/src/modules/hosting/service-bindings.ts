import { createHostingServiceBindings } from "@webdock/module-hosting/server/service-bindings";
import { applicationCapabilities } from "./setup-capabilities";
import {
  mailBindingMaterial,
  revokeBindingMail,
  bindingMailReady,
} from "./mail-binding";
import { authorizeHosting } from "./authorization";
import { transaction, planLock, audit } from "./db";
import { replay, recordResult } from "./operations";
import { openEnvironment, sealEnvironment } from "./environment";
import { encryptMailSecret } from "../platform/platform";
import { databaseBindingMaterial } from "./database-binding";
import { openBinding } from "./binding-runtime";
import { executeAppCommand } from "./apps";
export const { executeServiceBinding, readServiceBindings } =
  createHostingServiceBindings({
    get applicationCapabilities() {
      return applicationCapabilities;
    },
    get mailBindingMaterial() {
      return mailBindingMaterial;
    },
    get revokeBindingMail() {
      return revokeBindingMail;
    },
    get bindingMailReady() {
      return bindingMailReady;
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
    get audit() {
      return audit;
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
    get sealEnvironment() {
      return sealEnvironment;
    },
    get encryptMailSecret() {
      return encryptMailSecret;
    },
    get databaseBindingMaterial() {
      return databaseBindingMaterial;
    },
    get openBinding() {
      return openBinding;
    },
    get executeAppCommand() {
      return executeAppCommand;
    },
  });
