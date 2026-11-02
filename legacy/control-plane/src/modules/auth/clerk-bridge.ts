import { randomBytes } from "node:crypto";
import type { Pool } from "pg";
import type { createClerkAdapter } from "@webdock/auth-clerk";

export class ClerkSessionUnavailable extends Error {}
type ExternalEvidence = NonNullable<
  Awaited<ReturnType<ReturnType<typeof createClerkAdapter>["verifyExternal"]>>
>;
export type BridgeUser = {
  id: string;
  name: string;
  email: string;
  role: "operator" | "user";
  banned: boolean;
  emailVerified: boolean;
  twoFactorEnabled: boolean;
  mustChangePassword: boolean;
};
export function allowedClerkProjection(
  user: BridgeUser,
  evidence: ExternalEvidence,
): boolean {
  return (
    !user.banned &&
    evidence.user.emailVerified &&
    !user.mustChangePassword &&
    (evidence.user.role === "user" ||
      (evidence.user.role === "operator" &&
        evidence.user.twoFactorEnabled &&
        evidence.factorVerificationAge !== null &&
        evidence.factorVerificationAge[1] >= 0))
  );
}
/** Verified external identity becomes a short-lived local session; no native cookie reaches the browser. */
export function createClerkSessionBridge(dependencies: {
  database: Pick<Pool, "connect">;
  verifyExternal(token: string): Promise<ExternalEvidence | null>;
  signSession(token: string): Promise<string>;
  origin: string;
  now?: () => number;
}) {
  return async function clerkSession(headers: Headers) {
    const bearer = /^Bearer ([^\s,]+)$/i.exec(
      headers.get("authorization") ?? "",
    );
    if (!bearer) throw new ClerkSessionUnavailable();
    const evidence = await dependencies.verifyExternal(bearer[1]);
    const now = (dependencies.now ?? Date.now)();
    if (!evidence || evidence.expiresAt <= now)
      throw new ClerkSessionUnavailable();
    const db = await dependencies.database.connect();
    try {
      await db.query("BEGIN");
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        "clerk-session:" +
          evidence.external.issuer +
          ":" +
          evidence.external.sessionID,
      ]);
      const user = (
        await db.query<BridgeUser>(
          `SELECT u.id,u.name,u.email,u.role,u.banned,u."emailVerified",u."twoFactorEnabled",u."mustChangePassword" FROM webdock_auth.clerk_identity_mapping m JOIN webdock_auth."user" u ON u.id=m.local_subject WHERE m.issuer=$1 AND m.external_subject=$2 AND m.revoked_at IS NULL FOR SHARE OF m,u`,
          [evidence.external.issuer, evidence.external.subject],
        )
      ).rows[0];
      if (!user || !allowedClerkProjection(user, evidence))
        throw new ClerkSessionUnavailable();
      const existing = (
        await db.query<{
          local_session_id: string;
          external_subject: string;
          token: string;
          userId: string;
        }>(
          `SELECT m.local_session_id,m.external_subject,s.token,s."userId" FROM webdock_auth.clerk_session_mapping m JOIN webdock_auth.session s ON s.id=m.local_session_id WHERE m.issuer=$1 AND m.external_session=$2 FOR UPDATE OF m,s`,
          [evidence.external.issuer, evidence.external.sessionID],
        )
      ).rows[0];
      if (
        existing &&
        (existing.external_subject !== evidence.external.subject ||
          existing.userId !== user.id)
      )
        throw new ClerkSessionUnavailable();
      const expiresAt = new Date(Math.min(evidence.expiresAt, now + 60_000));
      const session = existing
        ? { id: existing.local_session_id, token: existing.token }
        : (
            await db.query<{ id: string; token: string }>(
              `INSERT INTO webdock_auth.session("userId",token,"expiresAt","updatedAt") VALUES($1,$2,$3,now()) RETURNING id,token`,
              [user.id, randomBytes(48).toString("base64url"), expiresAt],
            )
          ).rows[0];
      await db.query(
        "INSERT INTO webdock_auth.clerk_session_mapping(issuer,external_session,external_subject,local_session_id,expires_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(issuer,external_session) DO UPDATE SET expires_at=excluded.expires_at",
        [
          evidence.external.issuer,
          evidence.external.sessionID,
          evidence.external.subject,
          session.id,
          expiresAt,
        ],
      );
      if (existing)
        await db.query(
          `UPDATE webdock_auth.session SET "expiresAt"=$2,"updatedAt"=now() WHERE id=$1`,
          [session.id, expiresAt],
        );
      const cookie = await dependencies.signSession(session.token);
      await db.query("COMMIT");
      const { id, name, email, mustChangePassword } = user;
      const { role, emailVerified, twoFactorEnabled } = evidence.user;
      return {
        headers: new Headers({ Origin: dependencies.origin, Cookie: cookie }),
        actor: { userID: id, sessionID: session.id },
        safe: {
          user: {
            id,
            name,
            email,
            role,
            emailVerified,
            twoFactorEnabled,
            mustChangePassword,
          },
          operator: role === "operator",
        },
      };
    } catch (error) {
      await db.query("ROLLBACK");
      throw error;
    } finally {
      db.release();
    }
  };
}
