import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { currentConsolePrincipal } from "../auth/principal";
import { legacyClerkMemberships } from "../auth/clerk-provider";
import {
  storedConsoleIdentity,
  withStoredConsolePrincipal,
} from "../auth/principal";
import { revalidateStoredConsoleIdentity } from "../auth/clerk-provider";
import { createDatabaseService } from "@webdock/module-databases/server/service";
import { authorizeHosting } from "../hosting/authorization";
import { audit as hostingAudit } from "../hosting/db";
import { encryptMailSecret, decryptMailSecret } from "../platform/platform";
/** Existing API entrypoints retain their signatures and transaction composition. */
export const { executeDatabase, exchangeLaunch, inspectSession } =
  createDatabaseService({
    recipientAuthority: {
      isExternal: () => !!currentConsolePrincipal() || readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider === "clerk",
      async allowed(subject, customerID) {
        const proof = await legacyClerkMemberships(subject);
        return !!proof && (proof.operator || proof.roles.some(role => role.customerID === customerID));
      },
    },
    storedConsoleIdentity,
    async withStoredIdentity(reference, run) {
      const principal = await revalidateStoredConsoleIdentity(reference);
      return withStoredConsolePrincipal(principal, () => run(principal));
    },
    authorizeHosting,
    hostingAudit,
    encryptMailSecret,
    decryptMailSecret,
    gatewayOrigin: () => process.env.WEBDOCK_DATABASE_GATEWAY_ORIGIN,
  });
