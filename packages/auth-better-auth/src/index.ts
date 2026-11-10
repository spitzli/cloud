import type {
  IdentityDirectory,
  LocalIdentity,
  SessionAuthenticator,
} from "@spitzli/core/identity";
function identity(
  row: Record<string, unknown> | undefined,
): LocalIdentity | null {
  if (!row || typeof row.id !== "string" || !row.id) return null;
  return {
    subject: row.id,
    role: typeof row.role === "string" ? row.role : "",
    banned: row.banned === true,
    emailVerified: row.emailVerified === true,
    mustChangePassword: row.mustChangePassword === true,
    twoFactorEnabled: row.twoFactorEnabled === true,
  };
}
/** Existing local Better Auth projections. No app imports and no cached identity decisions. */
export const betterAuthIdentity: IdentityDirectory = {
  async session(db, reference, options) {
    if (!reference.subject || !reference.sessionID) return null;
    const row = (
      await db.query(
        `SELECT u.id,u.role,u.banned,u."emailVerified",u."mustChangePassword",u."twoFactorEnabled" FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id WHERE u.id=$1 AND s.id=$2 AND s."expiresAt">now()${options?.lock ? " FOR SHARE OF u,s" : ""}`,
        [reference.subject, reference.sessionID],
      )
    ).rows[0];
    const local = identity(row);
    if (!local) return null;
    const preview =
      (
        await db.query(
          "SELECT session_id FROM webdock_auth.studio_tenant_preview WHERE session_id=$1 AND actor_id=$2",
          [reference.sessionID, reference.subject],
        )
      ).rows.length > 0;
    return { ...local, sessionID: reference.sessionID, preview };
  },
  async subject(db, subject, options) {
    if (!subject) return null;
    return identity(
      (
        await db.query(
          `SELECT id,role,banned,"emailVerified","mustChangePassword","twoFactorEnabled" FROM webdock_auth."user" WHERE id=$1${options?.lock ? " FOR SHARE" : ""}`,
          [subject],
        )
      ).rows[0],
    );
  },
  async membership(db, subject, customerID, options) {
    const tenant = (
      await db.query(
        "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0];
    if (!tenant) return null;
    const member = (
      await db.query(
        `SELECT id,role FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2${options?.lock ? " FOR SHARE" : ""}`,
        [tenant.organization_id, subject],
      )
    ).rows[0];
    return { memberID: member?.id ?? null, role: member?.role ?? null };
  },
};
export function createBetterAuthAuthenticator(
  resolve: (
    headers: Headers,
  ) => Promise<{ user: { id: string }; session: { id: string } } | null>,
): SessionAuthenticator {
  return {
    async authenticate(headers) {
      const session = await resolve(headers);
      if (
        !session ||
        typeof session.user.id !== "string" ||
        !session.user.id ||
        typeof session.session.id !== "string" ||
        !session.session.id
      )
        return null;
      return { subject: session.user.id, sessionID: session.session.id };
    },
  };
}
