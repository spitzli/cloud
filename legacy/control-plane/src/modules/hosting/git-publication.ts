import { createHostingGitPublication } from "@webdock/module-hosting/server/git-publication";
import {
  createGitVercelObserver,
  gitReleaseHealthPath,
} from "./git-vercel-publication";
import { authorizeHosting } from "./authorization";
import { transaction } from "./db";
import {
  authenticateGitWorker,
  gitEnvironmentSnapshot,
} from "./git-deployments";
import { createGitHubProvider } from "./git-github";
import {
  resolveGitVercelCredential,
  prepareGitVercelBuild,
} from "./git-vercel";
import { executeAppCommand } from "./apps";
import { openEnvironment } from "./environment";
export const {
  gitRegistryRepository,
  assertGitAutomaticSourceHead,
  prepareGitPublication,
  publishGitContainer,
  observeGitPublication,
  recordGitVercelPublication,
} = createHostingGitPublication({
  get createGitVercelObserver() {
    return createGitVercelObserver;
  },
  get gitReleaseHealthPath() {
    return gitReleaseHealthPath;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get transaction() {
    return transaction;
  },
  get authenticateGitWorker() {
    return authenticateGitWorker;
  },
  get gitEnvironmentSnapshot() {
    return gitEnvironmentSnapshot;
  },
  get createGitHubProvider() {
    return createGitHubProvider;
  },
  get resolveGitVercelCredential() {
    return resolveGitVercelCredential;
  },
  get prepareGitVercelBuild() {
    return prepareGitVercelBuild;
  },
  get executeAppCommand() {
    return executeAppCommand;
  },
  get openEnvironment() {
    return openEnvironment;
  },
  env: process.env,
});
