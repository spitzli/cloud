import { createHostingGitWorkerApi } from "@webdock/module-hosting/server/git-worker-api";
import { withDatabaseScope } from "@webdock/database-runtime";
import { authDatabase } from "../../db/runtime";
import { transaction } from "./db";
import { readHostingJSON, requireHostingEnvironment } from "./http";
import {
  authenticateGitWorker,
  claimGitBuild,
  completeGitBuild,
  claimGitRelease,
  completeGitRelease,
  inspectGitBuildLease,
  prepareGitActionsArtifact,
  processGitEvents,
  processGitChecks,
} from "./git-deployments";
import { createGitHubProvider } from "./git-github";
import {
  prepareGitVercelBuild,
  resolveGitVercelCredential,
} from "./git-vercel";
import {
  prepareGitPublication,
  publishGitContainer,
  observeGitPublication,
  recordGitVercelPublication,
} from "./git-publication";
export const { gitWorkerAPI } = createHostingGitWorkerApi({
  get withDatabaseScope() {
    return withDatabaseScope;
  },
  get authDatabase() {
    return authDatabase;
  },
  get transaction() {
    return transaction;
  },
  get readHostingJSON() {
    return readHostingJSON;
  },
  get requireHostingEnvironment() {
    return requireHostingEnvironment;
  },
  get authenticateGitWorker() {
    return authenticateGitWorker;
  },
  get claimGitBuild() {
    return claimGitBuild;
  },
  get completeGitBuild() {
    return completeGitBuild;
  },
  get claimGitRelease() {
    return claimGitRelease;
  },
  get completeGitRelease() {
    return completeGitRelease;
  },
  get inspectGitBuildLease() {
    return inspectGitBuildLease;
  },
  get prepareGitActionsArtifact() {
    return prepareGitActionsArtifact;
  },
  get processGitEvents() {
    return processGitEvents;
  },
  get processGitChecks() {
    return processGitChecks;
  },
  get createGitHubProvider() {
    return createGitHubProvider;
  },
  get prepareGitVercelBuild() {
    return prepareGitVercelBuild;
  },
  get resolveGitVercelCredential() {
    return resolveGitVercelCredential;
  },
  get prepareGitPublication() {
    return prepareGitPublication;
  },
  get publishGitContainer() {
    return publishGitContainer;
  },
  get observeGitPublication() {
    return observeGitPublication;
  },
  get recordGitVercelPublication() {
    return recordGitVercelPublication;
  },
});
