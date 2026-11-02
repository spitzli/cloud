import { authorizeSetupConfiguration } from "./permissions";
import { requireDatabases } from "./product-admission";
import { requireMail } from "../../installation";
import { createHostingProjectSetup } from "@webdock/module-hosting/server/project-setup";
import { executeSetupSource } from "./setup-source";
import { repairSetupEnvironment, resetGitSetupRetry } from "./setup-recovery";
import { executeAppCommand } from "./apps";
import { transaction, audit } from "./db";
import { authorizeHosting } from "./authorization";
import { replay, recordResult } from "./operations";
import {
  openEnvironment,
  sealEnvironment,
  patchEnvironment,
} from "./environment";
import { previewProjectSetup, assertSetupConfirmation } from "./setup-preview";
export type { SetupRow } from "@webdock/module-hosting/server/project-setup";
export const { setupView, readSetup, executeProjectSetup } =
  createHostingProjectSetup({
    authorizeSetupConfiguration,
    requireDatabases,
    requireMail,
    get executeSetupSource() {
      return executeSetupSource;
    },
    get repairSetupEnvironment() {
      return repairSetupEnvironment;
    },
    get resetGitSetupRetry() {
      return resetGitSetupRetry;
    },
    get executeAppCommand() {
      return executeAppCommand;
    },
    get transaction() {
      return transaction;
    },
    get audit() {
      return audit;
    },
    get authorizeHosting() {
      return authorizeHosting;
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
    get patchEnvironment() {
      return patchEnvironment;
    },
    get previewProjectSetup() {
      return previewProjectSetup;
    },
    get assertSetupConfirmation() {
      return assertSetupConfirmation;
    },
    env: process.env,
  });
