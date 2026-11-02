import { consoleAuthConfiguration } from "./console-configuration";
import { database, authDatabase } from "../../db/runtime";
import { clerkProvider, liveMemberships } from "./clerk-provider";
import { currentConsolePrincipal } from "./principal";
import { ConsoleAuthError } from "./clerk-identity";
import { invitationRole, validateMemberChange } from "./clerk-teams-policy";
export async function linkedOrganization(organizationID: string) {
  if (!consoleAuthConfiguration().issuer) return null;
  return (
    await authDatabase.query(
      "SELECT issuer,clerk_organization_id,enabled FROM webdock_auth.clerk_organization WHERE issuer=$1 AND organization_id=$2",
      [consoleAuthConfiguration().issuer, organizationID],
    )
  ).rows[0] as
    | { issuer: string; clerk_organization_id: string; enabled: boolean }
    | undefined;
}
async function members(organizationId: string) {
  const provider = clerkProvider(),
    all = [];
  for (let offset = 0; ; offset += 100) {
    const page = await provider.organizations.getOrganizationMembershipList({
      organizationId,
      limit: 100,
      offset,
    });
    all.push(...page.data);
    if (all.length >= page.totalCount || !page.data.length) return all;
    if (offset >= 10000) throw new ConsoleAuthError(503, "Team unavailable.");
  }
}
export async function clerkTeamView(organizationID: string) {
  const link = await linkedOrganization(organizationID);
  if (!link?.enabled)
    throw new ConsoleAuthError(403, "Customer team is not linked.");
  const [team, invitations] = await Promise.all([
    members(link.clerk_organization_id),
    clerkProvider().organizations.getOrganizationInvitationList({
      organizationId: link.clerk_organization_id,
      limit: 100,
    }),
  ]);
  return {
    members: team.map((m) => ({
      id: m.id,
      role: m.role === "org:admin" ? "admin" : "member",
      name: [m.publicUserData?.firstName, m.publicUserData?.lastName]
        .filter(Boolean)
        .join(" "),
      email: m.publicUserData?.identifier || "",
      banned: null,
      emailVerified: true,
      mustChangePassword: false,
    })),
    invitations: invitations.data.map((i) => ({
      id: i.id,
      email: i.emailAddress,
      role: i.role === "org:admin" ? "admin" : "member",
      status: i.status || "pending",
      expiresAt: new Date(i.expiresAt).toISOString(),
    })),
  };
}
export async function manageClerkTeam(
  customerID: string,
  organizationID: string,
  input: Record<string, string>,
) {
  const principal = currentConsolePrincipal();
  if (!principal) throw new ConsoleAuthError(401, "Sign in again.");
  const db = await database.connect();
  try {
    await db.query("BEGIN");
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      "clerk-team:" + organizationID,
    ]);
    const customer = (
      await db.query(
        "SELECT c.status,t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 FOR SHARE OF c",
        [customerID],
      )
    ).rows[0];
    const link = await linkedOrganization(organizationID);
    if (
      !customer ||
      customer.status !== "active" ||
      customer.organization_id !== organizationID ||
      !link?.enabled
    )
      throw new ConsoleAuthError(403, "Active customer team required.");
    const provider = clerkProvider(),
      organizationId = link.clerk_organization_id;
    if (!principal.operator) {
      const own = (await liveMemberships(principal.clerkUserID)).find(
        (m) => m.organization.id === organizationId,
      );
      if (principal.customerID !== customerID || own?.role !== "org:admin")
        throw new ConsoleAuthError(403, "Customer administrator required.");
    }
    if (["role", "remove"].includes(input.action)) {
      const team = await members(organizationId);
      const target = validateMemberChange(
          team,
          input.member,
          input.action,
          input.role,
        ),
        userId = team.find((m) => m.id === target.id)?.publicUserData?.userId;
      if (!userId) throw new ConsoleAuthError(404, "Member unavailable.");
      if (input.action === "remove")
        await provider.organizations.deleteOrganizationMembership({
          organizationId,
          userId,
        });
      else
        await provider.organizations.updateOrganizationMembership({
          organizationId,
          userId,
          role: invitationRole(input.role),
        });
    } else if (input.action === "invite") {
      const emailAddress = (input.email || "").trim();
      if (emailAddress.length > 254 || !/^\S+@\S+\.\S+$/.test(emailAddress))
        throw new ConsoleAuthError(400, "Enter a valid email address.");
      await provider.organizations.createOrganizationInvitation({
        organizationId,
        emailAddress,
        role: invitationRole(input.role),
        redirectUrl: new URL("/sign-up", process.env.WEBDOCK_CONSOLE_ORIGIN)
          .href,
      });
    } else if (["cancel", "resend"].includes(input.action)) {
      const invitations =
        await provider.organizations.getOrganizationInvitationList({
          organizationId,
          limit: 100,
          status: ["pending"],
        });
      const invitation = invitations.data.find(
        (i) => i.id === input.invitation,
      );
      if (!invitation)
        throw new ConsoleAuthError(404, "Pending invitation unavailable.");
      // Clerk replaces the pending invitation on resend; no legacy account is created.
      if (input.action === "cancel")
        await provider.organizations.revokeOrganizationInvitation({
          organizationId,
          invitationId: invitation.id,
        });
      else
        await provider.organizations.createOrganizationInvitation({
          organizationId,
          emailAddress: invitation.emailAddress,
          role: invitation.role,
          redirectUrl: new URL("/sign-up", process.env.WEBDOCK_CONSOLE_ORIGIN)
            .href,
        });
    } else throw new ConsoleAuthError(400, "Unknown team action.");
    await db.query(
      "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,$3,'succeeded')",
      [principal.userID, "clerk-team-" + input.action, customerID],
    );
    await db.query("COMMIT");
    return { message: "Team updated." };
  } catch (error) {
    await db.query("ROLLBACK");
    throw error;
  } finally {
    db.release();
  }
}
/** Synchronous operator action; stable slug/metadata reconcile a partial provider create. */
export async function linkClerkTeam(
  customerID: string,
  organizationID: string,
) {
  const principal = currentConsolePrincipal();
  if (!principal?.operator)
    throw new ConsoleAuthError(403, "Platform operator access is required.");
  const db = await database.connect();
  try {
    await db.query("BEGIN");
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      "clerk-team:" + organizationID,
    ]);
    const local = (
      await db.query(
        `SELECT c.name FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 AND t.organization_id=$2 AND c.status='active' FOR SHARE OF c`,
        [customerID, organizationID],
      )
    ).rows[0];
    if (!local)
      throw new ConsoleAuthError(403, "Active customer team required.");
    const link = await linkedOrganization(organizationID),
      provider = clerkProvider(),
      slug = "webdock-" + organizationID;
    if (link && !link.enabled)
      throw new ConsoleAuthError(403, "Customer team is disabled.");
    let org;
    try {
      org = await provider.organizations.getOrganization(
        link ? { organizationId: link.clerk_organization_id } : { slug },
      );
    } catch (error) {
      if ((error as { status?: number }).status !== 404 || link) throw error;
    }
    if (
      org &&
      !link &&
      org.privateMetadata.webdockOrganizationID !== organizationID
    )
      throw new ConsoleAuthError(
        409,
        "Review the existing provider team before linking.",
      );
    if (!org)
      org = await provider.organizations.createOrganization({
        name: local.name,
        slug,
        privateMetadata: {
          webdockOrganizationID: organizationID,
          webdockCustomerID: customerID,
        },
      });
    await db.query(
      "INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
      [principal.issuer, org.id, organizationID],
    );
    const mapped = (
      await db.query(
        "SELECT clerk_organization_id FROM webdock_auth.clerk_organization WHERE issuer=$1 AND organization_id=$2",
        [principal.issuer, organizationID],
      )
    ).rows[0];
    if (mapped?.clerk_organization_id !== org.id)
      throw new ConsoleAuthError(409, "Customer team mapping changed.");
    await db.query("COMMIT");
    return { message: "Changes saved." };
  } catch (error) {
    await db.query("ROLLBACK");
    throw error;
  } finally {
    db.release();
  }
}
