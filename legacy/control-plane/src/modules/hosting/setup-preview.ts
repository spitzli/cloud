import { authorizeSetupConfiguration } from "./permissions";
import { requireDatabases } from "./product-admission";
import { requireMail } from "../../installation";
export { assertSetupConfirmation } from "@webdock/module-hosting/server/setup-confirmation";
import { createHostingSetupPreview } from "@webdock/module-hosting/server/setup-preview";
import {
  applicationCapabilities,
  setupBuildCapabilities,
} from "./setup-capabilities";
import { transaction } from "./db";
import { authorizeHosting } from "./authorization";
import { checkDemand } from "./reservations";
import { executeDatabaseCommand } from "./databases";
import { createGitHubProvider } from "./git-github";
import { applicationMailRoute } from "./mail-binding";
export const { previewProjectSetup } = createHostingSetupPreview({
  authorizeSetupConfiguration,
  requireDatabases,
  requireMail,
  get applicationCapabilities() {
    return applicationCapabilities;
  },
  get setupBuildCapabilities() {
    return setupBuildCapabilities;
  },
  get transaction() {
    return transaction;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get checkDemand() {
    return checkDemand;
  },
  get executeDatabaseCommand() {
    return executeDatabaseCommand;
  },
  get createGitHubProvider() {
    return createGitHubProvider;
  },
  get applicationMailRoute() {
    return applicationMailRoute;
  },
  env: process.env,
});
