import { introspectToken } from "../auth/introspection";
import { createHostingBridge } from "@webdock/module-hosting/server/bridge";
import { requireHostingCommand } from "../../installation";
import { vercelSettings } from "../integrations/vercel-settings";
import { displayDatabaseCredentials } from "./database-credentials";
import { executeGitOAuth } from "./git-oauth";
import { authDatabase as database } from "../../db/runtime";
import { mcpResource, currentHostingMCPClaims } from "../auth/mcp";
import { executeHosting } from "./service";
import { issueEnrollment } from "./clusters";
import { readHostingJSON, requireHostingEnvironment } from "./http";
export const { hostingIdentity, executeHostingBridgeCommand, hostingBridge } =
  createHostingBridge({
    introspectToken,
    get requireHostingCommand() {
      return requireHostingCommand;
    },
    get vercelSettings() {
      return vercelSettings;
    },
    get displayDatabaseCredentials() {
      return displayDatabaseCredentials;
    },
    get executeGitOAuth() {
      return executeGitOAuth;
    },
    get database() {
      return database;
    },
    get mcpResource() {
      return mcpResource;
    },
    get currentHostingMCPClaims() {
      return currentHostingMCPClaims;
    },
    get executeHosting() {
      return executeHosting;
    },
    get issueEnrollment() {
      return issueEnrollment;
    },
    get readHostingJSON() {
      return readHostingJSON;
    },
    get requireHostingEnvironment() {
      return requireHostingEnvironment;
    },
    env: process.env,
  });

export const executeStudioHosting = executeHostingBridgeCommand;
