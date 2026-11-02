import {
  betterAuthIdentity,
} from "@webdock/auth-better-auth";
import { createPlugin as createBetterAuthPlugin } from "@webdock/auth-better-auth/plugin";
import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
const browserAuthProvider = readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider;
import { createBrowserPrincipal } from "./browser-principal";
import { oauthCompatibilityIdentity } from "./oauth-compatibility";
const local = createBetterAuthPlugin(async (headers) => {
  const { auth } = await import("./auth");
  return auth.api.getSession({ headers });
}).authenticator;
export const {
  withTrustedLocalSession,
  currentTrustedBrowserReference,
  trustedLocalSessionKind,
  resolveSessionReference: resolveBrowserSessionReference,
  selectedIdentityDirectory: browserIdentityDirectory,
} = createBrowserPrincipal({
  provider: browserAuthProvider,
  local,
  localDirectory: betterAuthIdentity,
  clerkDirectory: () => ({
    session: async (...args) =>
      (await (await import("./clerk-runtime")).getClerkBrowserAdapter()).identity.session(...args),
    subject: async (...args) =>
      (await (await import("./clerk-runtime")).getClerkBrowserAdapter()).identity.subject(...args),
    membership: async (...args) =>
      (await (await import("./clerk-runtime")).getClerkBrowserAdapter()).identity.membership(...args),
  }),
});

import { AsyncLocalStorage } from "node:async_hooks";
import { currentDatabaseScope, withDatabaseScope } from "@webdock/database-runtime";
import { consoleAuthConfiguration } from "./console-configuration";
import { ConsoleAuthError, type ConsolePrincipal } from "./clerk-identity";
const principals = new AsyncLocalStorage<ConsolePrincipal>();
export function currentConsolePrincipal() {
  const principal = principals.getStore();
  if (principal && principal.expiresAt <= Date.now())
    throw new ConsoleAuthError(401, "Sign in again.");
  return principal;
}
export function withConsolePrincipal<T>(
  principal: ConsolePrincipal,
  run: () => T,
): T {
  return principals.run(principal, () =>
    withDatabaseScope(
      {
        kind: "session",
        authProvider: "clerk",
        subject: principal.userID,
        sessionID: principal.sessionID,
        clerk: {
          issuer: principal.issuer,
          operator: principal.operator,
          userID: principal.clerkUserID,
          organizationID: principal.organizationID,
          clerkOrganizationID: principal.clerkOrganizationID,
          customerID: principal.customerID,
          role: principal.role,
          permissions: principal.permissions ?? [],
          verifiedAt: principal.verifiedAt,
          expiresAt: principal.expiresAt,
        },
      },
      run,
    ),
  );
}
/** Callers must still validate the customer's active state and resource relationship. */
export function consoleMembership(userID: string, organizationID: string) {
  const principal = currentConsolePrincipal();
  if (!principal) return undefined;
  if (principal.userID !== userID)
    throw new ConsoleAuthError(403, "Console identity mismatch.");
  return principal.organizationID === organizationID && principal.role
    ? { id: null, role: principal.role, permissions: [...(principal.permissions ?? [])] }
    : null;
}
export function withStoredConsolePrincipal<T>(
  principal: ConsolePrincipal,
  run: () => T,
): T {
  // Gateway/worker DB capabilities remain independently scoped by their caller.
  return principals.run(principal, run);
}
export function storedConsoleIdentity() {
  const p = currentConsolePrincipal();
  return p
    ? {
        issuer: p.issuer,
        userID: p.clerkUserID,
        sessionID: p.sessionID,
        organizationID: p.clerkOrganizationID,
      }
    : null;
}

/** Console principals have already passed live provider verification at the transport boundary. */
export async function resolveSessionReference(headers: Headers) {
  const principal = currentConsolePrincipal();
  return principal
    ? { subject: principal.userID, sessionID: principal.sessionID }
    : resolveBrowserSessionReference(headers);
}
/** The worker revalidates this proof for its claimed job; SQL also checks mapping and lease. */
function setupMemberships(subject: string, sessionID?: string) {
  const scope = currentDatabaseScope();
  if (browserAuthProvider !== "clerk" || scope?.kind !== "system" || scope.purpose !== "hosting-setup")
    return undefined;
  const proof = scope.memberships;
  const now = Date.now();
  if (!proof || scope.authProvider !== "clerk" || proof.subject !== subject ||
      !proof.sessionID || (sessionID !== undefined && proof.sessionID !== sessionID) ||
      proof.issuer !== consoleAuthConfiguration().issuer ||
      typeof proof.operator !== "boolean" || typeof proof.twoFactorEnabled !== "boolean" ||
      !Number.isFinite(proof.verifiedAt) || !Number.isFinite(proof.expiresAt) ||
      proof.verifiedAt > now || proof.verifiedAt < now - 60000 ||
      proof.expiresAt <= now || proof.expiresAt > proof.verifiedAt + 60000)
    return null;
  return proof;
}
function setupIdentity<T extends import("@webdock/core/identity").LocalIdentity>(
  local: T | null, proof: NonNullable<ReturnType<typeof setupMemberships>>,
) {
  return local && !local.banned ? {
    ...local, role: proof.operator ? "operator" : "user", emailVerified: true,
    mustChangePassword: false, twoFactorEnabled: proof.twoFactorEnabled,
    operatorAuthorized: proof.operator && proof.twoFactorEnabled,
  } : null;
}
export const selectedIdentityDirectory: import("@webdock/core/identity").IdentityDirectory =
  {
    async subject(db, subject, options) {
      const principal = currentConsolePrincipal();
      if (!principal) {
        const proof = setupMemberships(subject);
        if (proof !== undefined)
          return proof ? setupIdentity(await betterAuthIdentity.subject(db, subject, options), proof) : null;
        if (!currentTrustedBrowserReference()) {
          const compatibility = await oauthCompatibilityIdentity(db, subject, undefined, options);
          if (compatibility !== undefined) return compatibility?.identity ?? null;
        }
        return browserIdentityDirectory.subject(db, subject, options);
      }
      if (principal.userID !== subject) return null;
      const local = await betterAuthIdentity.subject(db, subject, options);
      if (
        !local ||
        local.banned
      )
        return null;
      // Console account setup and email verification are owned by the verified provider.
      return {
        ...local,
        role: principal.operator ? "operator" : "user",
        emailVerified: true,
        mustChangePassword: false,
        twoFactorEnabled: false,
        operatorAuthorized: principal.operator,
      };
    },
    async session(db, reference, options) {
      const principal = currentConsolePrincipal();
      if (!principal) {
        const proof = setupMemberships(reference.subject, reference.sessionID);
        if (proof !== undefined)
          return proof ? setupIdentity(await betterAuthIdentity.session(db, reference, options), proof) : null;
        if (!currentTrustedBrowserReference()) {
          const compatibility = await oauthCompatibilityIdentity(db, reference.subject, reference.sessionID, options);
          if (compatibility !== undefined) return compatibility?.identity ?? null;
        }
        return browserIdentityDirectory.session(db, reference, options);
      }
      if (
        principal.userID !== reference.subject ||
        principal.sessionID !== reference.sessionID
      )
        return null;
      const local = await selectedIdentityDirectory.subject(
        db,
        reference.subject,
        options,
      );
      if (!local) return null;
      const preview =
        (
          await db.query(
            "SELECT session_id FROM webdock_auth.console_tenant_preview WHERE session_id=$1 AND issuer=$2",
            [reference.sessionID, principal.issuer],
          )
        ).rows.length > 0;
      return { ...local, sessionID: reference.sessionID, preview };
    },
    async membership(db, subject, customerID, options) {
      const principal = currentConsolePrincipal();
      if (!principal) {
        const proof = setupMemberships(subject);
        if (proof !== undefined) {
          const member = proof?.roles.find(role => role.customerID === customerID);
          return proof ? { memberID: null, role: member?.role ?? null, permissions: [...(member?.permissions ?? [])] } : null;
        }
        if (!currentTrustedBrowserReference()) {
          const compatibility = await oauthCompatibilityIdentity(db, subject, undefined, options);
          if (compatibility !== undefined) {
            const member = compatibility?.memberships.find(role => role.customerID === customerID);
            return compatibility ? { memberID: null, role: member?.role ?? null, permissions: [...(member?.permissions ?? [])] } : null;
          }
        }
        if (browserAuthProvider === "clerk")
          return browserIdentityDirectory.membership(db, subject, customerID, options);
        const tenant = (
          await db.query(
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [customerID],
          )
        ).rows[0];
        if (!tenant) return null;
        const { linkedMembershipRole } = await import("./clerk-provider");
        const role = await linkedMembershipRole(
          subject,
          tenant.organization_id,
        );
        return role === undefined
          ? browserIdentityDirectory.membership(
              db,
              subject,
              customerID,
              options,
            )
          : { memberID: null, role };
      }
      if (principal.userID !== subject)
        throw new ConsoleAuthError(403, "Console identity mismatch.");
      const tenant = (
        await db.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [customerID],
        )
      ).rows[0];
      if (!tenant) return null;
      const matches = principal.customerID === customerID &&
        principal.organizationID === tenant.organization_id;
      return {
        memberID: null,
        role: matches ? principal.role : null,
        permissions: matches ? [...(principal.permissions ?? [])] : [],
      };
    },
  };
