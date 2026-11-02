import { consoleAuthConfiguration } from "./console-configuration";
import {
  normalizeOrganizationPermissions,
  isClerkOrganizationRole,
  type OrgPermission,
} from "./organization-permissions";
import { currentConsolePrincipal } from "./principal";
import { currentDatabaseScope } from "@webdock/database-runtime";
import { createClerkClient } from "@clerk/backend";
import { authDatabase } from "../../db/runtime";
import {
  type ConsoleIdentityDependencies,
  ConsoleAuthError,
  createConsoleVerifier,
  resolveConsolePrincipal,
} from "./clerk-identity";
export function clerkProvider() {
  const secretKey = process.env.CLERK_SECRET_KEY,
    publishableKey = process.env.CLERK_PUBLISHABLE_KEY;
  if (!secretKey || !publishableKey)
    throw new ConsoleAuthError(503, "Console identity is not configured.");
  return createClerkClient({
    secretKey,
    publishableKey,
    telemetry: { disabled: true },
  });
}
export async function liveMemberships(userID: string) {
  const provider = clerkProvider(),
    members = [];
  for (let offset = 0; ; offset += 100) {
    const page = await provider.users.getOrganizationMembershipList({
      userId: userID,
      limit: 100,
      offset,
    });
    members.push(...page.data);
    if (members.length >= page.totalCount || page.data.length === 0)
      return members;
    if (offset >= 10000)
      throw new ConsoleAuthError(
        503,
        "Organization access is temporarily unavailable.",
      );
  }
}
function consoleConfiguration() {
  const config = consoleAuthConfiguration();
  if (!config.issuer || !config.authorizedParties.length || !config.serviceKey)
    throw new ConsoleAuthError(503, "Console identity is not configured.");
  return { ...config, issuer: config.issuer, serviceKey: config.serviceKey };
}
function consoleDependencies() {
  const provider = clerkProvider();
  return {
    verify: async (request: Request) => {
      // Strip cookies: this boundary accepts only the explicitly delegated session token.
      return authenticateConsole(
        new Request(request.url, {
          headers: {
            authorization: request.headers.get("authorization") || "",
          },
        }),
      );
    },
    session: (id: string) => provider.sessions.getSession(id),
    user: (id: string) => provider.users.getUser(id),
    memberships: liveMemberships,
    projectIdentity: consoleAuthConfiguration().requireExistingMapping
      ? undefined
      : async (input: {
          issuer: string;
          userID: string;
          organizationID: string;
          name: string;
          email: string;
        }) => {
          await provider.users.updateUser(input.userID, {
            deleteSelfEnabled: false,
            createOrganizationEnabled: false,
          });
          try {
            await authDatabase.query(
              "SELECT webdock_rls.project_clerk_user($1,$2,$3,$4,$5)",
              [
                input.issuer,
                input.userID,
                input.organizationID,
                input.name,
                input.email,
              ],
            );
          } catch (error) {
            if ((error as { code?: string }).code === "23505")
              throw new ConsoleAuthError(
                403,
                "This email requires an explicit account mapping.",
              );
            throw error;
          }
        },
    identity: async (issuer: string, userID: string) =>
      (
        await authDatabase.query(
          `SELECT u.id,u.name,u.email,u.role,u.banned,u."emailVerified",i.enabled FROM webdock_auth.clerk_identity i JOIN webdock_auth."user" u ON u.id=i.user_id WHERE i.issuer=$1 AND i.clerk_user_id=$2`,
          [issuer, userID],
        )
      ).rows[0] || null,
    organization: async (issuer: string, organizationID: string) =>
      (
        await authDatabase.query(
          `SELECT o.organization_id AS id,t.customer_id AS "customerID",c.status='active' AS active,o.enabled FROM webdock_auth.clerk_organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.organization_id JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE o.issuer=$1 AND o.clerk_organization_id=$2`,
          [issuer, organizationID],
        )
      ).rows[0] || null,
  };
}
export async function requireConsolePrincipal(request: Request) {
  return createConsoleVerifier(
    consoleConfiguration(),
    consoleDependencies(),
  )(request);
}
export type StoredConsoleIdentity = {
  issuer: string;
  userID: string;
  sessionID: string;
  organizationID: string | null;
};
/** Only call with a reference loaded from a private viewer/session/job record. */
export async function revalidateStoredConsoleIdentity(
  reference: StoredConsoleIdentity,
  mappings?: Pick<ConsoleIdentityDependencies, "identity" | "organization">,
) {
  const config = consoleConfiguration();
  if (reference.issuer !== config.issuer)
    throw new ConsoleAuthError(401, "Identity issuer mismatch.");
  const dependencies = consoleDependencies();
  return resolveConsolePrincipal(
    config,
    mappings
      ? { ...dependencies, ...mappings, projectIdentity: undefined }
      : dependencies,
    {
      iss: reference.issuer,
      sub: reference.userID,
      sid: reference.sessionID,
      orgId: reference.organizationID,
      azp: config.authorizedParties[0],
      exp: Math.floor(Date.now() / 1000) + 60,
      aud: config.audience,
    },
  );
}

async function authenticateConsole(request: Request) {
  const config = consoleConfiguration();
  const state = await clerkProvider().authenticateRequest(request, {
    acceptsToken: "session_token",
    authorizedParties: config.authorizedParties,
    ...(config.audience ? { audience: config.audience } : {}),
  });
  const identity = state.toAuth();
  if (
    !identity ||
    identity.tokenType !== "session_token" ||
    !identity.userId ||
    !identity.sessionId ||
    !identity.sessionClaims
  )
    throw new ConsoleAuthError(401, "Sign in again.");
  return {
    sub: identity.userId,
    sid: identity.sessionId,
    iss: identity.sessionClaims.iss,
    azp: identity.sessionClaims.azp || "",
    exp: identity.sessionClaims.exp,
    orgId: identity.orgId,
    aud: identity.sessionClaims.aud,
  };
}
/** Fixed browser callback/form routes only; generic business calls still require the service key. */
export async function requireConsoleBrowserPrincipal(request: Request) {
  if (
    !consoleAuthConfiguration().enabled ||
    request.headers.has("authorization")
  )
    throw new ConsoleAuthError(401, "Sign in again.");
  const headers = new Headers({ cookie: request.headers.get("cookie") || "" });
  const claims = await authenticateConsole(
    new Request(request.url, { headers }),
  );
  return resolveConsolePrincipal(
    consoleConfiguration(),
    consoleDependencies(),
    claims,
  );
}

/** Live membership authority for linked legacy clients; their own session/MFA checks still run. */
export async function legacyClerkMemberships(subject: string, sessionID?: string) {
  const scope = currentDatabaseScope();
  if (scope?.kind === "system" && scope.purpose === "hosting-setup") {
    // The claimed-job verifier supplies this proof; SQL independently rejects linked jobs without it.
    const proof = scope.memberships;
    if (!proof) return undefined;
    const now = Date.now();
    if (
      proof.subject !== subject ||
      proof.issuer !== consoleAuthConfiguration().issuer ||
      !Number.isFinite(proof.verifiedAt) ||
      !Number.isFinite(proof.expiresAt) ||
      proof.verifiedAt > now ||
      proof.verifiedAt < now - 60000 ||
      proof.expiresAt <= now ||
      proof.expiresAt > proof.verifiedAt + 60000
    )
      throw new ConsoleAuthError(403, "Setup membership proof is unavailable.");
    return proof;
  }
  if (!consoleAuthConfiguration().requireExistingMapping && !currentConsolePrincipal()) return undefined;
  if (
    scope?.kind === "session" &&
    scope.subject === subject &&
    (sessionID === undefined || scope.sessionID === sessionID) &&
    scope.memberships
  ) {
    const proof = scope.memberships, now = Date.now();
    if (scope.authProvider !== "clerk" || proof.subject !== subject ||
        proof.sessionID !== scope.sessionID || !scope.sessionID ||
        proof.issuer !== consoleAuthConfiguration().issuer ||
        !Number.isFinite(proof.verifiedAt) || !Number.isFinite(proof.expiresAt) ||
        proof.verifiedAt > now || proof.verifiedAt < now - 60000 ||
        proof.expiresAt <= now || proof.expiresAt > proof.verifiedAt + 60000)
      throw new ConsoleAuthError(403, "Session membership proof is unavailable.");
    return scope.memberships;
  }
  const issuer = consoleAuthConfiguration().issuer;
  if (!issuer) {
    if (
      (
        await authDatabase.query(
          "SELECT to_regclass('webdock_auth.clerk_identity') IS NOT NULL AS present",
        )
      ).rows[0]?.present &&
      (
        await authDatabase.query(
          "SELECT 1 FROM webdock_auth.clerk_identity WHERE user_id=$1",
          [subject],
        )
      ).rowCount
    )
      throw new ConsoleAuthError(
        503,
        "Linked identity verification is not configured.",
      );
    return undefined;
  }
  const link = (
    await authDatabase.query(
      "SELECT clerk_user_id,enabled FROM webdock_auth.clerk_identity WHERE issuer=$1 AND user_id=$2",
      [issuer, subject],
    )
  ).rows[0];
  if (!link) {
    if (
      (
        await authDatabase.query(
          "SELECT 1 FROM webdock_auth.clerk_identity WHERE user_id=$1",
          [subject],
        )
      ).rowCount
    )
      throw new ConsoleAuthError(
        503,
        "Linked identity verification is not configured for this issuer.",
      );
    if (consoleAuthConfiguration().requireExistingMapping)
      throw new ConsoleAuthError(403, "Clerk identity mapping is unavailable.");
    return undefined;
  }
  if (!link.enabled)
    throw new ConsoleAuthError(403, "Account access is disabled.");
  const provider = clerkProvider(),
    user = await provider.users.getUser(link.clerk_user_id);
  if (
    user.id !== link.clerk_user_id ||
    user.banned ||
    user.locked ||
    !user.emailAddresses.some(
      (e) =>
        e.id === user.primaryEmailAddressId &&
        e.verification?.status === "verified",
    )
  )
    throw new ConsoleAuthError(403, "Account access is disabled.");
  const live = await liveMemberships(link.clerk_user_id);
  const links = (
    await authDatabase.query(
      `SELECT o.clerk_organization_id,o.organization_id,t.customer_id FROM webdock_auth.clerk_organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.organization_id JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE o.issuer=$1 AND o.enabled AND c.status='active'`,
      [issuer],
    )
  ).rows;
  const roles = links.flatMap((row) => {
    const member = live.find(
      (m) => m.organization.id === row.clerk_organization_id,
    );
    return member && isClerkOrganizationRole(member.role)
      ? [
          {
            organizationID: String(row.organization_id),
            customerID: String(row.customer_id),
            role: member.role === "org:admin" ? "admin" : "member",
            permissions: normalizeOrganizationPermissions(member.permissions),
          },
        ]
      : [];
  });
  const now = Date.now();
  return {
    subject,
    issuer,
    userID: link.clerk_user_id as string,
    operator: user.privateMetadata?.webdockRole === "operator",
    twoFactorEnabled: user.twoFactorEnabled === true,
    verifiedAt: now,
    expiresAt: now + 60000,
    roles,
  };
}
export async function linkedMembershipRole(
  subject: string,
  organizationID: string,
): Promise<string | null | undefined> {
  const scope = currentDatabaseScope();
  if (scope?.kind === "system" && scope.purpose === "hosting-setup") {
    const proof = await legacyClerkMemberships(subject);
    return proof
      ? (proof.roles.find((row) => row.organizationID === organizationID)
          ?.role ?? null)
      : undefined;
  }
  if (!consoleAuthConfiguration().requireExistingMapping && !currentConsolePrincipal()) return undefined;
  const issuer = consoleAuthConfiguration().issuer;
  if (!issuer) {
    if (
      (
        await authDatabase.query(
          "SELECT to_regclass('webdock_auth.clerk_organization') IS NOT NULL AS present",
        )
      ).rows[0]?.present &&
      (
        await authDatabase.query(
          "SELECT 1 FROM webdock_auth.clerk_organization WHERE organization_id=$1",
          [organizationID],
        )
      ).rowCount
    )
      throw new ConsoleAuthError(
        503,
        "Linked team verification is not configured.",
      );
    return undefined;
  }
  const link = (
    await authDatabase.query(
      "SELECT enabled FROM webdock_auth.clerk_organization WHERE issuer=$1 AND organization_id=$2",
      [issuer, organizationID],
    )
  ).rows[0];
  if (!link) {
    if (
      (
        await authDatabase.query(
          "SELECT 1 FROM webdock_auth.clerk_organization WHERE organization_id=$1",
          [organizationID],
        )
      ).rowCount
    )
      throw new ConsoleAuthError(
        503,
        "Linked team verification is not configured for this issuer.",
      );
    return consoleAuthConfiguration().requireExistingMapping ? null : undefined;
  }
  if (!link.enabled) return null;
  const proof = await legacyClerkMemberships(subject);
  return (
    proof?.roles.find((r) => r.organizationID === organizationID)?.role ?? null
  );
}

/** undefined means an unmigrated legacy organization; callers retain that legacy policy only. */
export async function organizationPermission(
  subject: string,
  organizationID: string,
  permission: OrgPermission,
): Promise<boolean | undefined> {
  const principal = currentConsolePrincipal();
  if (principal) {
    if (principal.userID !== subject)
      throw new ConsoleAuthError(403, "Console identity mismatch.");
    return (
      principal.operator ||
      (principal.organizationID === organizationID &&
        normalizeOrganizationPermissions(principal.permissions).includes(
          permission,
        ))
    );
  }
  const role = await linkedMembershipRole(subject, organizationID);
  if (role === undefined) return undefined;
  if (role === null) return false;
  const proof = await legacyClerkMemberships(subject);
  const member = proof?.roles.find(
    (row) => row.organizationID === organizationID,
  );
  return normalizeOrganizationPermissions(member?.permissions).includes(
    permission,
  );
}
