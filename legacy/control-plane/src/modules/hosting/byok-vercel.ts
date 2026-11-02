import {
  fetchVercelProject,
  listVercelProjects,
} from "@webdock/hosting-contracts/vercel";
import { createHostingByokVercel } from "@webdock/module-hosting/server/byok-vercel";
import { createVercelJSONTransport } from "@webdock/adapter-hosting-vercel/transport";
import { encryptMailSecret, decryptMailSecret } from "../platform/platform";
import { transaction, audit, planLock } from "./db";
import { byokAccess } from "./byok";
export type { VercelRuntime } from "@webdock/module-hosting/server/byok-vercel";
export const { vercelJSON, executeByokVercel } = createHostingByokVercel({
  get fetchVercelProject() {
    return fetchVercelProject;
  },
  get listVercelProjects() {
    return listVercelProjects;
  },

  get createVercelJSONTransport() {
    return createVercelJSONTransport;
  },
  get encryptMailSecret() {
    return encryptMailSecret;
  },
  get decryptMailSecret() {
    return decryptMailSecret;
  },
  get transaction() {
    return transaction;
  },
  get audit() {
    return audit;
  },
  get planLock() {
    return planLock;
  },
  get byokAccess() {
    return byokAccess;
  },
  env: process.env,
});
