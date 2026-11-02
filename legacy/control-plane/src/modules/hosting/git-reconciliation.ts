import { createHostingGitReconciliation } from "@webdock/module-hosting/server/git-reconciliation";
import { authorizeHosting } from "./authorization";
import { transaction, audit } from "./db";
import { resolveGitVercelCredential } from "./git-vercel";
import {
  createGitVercelObserver,
  gitReleaseHealthPath,
} from "./git-vercel-publication";
export const { reconcileGitRelease } = createHostingGitReconciliation({
  get authorizeHosting() {
    return authorizeHosting;
  },
  get transaction() {
    return transaction;
  },
  get audit() {
    return audit;
  },
  get resolveGitVercelCredential() {
    return resolveGitVercelCredential;
  },
  get createGitVercelObserver() {
    return createGitVercelObserver;
  },
  get gitReleaseHealthPath() {
    return gitReleaseHealthPath;
  },
});
