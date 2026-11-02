import { createHostingGitDeployments } from "@webdock/module-hosting/server/git-deployments";
import { reconcileGitRelease } from "./git-reconciliation";
import { bindGitTarget } from "./git-targets";
import {
  openEnvironment,
  sealEnvironment,
  patchEnvironment,
  redactValues,
} from "./environment";
import { authorizeHosting } from "./authorization";
import { transaction, audit } from "./db";
import { createGitHubProvider } from "./git-github";
export type { GitBuildCompletion } from "@webdock/module-hosting/server/git-deployments";
export const {
  gitEnvironmentSnapshot,
  registerGitConnection,
  executeGitDeployment,
  enrollGitWorker,
  authenticateGitWorker,
  claimGitBuild,
  prepareGitActionsArtifact,
  completeGitBuild,
  receiveGitEvent,
  processGitEvents,
  claimGitRelease,
  completeGitRelease,
  inspectGitBuildLease,
  processGitChecks,
} = createHostingGitDeployments({
  get reconcileGitRelease() {
    return reconcileGitRelease;
  },
  get bindGitTarget() {
    return bindGitTarget;
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
  get redactValues() {
    return redactValues;
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
  get createGitHubProvider() {
    return createGitHubProvider;
  },
  env: process.env,
});
