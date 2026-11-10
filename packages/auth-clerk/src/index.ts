import type { AuthProviderConfiguration } from "@spitzli/auth-configuration";
import type {
  DatabaseReader,
  IdentityDirectory,
  IdentityReadOptions,
  LocalIdentity,
  SessionAuthenticator,
  SessionReference,
} from "@spitzli/core/identity";
export type ClerkConfiguration = Extract<
  AuthProviderConfiguration,
  { provider: "clerk" }
>;
export type ExternalIdentity = { issuer: string; subject: string };
export type ExternalSession = ExternalIdentity & { sessionID: string };
export type ClerkClaims = {
  iss?: unknown;
  sub?: unknown;
  sid?: unknown;
  azp?: unknown;
  aud?: unknown;
  exp?: unknown;
  nbf?: unknown;
  iat?: unknown;
  fva?: unknown;
  act?: unknown;
};
export type ClerkSession = {
  id: string;
  userId: string;
  status: string;
  expireAt: number;
  actor: unknown;
};
export type ClerkUser = {
  id: string;
  /** Sanitized exclusively from the live Backend User.privateMetadata.webdockRole. */
  role: "operator" | "user";
  banned: boolean;
  locked: boolean;
  twoFactorEnabled: boolean;
  emailVerified: boolean;
};
export type ClerkMembership = {
  id: string;
  organizationID: string;
  subject: string;
  role: string;
  permissions: string[];
};
/** No provider SDK objects or tokens cross this port. Each call must read current provider state. */
export interface ClerkBackend {
  verifyToken(token: string, config: ClerkConfiguration): Promise<ClerkClaims>;
  session(sessionID: string): Promise<ClerkSession>;
  user(subject: string): Promise<ClerkUser>;
  membership(
    organizationID: string,
    subject: string,
  ): Promise<ClerkMembership | null>;
}
/** Durable, issuer-scoped mappings created by an explicit enrollment workflow; never email auto-linking. */
export interface ClerkMappings {
  externalOrganization(
    db: DatabaseReader,
    customerID: string,
    options?: IdentityReadOptions,
  ): Promise<{ issuer: string; organizationID: string } | null>;
  localSubject(external: ExternalIdentity): Promise<string | null>;
  localSession(external: ExternalSession): Promise<SessionReference | null>;
  externalSubject(
    db: DatabaseReader,
    subject: string,
    options?: IdentityReadOptions,
  ): Promise<ExternalIdentity | null>;
  externalSession(
    db: DatabaseReader,
    reference: SessionReference,
    options?: IdentityReadOptions,
  ): Promise<ExternalSession | null>;
}
export type ClerkExternalEvidence = {
  user: ClerkUser;
  external: ExternalSession;
  expiresAt: number;
  /** Signed token facts in minutes; null means absent. -1 means no verified factor. */
  factorVerificationAge: [number, number] | null;
};
export type ClerkEvidence = Omit<ClerkExternalEvidence, "user"> & {
  reference: SessionReference;
};
function liveSession(
  value: ClerkSession,
  external: ExternalSession,
  now: number,
) {
  return (
    value.id === external.sessionID &&
    value.userId === external.subject &&
    value.status === "active" &&
    Number.isFinite(value.expireAt) &&
    value.expireAt > now &&
    value.actor == null
  );
}
function providerIdentity(
  local: LocalIdentity | null,
  user: ClerkUser,
  external: ExternalIdentity,
): LocalIdentity | null {
  if (!local || user.id !== external.subject || user.banned || user.locked)
    return null;
  return {
    ...local,
    role: user.role === "operator" ? "operator" : "user",
    operatorAuthorized: false,
    emailVerified: user.emailVerified,
    twoFactorEnabled: user.twoFactorEnabled,
  };
}
export function createClerkAdapter(dependencies: {
  config: ClerkConfiguration;
  backend: ClerkBackend;
  mappings: ClerkMappings;
  local: IdentityDirectory;
  now?: () => number;
}) {
  const { config, backend, mappings, local } = dependencies;
  const now = dependencies.now ?? Date.now;
  async function verifyExternal(
    token: string,
  ): Promise<ClerkExternalEvidence | null> {
    if (!token || token.length > 16384) return null;
    try {
      const claims = await backend.verifyToken(token, config),
        current = now();
      if (
        claims.iss !== config.issuer ||
        typeof claims.sub !== "string" ||
        !claims.sub.startsWith("user_") ||
        typeof claims.sid !== "string" ||
        !claims.sid.startsWith("sess_") ||
        typeof claims.azp !== "string" ||
        !config.authorizedParties.includes(claims.azp) ||
        claims.act != null
      )
        return null;
      const audiences = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
      if (
        (config.tokenMode !== "session" &&
          !audiences.includes(config.audience)) ||
        typeof claims.exp !== "number" ||
        !Number.isFinite(claims.exp) ||
        claims.exp * 1000 <= current ||
        typeof claims.nbf !== "number" ||
        !Number.isFinite(claims.nbf) ||
        claims.nbf * 1000 > current ||
        typeof claims.iat !== "number" ||
        !Number.isFinite(claims.iat) ||
        claims.iat * 1000 > current
      )
        return null;
      let factorVerificationAge: [number, number] | null = null;
      if (claims.fva !== undefined) {
        if (
          !Array.isArray(claims.fva) ||
          claims.fva.length !== 2 ||
          claims.fva.some((value) => !Number.isInteger(value) || value < -1)
        )
          return null;
        factorVerificationAge = [claims.fva[0], claims.fva[1]];
      }
      const external = {
        issuer: config.issuer,
        subject: claims.sub,
        sessionID: claims.sid,
      };
      const session = await backend.session(external.sessionID);
      if (!liveSession(session, external, current)) return null;
      const user = await backend.user(external.subject);
      if (user.id !== external.subject || user.banned || user.locked)
        return null;
      return {
        user,
        external,
        expiresAt: Math.min(claims.exp * 1000, session.expireAt),
        factorVerificationAge,
      };
    } catch {
      return null;
    }
  }
  async function verify(token: string): Promise<ClerkEvidence | null> {
    const evidence = await verifyExternal(token);
    if (!evidence) return null;
    try {
      const subject = await mappings.localSubject(evidence.external),
        reference = await mappings.localSession(evidence.external);
      if (
        !subject ||
        !reference ||
        reference.subject !== subject ||
        !reference.sessionID
      )
        return null;
      return {
        reference,
        external: evidence.external,
        expiresAt: evidence.expiresAt,
        factorVerificationAge: evidence.factorVerificationAge,
      };
    } catch {
      return null;
    }
  }
  const authenticator: SessionAuthenticator = {
    async authenticate(headers) {
      // Explicit bearer transport avoids reinterpreting legacy cookies or application API keys.
      const match = /^Bearer ([^\s,]+)$/i.exec(
        headers.get("authorization") ?? "",
      );
      return match ? ((await verify(match[1]))?.reference ?? null) : null;
    },
  };
  const identity: IdentityDirectory = {
    async session(db, reference, options) {
      const external = await mappings.externalSession(db, reference, options);
      if (
        !external ||
        external.issuer !== config.issuer ||
        (await mappings.localSubject(external)) !== reference.subject
      )
        return null;
      const mapped = await mappings.localSession(external);
      if (
        mapped?.subject !== reference.subject ||
        mapped.sessionID !== reference.sessionID
      )
        return null;
      let session: ClerkSession, user: ClerkUser;
      try {
        session = await backend.session(external.sessionID);
        user = await backend.user(external.subject);
      } catch {
        return null;
      }
      if (!liveSession(session, external, now())) return null;
      const projected = await local.session(db, reference, options);
      if (
        projected?.subject !== reference.subject ||
        projected.sessionID !== reference.sessionID
      )
        return null;
      const checked = providerIdentity(projected, user, external);
      return checked ? { ...projected, ...checked } : null;
    },
    async subject(db, subject, options) {
      const external = await mappings.externalSubject(db, subject, options);
      if (
        !external ||
        external.issuer !== config.issuer ||
        (await mappings.localSubject(external)) !== subject
      )
        return null;
      let user: ClerkUser;
      try {
        user = await backend.user(external.subject);
      } catch {
        return null;
      }
      const projected = await local.subject(db, subject, options);
      return projected?.subject === subject
        ? providerIdentity(projected, user, external)
        : null;
    },
    async membership(db, subject, customerID, options) {
      try {
        const external = await mappings.externalSubject(db, subject, options);
        const organization = await mappings.externalOrganization(
          db,
          customerID,
          options,
        );
        if (
          !external ||
          external.issuer !== config.issuer ||
          !organization ||
          organization.issuer !== config.issuer ||
          (await mappings.localSubject(external)) !== subject
        )
          return null;
        const member = await backend.membership(
          organization.organizationID,
          external.subject,
        );
        if (
          !member ||
          !member.id ||
          member.subject !== external.subject ||
          member.organizationID !== organization.organizationID ||
          !/^org:[a-z0-9_]+$/.test(member.role)
        )
          return { memberID: null, role: null, permissions: [] };
        return {
          memberID: member.id,
          role: member.role === "org:admin" ? "admin" : "member",
          permissions: Array.isArray(member.permissions)
            ? member.permissions.filter(
                (permission) => typeof permission === "string",
              )
            : [],
        };
      } catch {
        return null;
      }
    },
  };
  return { verifyExternal, verify, authenticator, identity };
}
