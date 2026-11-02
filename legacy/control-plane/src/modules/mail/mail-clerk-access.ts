import { currentConsolePrincipal } from "../auth/principal";
import { consoleAuthConfiguration } from "../auth/console-configuration";
import { currentDatabaseScope } from "@webdock/database-runtime";
import { authDatabase } from "../../db/runtime";
import { clerkProvider, liveMemberships } from "../auth/clerk-provider";
import { isClerkOrganizationRole } from "../auth/organization-permissions";
import type { Connection } from "../hosting/authorization";

/** Undefined selects Better Auth; null denies access in selected Clerk. */
export async function clerkMailAccess(
  db: Connection,
  subject: string,
  customerID: string,
) {
  const config = consoleAuthConfiguration();
  if (!config.requireExistingMapping && !currentConsolePrincipal())
    return undefined;
  const identities =
    currentDatabaseScope()?.kind === "system" ? db : authDatabase;
  if (
    !(
      await identities.query(
        "SELECT to_regclass('webdock_auth.clerk_identity') IS NOT NULL AS present",
      )
    ).rows[0]?.present
  )
    return null;
  const link = (
    await identities.query(
      `SELECT i.*,u.banned FROM webdock_auth.clerk_identity i JOIN webdock_auth."user" u ON u.id=i.user_id WHERE i.user_id=$1`,
      [subject],
    )
  ).rows[0];
  if (!link || !link.enabled || link.banned) return null;
  if (!config.issuer || link.issuer !== config.issuer)
    throw Error("Mail identity verification is not configured.");
  const user = await clerkProvider().users.getUser(link.clerk_user_id);
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
    return null;
  const tenant = (
    await db.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0];
  if (!tenant) return null;
  const organization = (
    await identities.query(
      "SELECT * FROM webdock_auth.clerk_organization WHERE issuer=$1 AND organization_id=$2 AND enabled",
      [link.issuer, tenant.organization_id],
    )
  ).rows[0];
  if (!organization) return null;
  const member = organization
    ? (await liveMemberships(link.clerk_user_id)).find(
        (m) =>
          m.organization.id === organization.clerk_organization_id &&
          isClerkOrganizationRole(m.role),
      )
    : undefined;
  const operator = user.privateMetadata?.webdockRole === "operator";
  if (!member && !operator) return null;
  return {
    operator,
    canManage: operator || member?.role === "org:admin",
    // Membership IDs prevent a removed-and-reinvited member inheriting an old grant.
    clerkGrant: JSON.stringify([
      link.issuer,
      link.clerk_user_id,
      member ? organization.clerk_organization_id : null,
      member?.id ?? null,
    ]),
  };
}

/** Call only after management authorization. Live organization members and explicitly eligible operators only. */
export async function clerkMailPeople(
  db: Connection,
  customerID: string,
  operator = false,
) {
  const config = consoleAuthConfiguration();
  if (!config.requireExistingMapping && !currentConsolePrincipal())
    return undefined;
  if (
    !(
      await authDatabase.query(
        "SELECT to_regclass('webdock_auth.clerk_identity') IS NOT NULL AS present",
      )
    ).rows[0]?.present
  )
    return [];
  const tenant = (
    await db.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0];
  const organization =
    tenant &&
    (
      await authDatabase.query(
        "SELECT * FROM webdock_auth.clerk_organization WHERE organization_id=$1",
        [tenant.organization_id],
      )
    ).rows[0];
  if (!organization) return [];
  if (!organization.enabled) return [];
  if (!config.issuer || organization.issuer !== config.issuer)
    throw Error("Mail identity verification is not configured.");
  const members: string[] = [];
  const provider = clerkProvider();
  for (let offset = 0; ; offset += 100) {
    const page = await provider.organizations.getOrganizationMembershipList({
      organizationId: organization.clerk_organization_id,
      limit: 100,
      offset,
    });
    members.push(
      ...page.data
        .filter((m) => isClerkOrganizationRole(m.role))
        .map((m) => m.publicUserData?.userId)
        .filter((id): id is string => typeof id === "string"),
    );
    if (offset + page.data.length >= page.totalCount) break;
    if (!page.data.length || offset >= 10000)
      throw Error("Mail membership verification is unavailable.");
  }
  const candidates = (
    await authDatabase.query<{
      id: string;
      name: string;
      email: string;
      clerk_user_id: string;
    }>(
      `SELECT u.id,u.name,u.email,i.clerk_user_id FROM webdock_auth.clerk_identity i JOIN webdock_auth."user" u ON u.id=i.user_id WHERE i.issuer=$1 AND (i.clerk_user_id=ANY($2::text[]) OR $3) AND i.enabled AND NOT coalesce(u.banned,false) ORDER BY u.name LIMIT 128`,
      [organization.issuer, members, operator],
    )
  ).rows;
  const eligible = await Promise.all(
    candidates.map(async (candidate) => {
      const user = await provider.users.getUser(candidate.clerk_user_id);
      if (
        user.id !== candidate.clerk_user_id ||
        user.banned ||
        user.locked ||
        !user.emailAddresses.some(
          (email) =>
            email.id === user.primaryEmailAddressId &&
            email.verification?.status === "verified",
        ) ||
        (!members.includes(candidate.clerk_user_id) &&
          !(operator && user.privateMetadata?.webdockRole === "operator"))
      )
        return null;
      const { id, name, email } = candidate;
      return { id, name, email };
    }),
  );
  return eligible.filter((user) => user !== null);
}
