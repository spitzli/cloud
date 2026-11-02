import {
  currentDatabaseScope,
  withDatabaseScope,
  type DatabaseScope,
} from "@webdock/database-runtime";
import { PluginError } from "@webdock/plugin-sdk/contracts";
import type { ConsolePrincipal } from "../auth/clerk-identity";
import type { StoredConsoleIdentity } from "../auth/clerk-provider";
export function createPluginSessionRunner(dependencies: {
  revalidate(reference: StoredConsoleIdentity): Promise<ConsolePrincipal>;
  withPrincipal<T>(principal: ConsolePrincipal, work: () => T): T;
}) {
  return async <T>(
    session: Extract<DatabaseScope, { kind: "session" }>,
    invocationID: string,
    work: () => Promise<T>,
  ): Promise<T> => {
    if (!session.clerk)
      return withDatabaseScope(
        { ...session, pluginInvocationID: invocationID },
        work,
      );
    const principal = await dependencies.revalidate({
      issuer: session.clerk.issuer,
      userID: session.clerk.userID,
      sessionID: session.sessionID,
      organizationID: session.clerk.clerkOrganizationID,
    });
    if (
      principal.userID !== session.subject ||
      principal.sessionID !== session.sessionID
    )
      throw new PluginError("permission_denied", "Invocation identity changed");
    return dependencies.withPrincipal(principal, () => {
      const fresh = currentDatabaseScope();
      if (
        fresh?.kind !== "session" ||
        fresh.subject !== session.subject ||
        fresh.sessionID !== session.sessionID
      )
        throw new PluginError(
          "permission_denied",
          "Invocation identity unavailable",
        );
      return withDatabaseScope(
        { ...fresh, pluginInvocationID: invocationID },
        work,
      );
    });
  };
}
