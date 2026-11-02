import { consoleAuthConfiguration } from "./console-configuration";
import { AsyncLocalStorage } from "node:async_hooks";
import { APIError } from "better-auth/api";
import { authDatabase as database } from "../../db/runtime";
import { currentMCPClaims } from "./mcp";

// Only the authorized tenant service can enter this scope; never read from HTTP.
export const tenantInvitationScope = new AsyncLocalStorage<string>();
export async function guardOrganizationRequest(
  path: string,
  body: Record<string, unknown> = {},
  query: Record<string, unknown> = {},
  subject?: string,
) {
  if (!path.startsWith("/organization/")) return;
  const deny = () => {
    throw new APIError("FORBIDDEN", {
      message: "Use your tenant portal to manage customer access.",
    });
  };
  const user = subject
    ? (
        await database.query(
          'SELECT id,email,role,banned,"emailVerified","mustChangePassword" FROM webdock_auth."user" WHERE id=$1',
          [subject],
        )
      ).rows[0]
    : null;
  if (!user || user.banned || !user.emailVerified || user.mustChangePassword)
    deny();
  if(consoleAuthConfiguration().issuer){
    const organizationID=body.organizationId||query.organizationId;
    if(typeof organizationID==="string"&&(await database.query('SELECT 1 FROM webdock_auth.clerk_organization WHERE organization_id=$1',[organizationID])).rowCount)deny();
    if(typeof body.invitationId==="string"&&(await database.query('SELECT 1 FROM webdock_auth.invitation i JOIN webdock_auth.clerk_organization o ON o.organization_id=i."organizationId" WHERE i.id=$1',[body.invitationId])).rowCount)deny();
  }
  const operator = !(await currentMCPClaims(subject)).disabled;
  // Keep organization mutations behind the service's explicit customer-ID checks.
  const allowed = [
    "/organization/get-invitation",
    "/organization/accept-invitation",
    "/organization/reject-invitation",
  ];
  if (path === "/organization/create" && operator) return;
  if (path === "/organization/invite-member") {
    if (!operator && tenantInvitationScope.getStore() !== body.organizationId)
      deny();
    if (!["admin", "member"].includes(String(body.role))) deny();
    const target = (
      await database.query(
        'SELECT role,banned FROM webdock_auth."user" WHERE lower(email)=lower($1)',
        [String(body.email || "")],
      )
    ).rows[0];
    if (target && (target.role !== "user" || target.banned)) deny();
    const tenant = (
      await database.query(
        "SELECT c.status FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.organization_id=$1",
        [body.organizationId],
      )
    ).rows[0];
    if (tenant && tenant.status !== "active") deny();
    return;
  }
  if (!allowed.includes(path)) deny();
  if (
    [
      "/organization/get-invitation",
      "/organization/accept-invitation",
      "/organization/reject-invitation",
    ].includes(path)
  ) {
    const invite = (
      await database.query(
        'SELECT i.email,c.status FROM webdock_auth.invitation i LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=i."organizationId" LEFT JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE i.id=$1',
        [body.invitationId || query.id],
      )
    ).rows[0];
    if (
      !invite ||
      invite.email.toLowerCase() !== user.email.toLowerCase() ||
      (invite.status && invite.status !== "active") ||
      user.role !== "user"
    )
      deny();
  }
}
