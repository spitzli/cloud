import { ConsoleAuthError } from "./clerk-identity";
import { currentConsolePrincipal, consoleMembership } from "./principal";
import { legacyClerkMemberships, linkedMembershipRole } from "./clerk-provider";
import { authDatabase as database } from "../../db/runtime";

export async function currentClaims(
  subject: string | undefined,
  binding: unknown,
) {
  if (!subject || typeof binding !== "string") return { disabled: true };
  const row = (
    await database.query(
      `SELECT u.id,u.email,u.name,u.role,u.banned,u."emailVerified",u."twoFactorEnabled",u."mustChangePassword",b.id AS binding_id,b.enabled,b.organization_id,b.client_id,c.disabled AS client_disabled
 FROM webdock_auth."user" u CROSS JOIN webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id WHERE u.id=$1 AND b.id=$2`,
      [subject, binding],
    )
  ).rows[0];
  const principal = currentConsolePrincipal();
  if (principal && principal.userID !== subject) return { disabled: true };
  if (
    !row ||
    row.banned ||
    (!principal && row.mustChangePassword) ||
    !row.enabled ||
    row.client_disabled
  )
    return { disabled: true };
  let legacyProof;
  if (!principal) {
    try {
      legacyProof = await legacyClerkMemberships(subject);
    } catch (error) {
      if (error instanceof ConsoleAuthError && error.status === 403)
        return { disabled: true };
      throw error;
    }
  }
  if (!principal && !legacyProof && !row.emailVerified)
    return { disabled: true };
  const operator = principal
    ? principal.operator
    : legacyProof
      ? legacyProof.operator
      : row.role === "operator";
  if (operator)
    return principal?.operator ||
      (legacyProof ? legacyProof.twoFactorEnabled : row.twoFactorEnabled)
      ? {
          webdock_role: "operator",
          webdock_email: row.email,
          webdock_name: row.name,
        }
      : { disabled: true };
  // Studio is the global customer portal; website grants and CMS availability
  // remain independent from this membership-based reader identity.
  if (
    process.env.WEBDOCK_STUDIO_CLIENT_ID &&
    row.client_id === process.env.WEBDOCK_STUDIO_CLIENT_ID &&
    !row.organization_id
  ) {
    const proof = legacyProof;
    const member = principal
      ? !!principal.customerID
      : proof
        ? proof.roles.length > 0
        : (
            await database.query(
              `SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE m."userId"=$1 AND c.status='active' LIMIT 1`,
              [subject],
            )
          ).rowCount;
    return member &&
      (principal || legacyProof ? !operator : row.role === "user")
      ? {
          webdock_role: "reader",
          webdock_email: row.email,
          webdock_name: row.name,
        }
      : { disabled: true };
  }
  if (!row.organization_id) return { disabled: true };
  if (
    (
      await database.query(
        "SELECT settings FROM webdock_auth.platform_settings WHERE id=true",
      )
    ).rows[0]?.settings?.cmsEnabled === false
  )
    return { disabled: true };
  if (
    process.env.DATABASE_RLS_SECRET &&
    !(
      await database.query(
        "SELECT webdock_rls.billing_cms_allowed($1) AS allowed",
        [binding],
      )
    ).rows[0].allowed
  )
    return { disabled: true };
  // Only mapped, active customer administrators inherit website access.
  // An explicit revoked or mismatched site grant still fails closed.
  const mappedRole = principal
    ? (consoleMembership(subject, row.organization_id)?.role ?? null)
    : await linkedMembershipRole(subject, row.organization_id);
  const memberSource =
    mappedRole === undefined
      ? "webdock_auth.member"
      : `(SELECT $1::text AS "userId",$3::text AS "organizationId",$4::text AS role WHERE $4::text IS NOT NULL)`;
  const access = (
    await database.query(
      `SELECT CASE WHEN c.status='active' AND m.role IN ('owner','admin') THEN 'admin' ELSE g.role END AS role
 FROM ${memberSource} m
 LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId"
 LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id
 LEFT JOIN webdock_auth.project_grant g ON g.user_id=m."userId" AND g.binding_id=$2
 WHERE m."userId"=$1 AND m."organizationId"=$3
 AND (t.customer_id IS NULL OR c.status='active')
 AND (g.id IS NULL OR (g.enabled=true AND g.organization_id=$3))
 AND ((c.status='active' AND m.role IN ('owner','admin')) OR g.enabled=true)`,
      [
        subject,
        binding,
        row.organization_id,
        ...(mappedRole === undefined ? [] : [mappedRole]),
      ],
    )
  ).rows[0];
  return access
    ? {
        webdock_role: access.role,
        webdock_email: row.email,
        webdock_name: row.name,
      }
    : { disabled: true };
}
