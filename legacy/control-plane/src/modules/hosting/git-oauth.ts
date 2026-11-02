import { createHostingGitOauth } from "@webdock/module-hosting/server/git-oauth";
import { withHostingDatabase } from "../../db/database-context";
import { encryptMailSecret, decryptMailSecret } from "../platform/platform";
import { authorizeHosting } from "./authorization";
import { transaction } from "./db";
import { createGitHubProvider } from "./git-github";
import { registerGitConnection } from "./git-deployments";
export const { gitOAuthSettings, executeGitOAuth } = createHostingGitOauth({
  get withHostingDatabase() {
    return withHostingDatabase;
  },
  get encryptMailSecret() {
    return encryptMailSecret;
  },
  get decryptMailSecret() {
    return decryptMailSecret;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get transaction() {
    return transaction;
  },
  get createGitHubProvider() {
    return createGitHubProvider;
  },
  get registerGitConnection() {
    return registerGitConnection;
  },
  env: process.env,
});
