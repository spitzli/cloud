import { pluginOperationAuthority } from "./plugin-provisioning-store";
import { createHostingDatabaseAgent } from "@webdock/module-databases/server/database-agent";
import { verifyAgent } from "./clusters";
import { transaction, clusterLock } from "./db";
import { authorizeHosting } from "./authorization";
import { decryptMailSecret } from "../platform/platform";
import { revalidateStoredConsoleIdentity } from "../auth/clerk-provider";
import { withStoredConsolePrincipal } from "../auth/principal";
export const {
  claimDatabaseOperation,
  checkpointDatabaseOperation,
  completeDatabaseOperation,
  guardDatabaseOperation,
} = createHostingDatabaseAgent({
  operationAuthority: pluginOperationAuthority,
  async withStoredIdentity(reference, run) {
    const principal = await revalidateStoredConsoleIdentity(reference);
    return withStoredConsolePrincipal(principal, () => run(principal));
  },
  get verifyAgent() {
    return verifyAgent;
  },
  get transaction() {
    return transaction;
  },
  get clusterLock() {
    return clusterLock;
  },
  get authorizeHosting() {
    return authorizeHosting;
  },
  get decryptSecret() {
    return decryptMailSecret;
  },
  env: process.env,
});
