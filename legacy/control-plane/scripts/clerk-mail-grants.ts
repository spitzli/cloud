import type { ClerkClient } from "@clerk/backend";
import type { PoolClient } from "pg";

type GrantReport = {
  mailboxID: string;
  subject: string;
  customerID: string;
  action: "unchanged" | "pending" | "convert" | "converted";
  reason?: string;
};
/** Offline importer only. Reads provider authority; never creates membership or restores disabled grants. */
export async function migrateClerkMailGrants(input: {
  db: Pick<PoolClient, "query">;
  issuer: string;
  users: ReadonlyMap<string, string>;
  teams: ReadonlyArray<{
    organizationID: string;
    clerkOrganizationID?: string;
  }>;
  provider: ClerkClient;
  dryRun: boolean;
  /** Includes reviewed accounts whose provider user has not been created in a dry run. */
  subjects?: readonly string[];
}): Promise<GrantReport[]> {
  const { db, issuer, users, teams, provider, dryRun } = input;
  if (
    !(
      await db.query(
        "SELECT to_regclass('webdock_mail.mailbox_grant') IS NOT NULL AS present",
      )
    ).rows[0]?.present
  )
    return [];
  const rows = (
    await db.query(
      `SELECT g.*,t.organization_id,u.role,u.banned,c.status,
    m.id AS live_member_id,i.clerk_user_id,i.enabled AS identity_enabled,
    o.clerk_organization_id,o.enabled AS organization_enabled
    FROM webdock_mail.mailbox_grant g
    JOIN webdock_auth."user" u ON u.id=g.subject
    JOIN webdock_auth.tenant_customer t ON t.customer_id=g.customer_id
    JOIN webdock_admin.customers c ON c.id=g.customer_id
    LEFT JOIN webdock_auth.member m ON m.id=g.member_id AND m."userId"=g.subject AND m."organizationId"=t.organization_id
    LEFT JOIN webdock_auth.clerk_identity i ON i.user_id=g.subject AND i.issuer=$2
    LEFT JOIN webdock_auth.clerk_organization o ON o.organization_id=t.organization_id AND o.issuer=$2
    WHERE g.subject=ANY($1::text[]) ORDER BY g.customer_id,g.mailbox_id,g.subject`,
      [input.subjects ?? [...users.keys()], issuer],
    )
  ).rows;
  const report: GrantReport[] = [];
  const plans = new Map<
    string,
    Array<{
      mailbox_id: string;
      subject: string;
      revision: string;
      member_id: string | null;
      operator_grant: boolean;
      clerk_grant: string;
    }>
  >();
  const verified = new Map<
    string,
    Awaited<ReturnType<typeof provider.users.getUser>>
  >();
  const memberships = new Map<
    string,
    Array<{ id: string; organization: { id: string }; role: string }>
  >();
  for (const row of rows) {
    const entry: GrantReport = {
      mailboxID: row.mailbox_id,
      subject: row.subject,
      customerID: row.customer_id,
      action: "unchanged",
    };
    report.push(entry);
    if (!row.enabled || row.clerk_grant) {
      entry.reason = row.enabled
        ? "already provider-qualified"
        : "disabled grant retained";
      continue;
    }
    const team = teams.find((t) => t.organizationID === row.organization_id);
    if (!team) {
      entry.reason = "customer outside reviewed teams";
      continue;
    }
    if (
      row.banned ||
      row.status !== "active" ||
      !["operator", "user"].includes(row.role) ||
      (!row.live_member_id && !(row.operator_grant && row.role === "operator"))
    ) {
      entry.reason = "local access suspended or original membership revoked";
      continue;
    }
    const userID = users.get(row.subject),
      organizationID = team.clerkOrganizationID;
    if (!userID || !organizationID) {
      if (!dryRun)
        throw Error("Missing reviewed provider mapping for Mail grants.");
      entry.action = "pending";
      entry.reason = "provider identity or team will be created";
      continue;
    }
    if (row.identity_enabled === false || row.organization_enabled === false) {
      entry.reason = "disabled provider mapping retained";
      continue;
    }
    if (
      (row.clerk_user_id && row.clerk_user_id !== userID) ||
      (row.clerk_organization_id &&
        row.clerk_organization_id !== organizationID)
    )
      throw Error("Conflicting provider mapping for Mail grants.");
    if (!dryRun && (!row.clerk_user_id || !row.clerk_organization_id))
      throw Error("Apply identity and team mappings before Mail grants.");
    let user = verified.get(userID);
    if (!user) {
      user = await provider.users.getUser(userID);
      verified.set(userID, user);
    }
    if (
      user.banned ||
      user.locked ||
      !user.emailAddresses.some(
        (e) =>
          e.id === user!.primaryEmailAddressId &&
          e.verification?.status === "verified",
      )
    ) {
      entry.reason = "provider account suspended or unverified";
      continue;
    }
    let members = memberships.get(userID);
    if (!members) {
      members = [];
      for (let offset = 0; ; offset += 100) {
        const page = await provider.users.getOrganizationMembershipList({
          userId: userID,
          limit: 100,
          offset,
        });
        members.push(...page.data);
        if (offset + page.data.length >= page.totalCount) break;
        if (!page.data.length || offset >= 10000)
          throw Error("Incomplete provider membership inventory.");
      }
      memberships.set(userID, members);
    }
    const member = members.find(
      (m) =>
        m.organization.id === organizationID &&
        ["org:admin", "org:member"].includes(m.role),
    );
    if (!member && !(row.operator_grant && row.role === "operator")) {
      if (dryRun && !row.clerk_organization_id) {
        entry.action = "pending";
        entry.reason = "reviewed provider membership will be created";
      } else
        entry.reason = "provider membership removed; explicit regrant required";
      continue;
    }
    const clerkGrant = JSON.stringify([
      issuer,
      userID,
      member ? organizationID : null,
      member?.id ?? null,
    ]);
    entry.action = "convert";
    const group = plans.get(row.customer_id) ?? [];
    group.push({
      mailbox_id: row.mailbox_id,
      subject: row.subject,
      revision: String(row.revision),
      member_id: row.member_id,
      operator_grant: row.operator_grant,
      clerk_grant: clerkGrant,
    });
    plans.set(row.customer_id, group);
  }
  if (dryRun) return report;
  for (const [customerID, plan] of plans) {
    // One statement keeps grant conversion and its reconciliation operation atomic even with a Pool caller.
    const changed = await db.query(
      `WITH locked AS MATERIALIZED (
      SELECT pg_advisory_xact_lock(hashtextextended('mail-configuration:'||$1,0))
    ), planned AS (
      SELECT * FROM jsonb_to_recordset($2::jsonb) AS p(mailbox_id text,subject text,revision bigint,member_id text,operator_grant boolean,clerk_grant text)
    ), changed AS (
      UPDATE webdock_mail.mailbox_grant g SET clerk_grant=p.clerk_grant,member_id=NULL,operator_grant=false,revision=g.revision+1
      FROM planned p,locked WHERE g.customer_id=$1 AND g.mailbox_id=p.mailbox_id AND g.subject=p.subject
      AND g.enabled AND g.clerk_grant IS NULL AND g.revision=p.revision
      AND g.member_id IS NOT DISTINCT FROM p.member_id AND g.operator_grant=p.operator_grant
      AND EXISTS(SELECT 1 FROM webdock_auth."user" u JOIN webdock_auth.clerk_identity i ON i.user_id=u.id
        WHERE u.id=g.subject AND NOT coalesce(u.banned,false) AND u.role IN ('operator','user') AND i.enabled AND i.issuer=$3 AND i.clerk_user_id=(p.clerk_grant::jsonb->>1))
      AND EXISTS(SELECT 1 FROM webdock_auth.tenant_customer t JOIN webdock_auth.clerk_organization o ON o.organization_id=t.organization_id JOIN webdock_admin.customers c ON c.id=t.customer_id
        WHERE t.customer_id=g.customer_id AND c.status='active' AND o.issuer=$3 AND o.enabled AND ((p.clerk_grant::jsonb->>2) IS NULL OR o.clerk_organization_id=(p.clerk_grant::jsonb->>2)))
      AND ((p.operator_grant AND EXISTS(SELECT 1 FROM webdock_auth."user" u WHERE u.id=g.subject AND u.role='operator')) OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId" WHERE m.id=p.member_id AND m."userId"=g.subject AND t.customer_id=g.customer_id))
      RETURNING g.mailbox_id,g.subject
    ), revision AS (
      INSERT INTO webdock_mail.configuration(customer_id) SELECT $1 WHERE EXISTS(SELECT 1 FROM changed)
      ON CONFLICT(customer_id) DO UPDATE SET revision=configuration.revision+1 RETURNING revision
    ), superseded AS (
      UPDATE webdock_mail.configuration_operation SET state='superseded',finished_at=now() WHERE customer_id=$1 AND state='pending' AND EXISTS(SELECT 1 FROM revision)
    ), queued AS (
      INSERT INTO webdock_mail.configuration_operation(customer_id,revision,requested_by,reason)
      SELECT $1,revision,NULL,'clerk_grants_migrated' FROM revision RETURNING id
    ) SELECT mailbox_id,subject FROM changed`,
      [customerID, JSON.stringify(plan), issuer],
    );
    for (const item of report.filter(
      (r) => r.customerID === customerID && r.action === "convert",
    )) {
      if (
        changed.rows.some(
          (r) => r.mailbox_id === item.mailboxID && r.subject === item.subject,
        )
      )
        item.action = "converted";
      else {
        item.action = "unchanged";
        item.reason = "grant or authorization changed during review";
      }
    }
  }
  return report;
}
