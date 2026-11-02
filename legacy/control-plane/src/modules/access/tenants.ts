import {organizationCapabilities} from "../auth/organization-permissions";
import {legacyClerkMemberships} from "../auth/clerk-provider";
import {clerkTeamView, manageClerkTeam, linkedOrganization, linkClerkTeam} from "../auth/clerk-teams";
import {currentConsolePrincipal, consoleMembership, selectedIdentityDirectory} from "../auth/principal";
import { getBusinessIdentity } from "../auth/business-identity";
import { readCachedView } from "../../cache";
import { AuthMailDeliveryError, withAuthMailDelivery } from "../auth/mail";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { randomBytes } from "node:crypto";
import { hashPassword } from "better-auth/crypto";
import type { PoolClient } from "pg";
import { database, authDatabase } from "../../db/runtime";
import { AccessError, websiteURL } from "./access-management";
import { currentClaims } from "../auth/authorization";
import { businessOperator } from "../auth/business-identity";
import { tenantInvitationScope } from "../auth/tenant-policy";

export const profileFields = [
  ["name", "Customer name", 160],
  ["first_name", "First name", 160],
  ["last_name", "Last name", 160],
  ["company_name", "Company name", 160],
  ["contact_name", "Contact name", 160],
  ["contact_email", "Contact email", 254],
  ["phone", "Phone", 50],
  ["address_line1", "Address line 1", 160],
  ["address_line2", "Address line 2", 160],
  ["postal_code", "Postal code", 32],
  ["city", "City", 160],
  ["region", "Region", 160],
  ["country", "Country code (two letters)", 2],
] as const;
export function validateTenantProfile(input: Record<string, string>) {
  if (!["person", "company"].includes(input.customer_type))
    throw new AccessError("Choose person or company.");
  const profile: Record<string, string | null> = {
    customer_type: input.customer_type,
  };
  for (const [field, label, max] of profileFields) {
    const value = (input[field] || "").trim();
    if (value.length > max || /[\u0000-\u001f]/.test(value))
      throw new AccessError(`${label} is invalid or too long.`);
    profile[field] = value || null;
  }
  if (!profile.name) throw new AccessError("Enter a customer name.");
  if (
    profile.contact_email &&
    !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(profile.contact_email)
  )
    throw new AccessError("Enter a valid contact email.");
  if (profile.country && !/^[a-z]{2}$/i.test(profile.country))
    throw new AccessError("Use a two-letter country code.");
  if (profile.country) profile.country = profile.country.toUpperCase();
  return profile;
}
const columns = [
  "id",
  "status",
  "customer_type",
  ...profileFields.map(([field]) => field),
]
  .map((field) => `c.${field}`)
  .join(",");
const validID = (value: string) => {
  if (!/^[1-9][0-9]{0,18}$/.test(value || ""))
    throw new AccessError("Tenant not found.");
  return value;
};
async function actor(headers: Headers) {
  const session = await getBusinessIdentity(headers);
  const user = session && await selectedIdentityDirectory.session(
    database, { subject: session.user.id, sessionID: session.sessionID },
  );
  if (!session || !user || user.banned || !user.emailVerified || user.mustChangePassword)
    throw new AccessError("Sign in and complete account setup first.");
  const operator = !(await businessOperator(user.subject)).disabled;
  if (user.role !== "user" && !operator)
    throw new AccessError("Complete operator security setup first.");
  return {
    id: user.subject,
    sessionID: session.sessionID,
    email: session.user.email,
    operator,
  };
}
async function tenantFor(
  user: Awaited<ReturnType<typeof actor>>,
  customerID: string,
  connection: Pick<PoolClient, "query"> = database,
) {
  const principal=currentConsolePrincipal();
  if(principal){
    if(principal.userID!==user.id)throw new AccessError("Identity mismatch.");
    const row=(await connection.query(`SELECT ${columns},t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1`,[validID(customerID)])).rows[0];
    const membership=row&&consoleMembership(user.id,row.organization_id);
    if(!row || (!user.operator && (row.status!=="active" || !membership)))throw new AccessError("Tenant not found or access unavailable.");
    return {...row,member_role:membership?.role};
  }
  const proof = await legacyClerkMemberships(user.id);
  if (proof) {
    const row = (await connection.query(
      `SELECT ${columns},t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1`,
      [validID(customerID)],
    )).rows[0];
    const member = row && proof.roles.find(member => member.customerID === customerID && member.organizationID === row.organization_id);
    if (!row || (!user.operator && (row.status !== "active" || !member)))
      throw new AccessError("Tenant not found or access unavailable.");
    return { ...row, member_role: member?.role };
  }
  const row = (
    await connection.query(
      `SELECT ${columns},t.organization_id,m.role AS member_role FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id LEFT JOIN webdock_auth.member m ON m."organizationId"=t.organization_id AND m."userId"=$2 WHERE c.id=$1 AND ($3 OR (c.status='active' AND m.id IS NOT NULL))`,
      [validID(customerID), user.id, user.operator],
    )
  ).rows[0];
  if (!row) throw new AccessError("Tenant not found or access unavailable.");
  return row;
}
export async function listTenants(headers: Headers) {
  return withAuthenticatedDatabase(headers, async () => {
    const user = await actor(headers);
    const principal=currentConsolePrincipal();
    if(principal)return (await database.query(`SELECT c.id,c.name,c.status,$3::text AS role FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE $1 OR (c.id=$2 AND c.status='active') ORDER BY c.name,c.id`,[user.operator,principal.customerID,principal.role])).rows.map(row => ({...row, ...(!user.operator ? {capabilities: organizationCapabilities(principal.permissions)} : {})}));
    const proof = await legacyClerkMemberships(user.id);
    if (proof) {
      const rows = (await database.query(
        `SELECT c.id,c.name,c.status,t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE $1 OR (c.status='active' AND c.id=ANY($2::varchar[])) ORDER BY c.name,c.id`,
        [user.operator, proof.roles.map(member => member.customerID)],
      )).rows;
      return rows.flatMap(row => {
        const member = proof.roles.find(member => member.customerID === row.id && member.organizationID === row.organization_id);
        return user.operator || member ? [{ id: row.id, name: row.name, status: row.status, role: member?.role,
          ...(!user.operator ? { capabilities: organizationCapabilities(member?.permissions) } : {}) }] : [];
      });
    }
    const rows = await readCachedView(
      { kind: "navigation", scope: {subject: user.id, sessionID: user.sessionID, operator: user.operator}, query: "tenant-membership-v2", ttlSeconds: 60 },
      async () => (await database.query("SELECT webdock_cache.version($1::text[]) AS version", [["webdock_admin.customers", "webdock_auth.user", "webdock_auth.member", "webdock_auth.tenant_customer"]])).rows[0].version,
      async () => (await database.query(
        `SELECT c.id,c.name,c.status,m.role,t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id LEFT JOIN webdock_auth.member m ON m."organizationId"=t.organization_id AND m."userId"=$1 WHERE $2 OR (c.status='active' AND m.id IS NOT NULL) ORDER BY c.name,c.id`,
        [user.id, user.operator],
      )).rows,
    );
    return rows.map(({id,name,status,role})=>({id,name,status,role}));
  });
}
export async function listTenantInvitations(headers: Headers) {
  return withAuthenticatedDatabase(headers, async () => {
    const user = await actor(headers);
    return (
      await database.query(
        'SELECT i.id,i.role,o.name FROM webdock_auth.invitation i JOIN webdock_auth.organization o ON o.id=i."organizationId" JOIN webdock_auth.tenant_customer t ON t.organization_id=o.id JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE lower(i.email)=lower($1) AND i.status=\'pending\' AND i."expiresAt">now() AND c.status=\'active\' ORDER BY i."createdAt" DESC',
        [user.email],
      )
    ).rows;
  });
}
export async function getTenant(headers: Headers, customerID: string) {
  return withAuthenticatedDatabase(headers, async () => {
    const user = await actor(headers),
      tenant = await tenantFor(user, customerID);
    const linked=await linkedOrganization(tenant.organization_id);
    const canManage =
      tenant.status === "active" &&
      (user.operator || ["owner", "admin"].includes(tenant.member_role));
    const [members, invitations, bindings] = await Promise.all([
      canManage && !linked
        ? database.query(
            'SELECT m.id,m.role,u.name,u.email,u.banned,u."emailVerified",u."mustChangePassword" FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=$1 AND u.role=\'user\' ORDER BY u.name',
            [tenant.organization_id],
          )
        : Promise.resolve({ rows: [] }),
      canManage && !linked
        ? database.query(
            'SELECT id,email,role,status,"expiresAt" FROM webdock_auth.invitation WHERE "organizationId"=$1 ORDER BY "createdAt" DESC LIMIT 100',
            [tenant.organization_id],
          )
        : Promise.resolve({ rows: [] }),
      database.query(
        'SELECT b.id,b.label,c."redirectUris" FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id WHERE b.organization_id=$1 AND b.enabled AND NOT c.disabled ORDER BY b.label',
        [tenant.organization_id],
      ),
    ]);
    const sites = (
      await Promise.all(
        bindings.rows.map(async (binding) => {
          const url = websiteURL(binding.redirectUris),
            claims = await currentClaims(user.id, binding.id);
          return url && !claims.disabled
            ? {
                id: binding.id,
                name: binding.label,
                url,
                role: claims.webdock_role,
              }
            : null;
        }),
      )
    ).filter((site) => site !== null);
    const team=linked && canManage ? await clerkTeamView(tenant.organization_id) : null;
    return {
      tenant,
      ...((currentConsolePrincipal()||linked)?{team:{provider:"clerk" as const,ready:!!linked?.enabled,canManage:!!currentConsolePrincipal()&&canManage}}:{}),
      canManage,
      operator: user.operator,
      members: team?.members ?? members.rows,
      invitations: team?.invitations ?? invitations.rows,
      sites,
    };
  });
}
async function provisionInvitee(email: string, name: string) {
  const existing = (
    await authDatabase.query(
      'SELECT id,role,banned FROM webdock_auth."user" WHERE lower(email)=$1',
      [email],
    )
  ).rows[0];
  if (existing) {
    if (existing.role !== "user" || existing.banned)
      throw new AccessError(
        "Operator or suspended accounts cannot receive customer invitations.",
      );
    return existing.id as string;
  }
  const password = await hashPassword(randomBytes(40).toString("base64url"));
  const connection = await authDatabase.connect();
  try {
    await connection.query("BEGIN");
    const user = (
      await connection.query(
        'INSERT INTO webdock_auth."user"(name,email,role,"emailVerified","mustChangePassword") VALUES($1,$2,\'user\',false,true) RETURNING id',
        [name, email],
      )
    ).rows[0];
    await connection.query(
      'INSERT INTO webdock_auth.account("accountId","providerId","userId",password,"updatedAt") VALUES($1,\'credential\',$1,$2,now())',
      [user.id, password],
    );
    await connection.query("COMMIT");
    return user.id as string;
  } catch (error) {
    await connection.query("ROLLBACK");
    throw error;
  } finally {
    connection.release();
  }
}
export async function manageTenant(
  headers: Headers,
  customerID: string,
  input: Record<string, string>,
) {
  return withAuthenticatedDatabase(headers, async () => {
    const user = await actor(headers);
    const tenant = await tenantFor(user, customerID);
    if(input.action==="link-team")return linkClerkTeam(customerID,tenant.organization_id);
    if(input.action!=="profile" && (currentConsolePrincipal()||await linkedOrganization(tenant.organization_id)))return manageClerkTeam(customerID,tenant.organization_id,input);
    if (
      tenant.status !== "active" ||
      (!user.operator && !["admin", "owner"].includes(tenant.member_role))
    )
      throw new AccessError("Tenant administrator access is required.");
    if (
      !["profile", "invite", "resend", "cancel", "role", "remove"].includes(
        input.action,
      )
    )
      throw new AccessError("Unknown tenant action.");
    const event = (
      await database.query(
        "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,$3,'started') RETURNING id",
        [user.id, `tenant-${input.action}`, customerID],
      )
    ).rows[0].id;
    try {
      let email = "",
        invitee = "",
        invitationRole = input.role;
      if (input.action === "invite") {
        email = (input.email || "").trim().toLowerCase();
        const name = (input.name || "").trim();
        if (
          !email ||
          email.length > 254 ||
          !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email) ||
          !name ||
          name.length > 160
        )
          throw new AccessError("Enter a name and valid email address.");
        if (!["admin", "member"].includes(invitationRole))
          throw new AccessError("Choose administrator or member.");
        invitee = await provisionInvitee(email, name);
      }
      const connection = await database.connect();
      try {
        await connection.query("BEGIN");
        // Serialize membership removals and demotions to protect the final customer admin.
        await connection.query(
          "SELECT id FROM webdock_auth.organization WHERE id=$1 FOR UPDATE",
          [tenant.organization_id],
        );
        const current = await tenantFor(user, customerID, connection);
        if (
          current.status !== "active" ||
          (!user.operator && !["admin", "owner"].includes(current.member_role))
        )
          throw new AccessError("Tenant administrator access is required.");
        const currentUser = (
          await connection.query(
            'SELECT role,banned,"emailVerified","mustChangePassword","twoFactorEnabled" FROM webdock_auth."user" WHERE id=$1',
            [user.id],
          )
        ).rows[0];
        if (
          !currentUser ||
          currentUser.banned ||
          (!currentConsolePrincipal() && (!currentUser.emailVerified || currentUser.mustChangePassword)) ||
          (user.operator
            ? currentUser.role !== "operator" || (!currentConsolePrincipal() && !currentUser.twoFactorEnabled)
            : currentUser.role !== "user")
        )
          throw new AccessError("Sign in and complete account setup first.");
        if (input.action === "profile") {
          const profile = validateTenantProfile(input),
            keys = Object.keys(profile);
          await connection.query(
            `UPDATE webdock_admin.customers SET ${keys.map((key, i) => `${key}=$${i + 2}`).join(",")},updated_at=now() WHERE id=$1`,
            [customerID, ...keys.map((key) => profile[key])],
          );
        } else if (["role", "remove"].includes(input.action)) {
          const target = (
            await connection.query(
              'SELECT m.id,m.role,m."userId" FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m.id=$1 AND m."organizationId"=$2 AND u.role=\'user\' FOR UPDATE OF m',
              [validID(input.member), tenant.organization_id],
            )
          ).rows[0];
          if (!target) throw new AccessError("Customer membership not found.");
          if (
            input.action === "role" &&
            !["owner", "admin", "member"].includes(input.role)
          )
            throw new AccessError("Choose owner, administrator or member.");
          if (
            !user.operator &&
            current.member_role !== "owner" &&
            (target.role !== "member" ||
              (input.action === "role" && input.role !== "member"))
          )
            throw new AccessError(
              "Tenant owner access is required to manage administrators and owners.",
            );
          if (input.action === "role" && input.role === "owner") {
            const eligible = (
              await connection.query(
                `SELECT 1 FROM webdock_auth."user" WHERE id=$1 AND role='user' AND NOT coalesce(banned,false) AND "emailVerified" AND NOT "mustChangePassword" FOR UPDATE`,
                [target.userId],
              )
            ).rowCount;
            if (!eligible)
              throw new AccessError(
                "A tenant owner must be active, verified and have completed account setup.",
              );
          }
          if (
            target.role === "owner" &&
            (input.action === "remove" || input.role !== "owner")
          ) {
            const owners = (
              await connection.query(
                `SELECT 1 FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=$1 AND m.id<>$2 AND m.role='owner' AND u.role='user' AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword" LIMIT 1`,
                [tenant.organization_id, target.id],
              )
            ).rowCount;
            if (!owners)
              throw new AccessError(
                "Keep at least one active verified customer owner.",
              );
          }
          if (
            ["owner", "admin"].includes(target.role) &&
            (input.action === "remove" || input.role === "member")
          ) {
            const remaining = (
              await connection.query(
                'SELECT count(*)::int AS count FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=$1 AND m.id<>$2 AND m.role IN (\'owner\',\'admin\') AND u.role=\'user\' AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword"',
                [tenant.organization_id, target.id],
              )
            ).rows[0].count;
            if (!remaining)
              throw new AccessError(
                "Keep at least one customer administrator.",
              );
          }
          if (input.action === "role")
            await connection.query(
              "UPDATE webdock_auth.member SET role=$2 WHERE id=$1",
              [target.id, input.role],
            );
          else {
            await connection.query(
              "DELETE FROM webdock_auth.member WHERE id=$1",
              [target.id],
            );
            await connection.query(
              "UPDATE webdock_auth.project_grant SET enabled=false WHERE user_id=$1 AND organization_id=$2",
              [target.userId, tenant.organization_id],
            );
          }
        } else {
          if (["resend", "cancel"].includes(input.action)) {
            const invite = (
              await connection.query(
                "SELECT id,email,role FROM webdock_auth.invitation WHERE id=$1 AND \"organizationId\"=$2 AND status='pending' FOR UPDATE",
                [validID(input.invitation), tenant.organization_id],
              )
            ).rows[0];
            if (!invite) throw new AccessError("Pending invitation not found.");
            email = invite.email;
            invitationRole = invite.role;
            if (input.action === "cancel") {
              await connection.query(
                "UPDATE webdock_auth.invitation SET status='canceled' WHERE id=$1",
                [invite.id],
              );
              await connection.query(
                'UPDATE webdock_auth.project_grant g SET enabled=false FROM webdock_auth."user" u WHERE g.user_id=u.id AND lower(u.email)=lower($1) AND g.organization_id=$2',
                [email, tenant.organization_id],
              );
            }
          }
          if (input.action !== "cancel") {
            if (
              input.action === "invite" &&
              (
                await connection.query(
                  "SELECT id FROM webdock_auth.invitation WHERE \"organizationId\"=$1 AND lower(email)=lower($2) AND status='pending'",
                  [tenant.organization_id, email],
                )
              ).rowCount
            )
              throw new AccessError(
                "A pending invitation already exists. Resend it, or cancel it before choosing another role.",
              );
            if (
              invitee &&
              (
                await connection.query(
                  'SELECT id FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2',
                  [tenant.organization_id, invitee],
                )
              ).rowCount
            )
              throw new AccessError(
                "This person is already a member. Contact an operator to change their role.",
              );
            if (user.operator)
              await connection.query(
                'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") SELECT $1,$2,\'owner\',now() WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2)',
                [tenant.organization_id, user.id],
              );
          }
        }
        await connection.query("COMMIT");
      } catch (error) {
        await connection.query("ROLLBACK");
        throw error;
      } finally {
        connection.release();
      }
      if (["invite", "resend"].includes(input.action))
        await withAuthMailDelivery(() =>
          tenantInvitationScope.run(tenant.organization_id, async () =>
            (await import("../auth/auth")).auth.api.createInvitation({
              headers,
              body: {
                email,
                role: invitationRole as "admin" | "member",
                organizationId: tenant.organization_id,
                resend: input.action === "resend",
              },
            }),
          ),
        );
      await database.query(
        "UPDATE webdock_auth.access_event SET outcome='succeeded' WHERE id=$1",
        [event],
      );
      return {
        message: ["invite", "resend"].includes(input.action)
          ? "Invitation email sent. Access starts after acceptance."
          : "Change saved.",
      };
    } catch (error) {
      await database.query(
        "UPDATE webdock_auth.access_event SET outcome='failed' WHERE id=$1",
        [event],
      );
      if (error instanceof AuthMailDeliveryError)
        throw new AccessError(error.message);
      throw error;
    }
  });
}
