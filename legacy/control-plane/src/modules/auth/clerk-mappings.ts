import type { Pool } from "pg";
import type { ClerkMappings, ExternalIdentity } from "@webdock/auth-clerk";
import type { DatabaseReader } from "@webdock/core/identity";

export function createClerkMappings(database: DatabaseReader): ClerkMappings {
  return {
    async externalOrganization(_db, customerID) {
      const rows = (
        await database.query<{ issuer: string; clerk_organization_id: string }>(
          `SELECT o.issuer,o.clerk_organization_id FROM webdock_auth.clerk_organization o JOIN webdock_auth.tenant_customer t ON t.organization_id=o.organization_id JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.customer_id=$1 AND o.enabled AND c.status='active'`,
          [customerID],
        )
      ).rows;
      return rows.length === 1
        ? {
            issuer: rows[0].issuer,
            organizationID: rows[0].clerk_organization_id,
          }
        : null;
    },
    async localSubject(external) {
      return (
        (
          await database.query<{ local_subject: string }>(
            "SELECT local_subject FROM webdock_auth.clerk_identity_mapping WHERE issuer=$1 AND external_subject=$2 AND revoked_at IS NULL",
            [external.issuer, external.subject],
          )
        ).rows[0]?.local_subject ?? null
      );
    },
    async localSession(external) {
      const row = (
        await database.query<{
          local_subject: string;
          local_session_id: string;
        }>(
          `SELECT m.local_subject,s.local_session_id FROM webdock_auth.clerk_session_mapping s JOIN webdock_auth.clerk_identity_mapping m USING(issuer,external_subject) JOIN webdock_auth.session p ON p.id=s.local_session_id AND p."userId"=m.local_subject WHERE s.issuer=$1 AND s.external_subject=$2 AND s.external_session=$3 AND m.revoked_at IS NULL AND s.expires_at>now() AND p."expiresAt">now()`,
          [external.issuer, external.subject, external.sessionID],
        )
      ).rows[0];
      return row
        ? { subject: row.local_subject, sessionID: row.local_session_id }
        : null;
    },
    async externalSubject(_db, subject) {
      const row = (
        await database.query<{ issuer: string; external_subject: string }>(
          "SELECT issuer,external_subject FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1 AND revoked_at IS NULL",
          [subject],
        )
      ).rows;
      // Selection must be unambiguous even if an offline operator enrolled two issuers.
      return row.length === 1
        ? { issuer: row[0].issuer, subject: row[0].external_subject }
        : null;
    },
    async externalSession(_db, reference) {
      const row = (
        await database.query<{
          issuer: string;
          external_subject: string;
          external_session: string;
        }>(
          `SELECT s.issuer,s.external_subject,s.external_session FROM webdock_auth.clerk_session_mapping s JOIN webdock_auth.clerk_identity_mapping m USING(issuer,external_subject) WHERE s.local_session_id=$1 AND m.local_subject=$2 AND m.revoked_at IS NULL AND s.expires_at>now()`,
          [reference.sessionID, reference.subject],
        )
      ).rows[0];
      return row
        ? {
            issuer: row.issuer,
            subject: row.external_subject,
            sessionID: row.external_session,
          }
        : null;
    },
  };
}

/** Offline enrollment only. An existing issuer/subject binding can never be reassigned or revived. */
export async function enrollClerkIdentity(
  database: Pick<Pool, "connect">,
  input: ExternalIdentity & { localSubject: string },
) {
  const issuer = new URL(input.issuer);
  if (
    issuer.protocol !== "https:" ||
    issuer.origin !== input.issuer ||
    !/^user_[A-Za-z0-9]+$/.test(input.subject) ||
    !/^[1-9][0-9]{0,18}$/.test(input.localSubject)
  )
    throw Error("Invalid explicit Clerk identity mapping");
  const db = await database.connect();
  try {
    await db.query("BEGIN");
    if (
      !(
        await db.query(
          'SELECT id FROM webdock_auth."user" WHERE id=$1 FOR SHARE',
          [input.localSubject],
        )
      ).rowCount
    )
      throw Error("Local identity must be provisioned first");
    await db.query(
      "INSERT INTO webdock_auth.clerk_identity_mapping(issuer,external_subject,local_subject) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
      [input.issuer, input.subject, input.localSubject],
    );
    const bound = (
      await db.query<{ local_subject: string; revoked_at: Date | null }>(
        "SELECT local_subject,revoked_at FROM webdock_auth.clerk_identity_mapping WHERE issuer=$1 AND external_subject=$2 FOR SHARE",
        [input.issuer, input.subject],
      )
    ).rows[0];
    if (
      !bound ||
      bound.local_subject !== input.localSubject ||
      bound.revoked_at
    )
      throw Error(
        "Clerk identity binding conflicts or was revoked; automatic rebinding is forbidden",
      );
    await db.query("COMMIT");
  } catch (error) {
    await db.query("ROLLBACK");
    throw error;
  } finally {
    db.release();
  }
}
