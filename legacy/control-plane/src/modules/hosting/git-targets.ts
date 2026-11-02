import { createHostingGitTargets } from "@webdock/module-hosting/server/git-targets";
import { authorizeHosting } from "./authorization";
import { transaction, audit } from "./db";
import {
  resolveGitVercelCredential,
  prepareGitVercelBuild,
} from "./git-vercel";
export const { bindGitTarget } = createHostingGitTargets({
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
  get prepareGitVercelBuild() {
    return prepareGitVercelBuild;
  },
  env: process.env,
});
