import { requireOAuthCapability } from "@webdock/auth-configuration/protocols";
import { getBusinessIdentity } from "../auth/business-identity";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { searchPage } from "@webdock/search";
import { randomBytes } from "node:crypto";
import { database } from "../../db/runtime";
import { businessOperator } from "../auth/business-identity";
import { currentClaims } from "../auth/authorization";
import {
  sendAuthMail,
  withAuthMailDelivery,
  AuthMailDeliveryError,
} from "../auth/mail";

export const accessSchemaSQL = `CREATE TABLE IF NOT EXISTS webdock_auth.access_event (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), actor_id text NOT NULL,
 action text NOT NULL, target_id text, outcome text NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
)`;
export class AccessError extends Error {}
const id = (value: string | undefined) => {
  if (!value || !/^[1-9][0-9]{0,18}$/.test(value))
    throw new AccessError("Choose a valid record.");
  return value;
};
const role = (value: string | undefined) => {
  if (!["reader", "editor", "admin"].includes(value || ""))
    throw new AccessError("Choose reader, editor or administrator.");
  return value!;
};
const name = (value: string | undefined) => {
  const result = value?.trim();
  if (!result || result.length > 160)
    throw new AccessError("Enter a name of up to 160 characters.");
  return result;
};
export async function requireAccessOperator(headers: Headers) {
  return withAuthenticatedDatabase(headers, async () => {
    const session = await getBusinessIdentity(headers);
    if (!session || (await businessOperator(session.user.id)).disabled)
      throw new AccessError(
        "Operator access with completed security setup is required.",
      );
    return session;
  });
}
export function websiteURL(redirectUris: unknown) {
  if (!Array.isArray(redirectUris)) return null;
  if (
    redirectUris.some((uri) => {
      try {
        return ["studio.webdock.dev", "admin.webdock.dev"].includes(
          new URL(uri).hostname,
        );
      } catch {
        return false;
      }
    })
  )
    return null;
  for (const uri of redirectUris) {
    try {
      const url = new URL(uri);
      if (["studio.webdock.dev", "admin.webdock.dev"].includes(url.hostname))
        return null;
      if (
        url.username ||
        url.password ||
        (url.protocol !== "https:" &&
          !(
            process.env.NODE_ENV !== "production" &&
            url.protocol === "http:" &&
            ["localhost", "127.0.0.1"].includes(url.hostname)
          ))
      )
        continue;
      if (url.pathname === "/api/sso/callback")
        return new URL("/cms", url).href;
    } catch {}
  }
  return null;
}
async function website(binding: string) {
  const row = (
    await database.query(
      'SELECT b.*,c."redirectUris" FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id WHERE b.id=$1 AND b.enabled AND NOT c.disabled',
      [id(binding)],
    )
  ).rows[0];
  if (!row || !websiteURL(row.redirectUris))
    throw new AccessError(
      "Choose an enabled customer website, not the operator studio.",
    );
  return row;
}
export async function listAccess(headers: Headers, search: string) {
  return withAuthenticatedDatabase(headers, async () => {
    await requireAccessOperator(headers);
    const term = search.trim().slice(0, 160);
    const query = `%${term}%`;
    const searched = async (
      sql: string,
      text: (row: { name: string; email: string }) => string,
      limit: number,
    ) => ({
      rows: (
        await searchPage(
          async (page) => {
            const rows = (
              await database.query(sql + " LIMIT 300 OFFSET $1", [
                (page - 1) * 300,
              ])
            ).rows;
            return { rows, hasMore: rows.length === 300 };
          },
          text,
          term,
          1,
          limit,
        )
      ).docs,
    });
    const [users, organizations, bindings, grants, invitations, events] =
      await Promise.all([
        term
          ? searched(
              'SELECT id,name,email,role,banned,"emailVerified","mustChangePassword" FROM webdock_auth."user" ORDER BY "createdAt" DESC,id',
              (r) => r.name + " " + r.email,
              100,
            )
          : database.query(
              'SELECT id,name,email,role,banned,"emailVerified","mustChangePassword" FROM webdock_auth."user" WHERE name ILIKE $1 OR email ILIKE $1 ORDER BY "createdAt" DESC LIMIT 100',
              [query],
            ),
        database.query(
          "SELECT id,name FROM webdock_auth.organization ORDER BY name LIMIT 200",
        ),
        database.query(
          'SELECT b.id,b.label,b.organization_id,c."redirectUris" FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id WHERE b.enabled AND NOT c.disabled ORDER BY b.label LIMIT 200',
        ),
        term
          ? searched(
              'SELECT g.id,g.role,g.enabled,u.name,u.email,b.label FROM webdock_auth.project_grant g JOIN webdock_auth."user" u ON u.id=g.user_id JOIN webdock_auth.app_binding b ON b.id=g.binding_id ORDER BY u.email,b.label,g.id',
              (r) => r.name + " " + r.email,
              200,
            )
          : database.query(
              'SELECT g.id,g.role,g.enabled,u.name,u.email,b.label FROM webdock_auth.project_grant g JOIN webdock_auth."user" u ON u.id=g.user_id JOIN webdock_auth.app_binding b ON b.id=g.binding_id WHERE u.name ILIKE $1 OR u.email ILIKE $1 ORDER BY u.email,b.label LIMIT 200',
              [query],
            ),
        term
          ? searched(
              'SELECT i.id,i.email,i.status,i."expiresAt",o.name FROM webdock_auth.invitation i JOIN webdock_auth.organization o ON o.id=i."organizationId" ORDER BY i."createdAt" DESC,i.id',
              (r) => r.email,
              100,
            )
          : database.query(
              'SELECT i.id,i.email,i.status,i."expiresAt",o.name FROM webdock_auth.invitation i JOIN webdock_auth.organization o ON o.id=i."organizationId" WHERE i.email ILIKE $1 ORDER BY i."createdAt" DESC LIMIT 100',
              [query],
            ),
        database.query(
          'SELECT e.*,u.email AS actor FROM webdock_auth.access_event e LEFT JOIN webdock_auth."user" u ON u.id=e.actor_id ORDER BY e.created_at DESC LIMIT 20',
        ),
      ]);
    return {
      users: users.rows,
      organizations: organizations.rows,
      websites: bindings.rows.filter((row) => websiteURL(row.redirectUris)),
      grants: grants.rows,
      invitations: invitations.rows,
      events: events.rows,
    };
  });
}
export async function accountSites(headers: Headers) {
  return withAuthenticatedDatabase(headers, async () => {
    const session = await getBusinessIdentity(headers);
    if (!session) throw new AccessError("Sign in to view your websites.");
    const rows = (
      await database.query(
        'SELECT b.id,b.label,c."redirectUris",t.customer_id,customer.name AS customer_name FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id LEFT JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id LEFT JOIN webdock_admin.customers customer ON customer.id=t.customer_id WHERE b.enabled AND NOT c.disabled ORDER BY b.label LIMIT 200',
      )
    ).rows;
    const sites = await Promise.all(
      rows.map(async (row) => {
        const url = websiteURL(row.redirectUris);
        const claims = url
          ? await currentClaims(session.user.id, row.id)
          : { disabled: true };
        return !claims.disabled && url
          ? {
              id: row.id,
              name: row.label,
              url,
              role: claims.webdock_role,
              ...(row.customer_id
                ? {
                    customerID: row.customer_id as string,
                    customerName: row.customer_name as string,
                  }
                : {}),
            }
          : null;
      }),
    );
    return sites.filter((site) => site !== null);
  });
}
export async function manageAccess(
  headers: Headers,
  input: Record<string, string>,
): Promise<{ id?: string; message: string }> {
  return withAuthenticatedDatabase(headers, async () => {
    const session = await requireAccessOperator(headers);
    const actions = [
      "create-organization",
      "link-website",
      "invite",
      "change-access",
      "revoke-access",
      "suspend",
      "cancel-invite",
    ];
    if (!actions.includes(input.action))
      throw new AccessError("Unknown action.");
    const event = (
      await database.query(
        "INSERT INTO webdock_auth.access_event(actor_id,action,outcome) VALUES($1,$2,$3) RETURNING id",
        [session.user.id, input.action, "started"],
      )
    ).rows[0].id;
    let target: string | undefined;
    try {
      requireOAuthCapability();
      const { auth } = await import("../auth/auth");
      if (input.action === "create-organization") {
        const org = await auth.api.createOrganization({
          headers,
          body: {
            name: name(input.name),
            slug: "customer-" + randomBytes(10).toString("hex"),
            keepCurrentActiveOrganization: true,
          },
        });
        target = org!.id;
      } else if (input.action === "link-website") {
        const site = await website(input.binding);
        const org = id(input.organization);
        if (site.organization_id && site.organization_id !== org)
          throw new AccessError(
            "This website already belongs to a customer. Ownership transfers require a separate review.",
          );
        const result = await database.query(
          "UPDATE webdock_auth.app_binding SET organization_id=$2 WHERE id=$1 AND (organization_id IS NULL OR organization_id=$2) AND EXISTS(SELECT 1 FROM webdock_auth.organization WHERE id=$2) RETURNING id",
          [site.id, org],
        );
        if (!result.rowCount)
          throw new AccessError(
            "Customer assignment changed. Refresh and try again.",
          );
        target = site.id;
      } else if (input.action === "invite") {
        const site = await website(input.binding),
          accessRole = role(input.role),
          displayName = name(input.name);
        if (!site.organization_id)
          throw new AccessError("Assign this website to a customer first.");
        const email = input.email?.trim().toLowerCase();
        if (
          !email ||
          email.length > 254 ||
          !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)
        )
          throw new AccessError("Enter a valid email address.");
        let user = (
          await database.query(
            'SELECT id,role,banned FROM webdock_auth."user" WHERE lower(email)=$1',
            [email],
          )
        ).rows[0];
        if (user && (user.role !== "user" || user.banned))
          throw new AccessError(
            "Operator or suspended accounts cannot receive customer invitations.",
          );
        if (!user)
          user = (
            await auth.api.createUser({
              headers,
              body: {
                email,
                name: displayName,
                password: randomBytes(40).toString("base64url"),
                role: "user",
                data: { mustChangePassword: true, emailVerified: false },
              },
            })
          ).user;
        target = user.id;
        // Customer membership remains pending until the emailed invitation is accepted.
        await database.query(
          "INSERT INTO webdock_auth.project_grant(user_id,binding_id,organization_id,role,enabled) VALUES($1,$2,$3,$4,true) ON CONFLICT(user_id,binding_id) DO UPDATE SET role=$4,enabled=true,organization_id=$3",
          [user.id, site.id, site.organization_id, accessRole],
        );
        // Platform operators may administer every customer; customer users never enter this path.
        await database.query(
          'INSERT INTO webdock_auth.member("userId","organizationId",role,"createdAt") SELECT $1,$2,\'owner\',now() WHERE NOT EXISTS(SELECT 1 FROM webdock_auth.member WHERE "userId"=$1 AND "organizationId"=$2)',
          [session.user.id, site.organization_id],
        );
        const member = (
          await database.query(
            'SELECT id FROM webdock_auth.member WHERE "userId"=$1 AND "organizationId"=$2',
            [user.id, site.organization_id],
          )
        ).rowCount;
        if (member) {
          await sendAuthMail({
            to: email,
            subject: "Your Webdock website access",
            text: `Your administrator has granted ${accessRole} access to ${site.label}.\n${process.env.BETTER_AUTH_URL || "https://auth.webdock.dev"}/sites`,
          });
          target = user.id;
        } else {
          const invitation = await withAuthMailDelivery(() =>
            auth.api.createInvitation({
              headers,
              body: {
                email,
                role: "member",
                organizationId: site.organization_id,
                resend: true,
              },
            }),
          );
          target = invitation.id;
        }
      } else if (
        input.action === "change-access" ||
        input.action === "revoke-access"
      ) {
        target = id(input.grant);
        const changed =
          input.action === "change-access"
            ? await database.query(
                "UPDATE webdock_auth.project_grant g SET role=$2,enabled=true FROM webdock_auth.\"user\" u WHERE g.id=$1 AND u.id=g.user_id AND u.role='user' RETURNING g.id",
                [target, role(input.role)],
              )
            : await database.query(
                "UPDATE webdock_auth.project_grant g SET enabled=false FROM webdock_auth.\"user\" u WHERE g.id=$1 AND u.id=g.user_id AND u.role='user' RETURNING g.id",
                [target],
              );
        if (!changed.rowCount)
          throw new AccessError("Customer access record not found.");
      } else if (input.action === "suspend") {
        target = id(input.user);
        if (!["true", "false"].includes(input.blocked))
          throw new AccessError("Invalid account status.");
        const targetUser = (
          await database.query(
            'SELECT role FROM webdock_auth."user" WHERE id=$1',
            [target],
          )
        ).rows[0];
        if (targetUser?.role !== "user")
          throw new AccessError("Platform operator accounts are protected.");
        if (input.blocked === "true")
          await auth.api.banUser({
            headers,
            body: {
              userId: target,
              banReason: "Suspended by Webdock operator",
            },
          });
        else await auth.api.unbanUser({ headers, body: { userId: target } });
      } else {
        target = id(input.id);
        // Platform authorization is independent of the inviter's organization role.
        // Withdraw pending grants atomically, so a later invitation cannot revive them.
        const connection = await database.connect();
        try {
          await connection.query("BEGIN");
          const cancelled = (
            await connection.query(
              "UPDATE webdock_auth.invitation SET status='canceled' WHERE id=$1 AND status='pending' RETURNING email,\"organizationId\"",
              [target],
            )
          ).rows[0];
          if (!cancelled)
            throw new AccessError(
              "This invitation has already been answered or cancelled. Refresh the page.",
            );
          await connection.query(
            'UPDATE webdock_auth.project_grant g SET enabled=false FROM webdock_auth."user" u WHERE u.id=g.user_id AND lower(u.email)=lower($1) AND g.organization_id=$2',
            [cancelled.email, cancelled.organizationId],
          );
          await connection.query("COMMIT");
        } catch (error) {
          await connection.query("ROLLBACK");
          throw error;
        } finally {
          connection.release();
        }
      }
      await database.query(
        "UPDATE webdock_auth.access_event SET target_id=$2,outcome='succeeded' WHERE id=$1",
        [event, target],
      );
      return {
        id: target,
        message:
          input.action === "invite"
            ? "Access saved and invitation email sent."
            : "Change saved.",
      };
    } catch (error) {
      await database.query(
        "UPDATE webdock_auth.access_event SET target_id=$2,outcome='failed' WHERE id=$1",
        [event, target],
      );
      if (error instanceof AuthMailDeliveryError)
        throw new AccessError(error.message);
      throw error;
    }
  });
}
