import { readAuthProviderConfiguration } from "@webdock/auth-configuration";

import { currentMCPClaims } from "./mcp";
import {
  resolveSessionReference,
  currentConsolePrincipal,
  selectedIdentityDirectory,
} from "./principal";
export async function getBusinessIdentity(headers: Headers) {
  const principal = currentConsolePrincipal();
  if (principal)
    return {
      provider: "clerk" as const,
      sessionID: principal.sessionID,
      user: {
        id: principal.userID,
        name: principal.name,
        email: principal.email,
        role: principal.operator ? "operator" : "user",
        banned: false,
        emailVerified: true,
        twoFactorEnabled: false,
        mustChangePassword: false,
      },
    };
  const reference = await resolveSessionReference(headers);
  if (!reference) return null;
  const { auth } = await import("./auth");
  const session = await auth.api.getSession({ headers });
  if (
    session?.user.id !== reference.subject ||
    session.session.id !== reference.sessionID
  )
    return null;
  if (
    readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER)
      .provider === "clerk"
  ) {
    const { authDatabase } = await import("../../db/runtime");
    const identity = await selectedIdentityDirectory.session(
      authDatabase,
      reference,
    );
    if (!identity) return null;
    return {
      provider: "clerk" as const,
      sessionID: reference.sessionID,
      user: {
        ...session.user,
        role: identity.role,
        banned: identity.banned,
        emailVerified: identity.emailVerified,
        twoFactorEnabled: identity.twoFactorEnabled,
        mustChangePassword: identity.mustChangePassword,
      },
    };
  }
  return session
    ? {
        provider: "legacy" as const,
        user: session.user,
        sessionID: session.session.id,
      }
    : null;
}
export async function businessOperator(subject: string | undefined) {
  const principal = currentConsolePrincipal();
  if (principal)
    return { disabled: principal.userID !== subject || !principal.operator };
  return currentMCPClaims(subject);
}
