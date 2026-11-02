import { setupProductAvailable } from "./product-admission";
import { createHostingSetupSource } from "@webdock/module-hosting/server/setup-source";
import { setupBuildCapabilities } from "./setup-capabilities";
import { readSetup } from "./project-setup";
import { createGitHubProvider } from "./git-github";
import { transaction } from "./db";
import { authorizeHosting } from "./authorization";
import { executeGitDeployment } from "./git-deployments";
import { executeDatabaseCommand } from "./databases";
export const { executeSetupSource } = createHostingSetupSource({
  setupProductAvailable,
  get setupBuildCapabilities() {
    return setupBuildCapabilities;
  },
  get readSetup() {
    return readSetup;
  },
  get createGitHubProvider() {
    return createGitHubProvider;
  },
  get transaction() {
    return transaction;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get executeGitDeployment() {
    return executeGitDeployment;
  },
  get executeDatabaseCommand() {
    return executeDatabaseCommand;
  },
  env: process.env,
});
