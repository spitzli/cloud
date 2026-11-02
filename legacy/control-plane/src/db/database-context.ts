import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { consoleAuthConfiguration } from "../modules/auth/console-configuration";
import {
  currentDatabaseScope,
  withDatabaseScope,
} from "@webdock/database-runtime";
import type { HostingActor } from "@webdock/hosting-contracts";
import type { MailActor } from "@webdock/mail-contracts";

/** Authentication stays on its bootstrap pool; business queries receive only verified identity. */
export async function withAuthenticatedDatabase<T>(
  headers: Headers,
  run: () => Promise<T>,
): Promise<T> {
  if (!process.env.DATABASE_RLS_SECRET && !consoleAuthConfiguration().issuer)
    return run();
  const { getBusinessIdentity } =
    await import("../modules/auth/business-identity");
  const session = await getBusinessIdentity(headers);
  if (!session) {
    const { AccessError } = await import("../modules/access/access-management");
    throw new AccessError("Sign in and complete account setup first.");
  }
  const inherited = currentDatabaseScope();
  const nativePrincipal = (await import("../modules/auth/principal")).currentConsolePrincipal();
  const memberships =
    !nativePrincipal &&
    !(
      inherited?.kind === "session" &&
      inherited.subject === session.user.id &&
      inherited.sessionID === session.sessionID &&
      inherited.memberships
    )
      ? await (
          await import("../modules/auth/clerk-provider")
        ).legacyClerkMemberships(session.user.id, session.sessionID)
      : undefined;
  return withDatabaseScope(
    {
      kind: "session",
      authProvider: readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider,
      subject: session.user.id,
      sessionID: session.sessionID,
      ...(memberships ? { memberships: { ...memberships, subject: session.user.id, sessionID: session.sessionID } } : {}),
    },
    run,
  );
}

export async function withHostingDatabase<T>(
  actor: HostingActor,
  run: () => Promise<T>,
): Promise<T> {
  // Git publication already runs inside an authenticated worker scope. It must not
  // manufacture a session or elevate an unscoped caller to worker authority.
  if (actor.source === "git-policy") return run();
  const scope = currentDatabaseScope();
  const principal = (
    await import("../modules/auth/principal")
  ).currentConsolePrincipal();
  if (principal && scope?.kind === "system") return run();
  const memberships = principal
    ? undefined
    : scope?.kind === "session" &&
        scope.subject === actor.subject &&
        scope.sessionID === actor.sessionID &&
        scope.memberships
      ? scope.memberships
      : await (
          await import("../modules/auth/clerk-provider")
        ).legacyClerkMemberships(actor.subject, actor.sessionID);
  return withDatabaseScope(
    {
      kind: "session",
      authProvider: readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider,
      subject: actor.subject,
      sessionID: actor.sessionID,
      ...(scope?.kind === "session" &&
      scope.subject === actor.subject &&
      scope.sessionID === actor.sessionID &&
      scope.pluginInvocationID
        ? { pluginInvocationID: scope.pluginInvocationID }
        : {}),
      ...(memberships ? { memberships: { ...memberships, subject: actor.subject, sessionID: actor.sessionID } } : {}),
    },
    run,
  );
}

export async function withMailDatabase<T>(
  actor: MailActor,
  run: () => Promise<T>,
): Promise<T> {
  const principal = actor.principal;
  if (principal.kind === "session") {
    const consolePrincipal = (
      await import("../modules/auth/principal")
    ).currentConsolePrincipal();
    const scope = currentDatabaseScope();
    const memberships = consolePrincipal
      ? undefined
      : scope?.kind === "session" &&
          scope.subject === principal.userID &&
          scope.sessionID === principal.sessionID &&
          scope.memberships
        ? scope.memberships
        : await (
            await import("../modules/auth/clerk-provider")
          ).legacyClerkMemberships(principal.userID, principal.sessionID);
    return withDatabaseScope(
      {
        kind: "session",
        authProvider: readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider,
        subject: principal.userID,
        sessionID: principal.sessionID,
        ...(memberships ? { memberships: { ...memberships, subject: principal.userID, sessionID: principal.sessionID } } : {}),
      },
      run,
    );
  }
  if (principal.kind === "api_key")
    return withDatabaseScope(
      {
        kind: "mail-key",
        customerID: actor.customerID,
        connectionID: actor.connectionID,
        keyID: principal.keyID,
      },
      run,
    );
  // Job actors carry no credential. The authenticated dispatcher supplies scope.
  return run();
}
