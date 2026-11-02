import {
  normalizeOrganizationPermissions,
  isClerkOrganizationRole,
  type OrgPermission,
} from "./organization-permissions";
import { timingSafeEqual } from "node:crypto";
export class ConsoleAuthError extends Error {
  readonly isPublic = true;
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
export type ConsolePrincipal = Readonly<{
  provider: "clerk";
  issuer: string;
  userID: string;
  clerkUserID: string;
  sessionID: string;
  operator: boolean;
  organizationID: string | null;
  clerkOrganizationID: string | null;
  customerID: string | null;
  role: "admin" | "member" | null;
  permissions?: readonly OrgPermission[];
  verifiedAt: number;
  expiresAt: number;
  name: string;
  email: string;
}>;
type Claims = {
  sub: string;
  sid: string;
  iss: string;
  azp: string;
  exp: number;
  orgId?: string | null;
  aud?: unknown;
};
export type ConsoleIdentityDependencies = {
  verify: (request: Request) => Promise<Claims>;
  session: (id: string) => Promise<{
    id: string;
    userId: string;
    status: string;
    expireAt: number;
  }>;
  user: (id: string) => Promise<{
    id: string;
    banned: boolean;
    locked: boolean;
    privateMetadata?: { webdockRole?: unknown } | null;
    primaryEmailAddressId: string | null;
    firstName?: string | null;
    lastName?: string | null;
    emailAddresses: Array<{
      id: string;
      emailAddress?: string;
      verification: { status: string } | null;
    }>;
  }>;
  identity: (
    issuer: string,
    userID: string,
  ) => Promise<{
    id: string;
    name: string;
    email: string;
    role: string;
    banned: boolean;
    enabled: boolean;
    emailVerified: boolean;
  } | null>;
  projectIdentity?: (input: {
    issuer: string;
    userID: string;
    organizationID: string;
    name: string;
    email: string;
  }) => Promise<void>;
  organization: (
    issuer: string,
    organizationID: string,
  ) => Promise<{
    id: string;
    customerID: string;
    active: boolean;
    enabled: boolean;
  } | null>;
  memberships: (userID: string) => Promise<
    Array<{
      organization: { id: string };
      role: string;
      permissions?: string[];
    }>
  >;
};
export type ConsoleVerificationConfig = {
  issuer: string;
  authorizedParties: string[];
  serviceKey: string;
  audience?: string;
  enabled?: boolean;
};
function denied(status = 401): never {
  throw new ConsoleAuthError(
    status,
    status === 401 ? "Sign in again." : "Console access is unavailable.",
  );
}
export function createConsoleVerifier(
  config: ConsoleVerificationConfig,
  deps: ConsoleIdentityDependencies,
) {
  if (
    new URL(config.issuer).origin !== config.issuer ||
    !config.issuer.startsWith("https://") ||
    !config.authorizedParties.length ||
    Buffer.byteLength(config.serviceKey) < 32
  )
    throw Error("Invalid Console identity configuration");
  for (const origin of config.authorizedParties)
    if (new URL(origin).origin !== origin)
      throw Error("Invalid Console authorized party");
  return async (request: Request): Promise<ConsolePrincipal> => {
    if (config.enabled === false) denied();
    const supplied = Buffer.from(
        request.headers.get("x-webdock-service-key") || "",
      ),
      expected = Buffer.from(config.serviceKey);
    if (
      supplied.length !== expected.length ||
      !timingSafeEqual(supplied, expected)
    )
      denied();
    if (
      !/^Bearer [^\s]{1,16384}$/.test(
        request.headers.get("authorization") || "",
      )
    )
      denied();
    return resolveConsolePrincipal(config, deps, await deps.verify(request));
  };
}
/** Resolve only SDK-verified claims, or a server-owned stored provider reference. */
export async function resolveConsolePrincipal(
  config: ConsoleVerificationConfig,
  deps: ConsoleIdentityDependencies,
  claims: Claims,
): Promise<ConsolePrincipal> {
  try {
    const now = Date.now();
    if (
      claims.iss !== config.issuer ||
      (config.audience !== undefined &&
        !(Array.isArray(claims.aud) ? claims.aud : [claims.aud]).includes(
          config.audience,
        )) ||
      !config.authorizedParties.includes(claims.azp) ||
      !claims.sub ||
      !claims.sid ||
      !Number.isFinite(claims.exp) ||
      claims.exp * 1000 <= now
    )
      denied();
    const [session, user, existing] = await Promise.all([
      deps.session(claims.sid),
      deps.user(claims.sub),
      deps.identity(config.issuer, claims.sub),
    ]);
    if (
      session.id !== claims.sid ||
      session.userId !== claims.sub ||
      session.status !== "active" ||
      !Number.isFinite(session.expireAt) ||
      session.expireAt <= now
    )
      denied();
    let local = existing;
    if (user.id !== claims.sub) denied();
    if (
      (local &&
        (!local.enabled || local.banned)) ||
      user.banned ||
      user.locked ||
      !user.emailAddresses.some(
        (e) =>
          e.id === user.primaryEmailAddressId &&
          e.verification?.status === "verified",
      )
    )
      denied(403);
    let organizationID: string | null = null,
      customerID: string | null = null,
      role: "admin" | "member" | null = null;
    let permissions: OrgPermission[] = [];
    if (claims.orgId) {
      const [organization, memberships] = await Promise.all([
        deps.organization(config.issuer, claims.orgId),
        deps.memberships(claims.sub),
      ]);
      const member = memberships.find(
        (m) => m.organization.id === claims.orgId,
      );
      if (
        !organization?.enabled ||
        !organization.active ||
        !member ||
        !isClerkOrganizationRole(member.role)
      )
        denied(403);
      organizationID = organization.id;
      customerID = organization.customerID;
      role = member.role === "org:admin" ? "admin" : "member";
      permissions = normalizeOrganizationPermissions(member.permissions);
    }
    if (!local && claims.orgId && organizationID && deps.projectIdentity) {
      const email = user.emailAddresses.find(
        (e) => e.id === user.primaryEmailAddressId,
      )?.emailAddress;
      if (!email) denied(403);
      await deps.projectIdentity({
        issuer: config.issuer,
        userID: claims.sub,
        organizationID: claims.orgId,
        email,
        name:
          [user.firstName, user.lastName].filter(Boolean).join(" ") ||
          "Team member",
      });
      local = await deps.identity(config.issuer, claims.sub);
    }
    if (!local)
      throw new ConsoleAuthError(403, "Ask an administrator to provision your Clerk identity mapping for this installation.");
    if (
      !local.enabled || local.banned
    )
      denied(403);
    return Object.freeze({
      provider: "clerk",
      issuer: config.issuer,
      userID: local.id,
      clerkUserID: claims.sub,
      sessionID: claims.sid,
      operator: user.privateMetadata?.webdockRole === "operator",
      organizationID,
      clerkOrganizationID: claims.orgId || null,
      customerID,
      role,
      permissions: Object.freeze(permissions),
      verifiedAt: now,
      expiresAt: Math.min(claims.exp * 1000, session.expireAt, now + 60000),
      name: local.name,
      email: local.email,
    });
  } catch (error) {
    if (error instanceof ConsoleAuthError) throw error;
    const status = (error as { status?: number })?.status;
    if (status === 404 || status === 401) denied();
    throw new ConsoleAuthError(
      status === 429 ? 429 : 503,
      "Identity service is temporarily unavailable.",
    );
  }
}
