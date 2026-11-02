import { readAuthProtocolSelection } from "@webdock/auth-configuration/protocols";
import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { betterAuthIdentity } from "@webdock/auth-better-auth";
import { currentDatabaseScope } from "@webdock/database-runtime";
import type {
  DatabaseReader,
  IdentityReadOptions,
} from "@webdock/core/identity";

/** Only authenticated OAuth composition installs this exact-session, live Clerk proof. */
export async function oauthCompatibilityIdentity(
  db: DatabaseReader,
  subject: string,
  sessionID?: string,
  options?: IdentityReadOptions,
) {
  if (!readAuthProtocolSelection(process.env).compatibility) return undefined;
  const scope = currentDatabaseScope();
  if (scope?.kind !== "session" || !scope.memberships) return undefined;
  const proof = scope.memberships;
  const config = readAuthProviderConfiguration(
    process.env.WEBDOCK_AUTH_PROVIDER,
  );
  const now = Date.now();
  if (
    scope.authProvider !== "clerk" ||
    config.provider !== "clerk" ||
    scope.subject !== subject ||
    !scope.sessionID ||
    (sessionID !== undefined && scope.sessionID !== sessionID) ||
    proof.subject !== subject ||
    proof.sessionID !== scope.sessionID ||
    proof.issuer !== config.issuer ||
    !proof.userID ||
    typeof proof.operator !== "boolean" ||
    typeof proof.twoFactorEnabled !== "boolean" ||
    !Number.isFinite(proof.verifiedAt) ||
    !Number.isFinite(proof.expiresAt) ||
    proof.verifiedAt > now ||
    proof.verifiedAt < now - 60_000 ||
    proof.expiresAt <= now ||
    proof.expiresAt > proof.verifiedAt + 60_000
  )
    return null;
  // Immutable account links are bootstrap-only; business SQL keeps its existing RLS policies.
  const { authDatabase } = await import("../../db/runtime");
  const mapped = (
    await authDatabase.query(
      "SELECT 1 FROM webdock_auth.clerk_identity WHERE issuer=$1 AND clerk_user_id=$2 AND user_id=$3 AND enabled AND legacy_eligible",
      [proof.issuer, proof.userID, subject],
    )
  ).rowCount;
  if (!mapped) return null;
  const local = await betterAuthIdentity.session(
    db,
    { subject, sessionID: scope.sessionID },
    options,
  );
  if (
    !local ||
    local.banned ||
    local.mustChangePassword ||
    (proof.operator && (!local.twoFactorEnabled || !proof.twoFactorEnabled))
  )
    return null;
  return {
    identity: {
      ...local,
      role: proof.operator ? "operator" : "user",
      emailVerified: true,
      twoFactorEnabled: local.twoFactorEnabled && proof.twoFactorEnabled,
      operatorAuthorized: false,
    },
    memberships: proof.roles,
  };
}
