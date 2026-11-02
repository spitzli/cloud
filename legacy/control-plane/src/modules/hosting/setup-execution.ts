import {
  authorizePluginSetup,
  beforePluginSetupRelease,
  beforePluginSetupReady,
} from "./plugin-provisioning-runtime";
import { authorizeSetupConfiguration } from "./permissions";
import { createHostingSetupExecution } from "@webdock/module-hosting/server/setup-execution";
import { transaction } from "./db";
import { replay } from "./operations";
import { authorizeHosting } from "./authorization";
import { executeAppCommand } from "./apps";
import { executeDatabaseCommand } from "./databases";
import { executeServiceBinding } from "./service-bindings";
import { executeGitDeployment } from "./git-deployments";
import { openEnvironment } from "./environment";
export const { executeSetupStep } = createHostingSetupExecution({
  authorizeSetupConfiguration,
  authorizeProvisioning: authorizePluginSetup,
  beforeRelease: beforePluginSetupRelease,
  beforeReady: beforePluginSetupReady,
  get transaction() {
    return transaction;
  },
  get replay() {
    return replay;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get executeAppCommand() {
    return executeAppCommand;
  },
  get executeDatabaseCommand() {
    return executeDatabaseCommand;
  },
  get executeServiceBinding() {
    return executeServiceBinding;
  },
  get executeGitDeployment() {
    return executeGitDeployment;
  },
  get openEnvironment() {
    return openEnvironment;
  },
});
