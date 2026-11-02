import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { currentDatabaseScope, type DatabaseScope } from "@webdock/database-runtime";

/** The trusted signer stamps provider authority even for older/manual session wrappers. */
export function signedDatabaseScope(): DatabaseScope | undefined {
  const scope = currentDatabaseScope();
  if (!scope || scope.kind === "mail-key") return scope;
  const authProvider = scope.clerk ? "clerk" : readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider;
  if (scope.kind === "session" && scope.memberships) {
    return { ...scope, authProvider, memberships: { ...scope.memberships, subject: scope.memberships.subject ?? scope.subject, sessionID: scope.memberships.sessionID ?? scope.sessionID } };
  }
  return { ...scope, authProvider };
}
