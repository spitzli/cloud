import { selectedIdentityDirectory } from "../auth/principal";
import { createStorageUsage } from "@webdock/module-storage/server/usage";
import {
  measureStorage,
  validateStoreToken,
} from "@webdock/adapter-storage-vercel";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { resolveSessionReference } from "../auth/principal";
import { database } from "../../db/runtime";
import { getTenant } from "../access/tenants";
import { encryptMailSecret, decryptMailSecret } from "./platform";
import { installationRegistry } from "../../installation";
export { measureStorage } from "@webdock/adapter-storage-vercel";
export {
  StorageUsageError,
  storageUsageSchemaSQL,
  normalizeStoragePrefix,
} from "@webdock/module-storage/server/usage";
export const { getTenantStorage, manageTenantStorage, refreshStorageBatch } =
  createStorageUsage({
    identity: selectedIdentityDirectory,
    database,
    withAuthenticatedDatabase,
    resolveSessionReference,
    getTenant: (...args) => getTenant(...args),
    requireStorage: (intent) =>
      installationRegistry().require("storage", intent),
    encryptSecret: (...args) => encryptMailSecret(...args),
    decryptSecret: (value, context) =>
      decryptMailSecret<string>(value, context),
    measureStorage,
    validateStoreToken,
  });
