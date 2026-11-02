import {linkedMembershipRole, organizationPermission} from "../auth/clerk-provider";
import {businessOperator} from "../auth/business-identity";
import {currentConsolePrincipal, consoleMembership, selectedIdentityDirectory} from "../auth/principal";
import { getBusinessIdentity } from "../auth/business-identity";
import { withAuthenticatedDatabase } from "../../db/database-context";
import { readCachedView } from "../../cache";
import {
  effectiveHostingAllowances,
  parseHostingFields,
  type HostingAllowances,
} from "@webdock/hosting-contracts";
import { createHash, randomBytes } from "node:crypto";
import type { PoolClient } from "pg";
import { database } from "../../db/runtime";
import { studioURL } from "../shared/studio-links";

export class PlanError extends Error {}
export const allowanceKeys = [
  "storageBytes",
  "mailMessages",
  "transferBytes",
  "websites",
  "editors",
] as const;
export type Allowances = Record<
  (typeof allowanceKeys)[number],
  number | null
> & { hosting?: HostingAllowances };
export type Plan = {
  id: string;
  name: string;
  description: string;
  allowances: Allowances;
  createdAt: string;
};
const zero: Allowances = {
  storageBytes: 0,
  mailMessages: 0,
  transferBytes: 0,
  websites: 0,
  editors: 0,
};
export const plansSchemaSQL = `
CREATE TABLE IF NOT EXISTS webdock_auth.plan_template (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(), name text NOT NULL, description text NOT NULL,
 allowances jsonb NOT NULL, created_by text NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_auth.tenant_subscription (
 customer_id varchar PRIMARY KEY REFERENCES webdock_auth.tenant_customer(customer_id) ON DELETE RESTRICT,
 plan_id text REFERENCES webdock_auth.plan_template(id), name text NOT NULL, description text NOT NULL,
 base jsonb NOT NULL, extras jsonb NOT NULL, revision integer NOT NULL CHECK(revision>0),
 updated_by text NOT NULL, updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_auth.subscription_history (
 customer_id varchar NOT NULL REFERENCES webdock_auth.tenant_customer(customer_id) ON DELETE RESTRICT,
 revision integer NOT NULL CHECK(revision>0), name text NOT NULL, description text NOT NULL,
 base jsonb NOT NULL, extras jsonb NOT NULL, plan_id text REFERENCES webdock_auth.plan_template(id),
 actor_id text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY(customer_id,revision)
);
CREATE TABLE IF NOT EXISTS webdock_auth.plan_offer (
 id text PRIMARY KEY DEFAULT webdock_auth.next_snowflake(),
 customer_id varchar NOT NULL REFERENCES webdock_auth.tenant_customer(customer_id) ON DELETE RESTRICT,
 plan_id text REFERENCES webdock_auth.plan_template(id), token_hash text NOT NULL UNIQUE,
 name text NOT NULL, description text NOT NULL, terms text NOT NULL, base jsonb NOT NULL, offered_extras jsonb NOT NULL,
 expected_revision integer NOT NULL CHECK(expected_revision>=0),
 status text NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted','revoked')),
 expires_at timestamptz NOT NULL, accepted_at timestamptz, accepted_by text,
 created_by text NOT NULL, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS plan_offer_customer ON webdock_auth.plan_offer(customer_id,created_at DESC);
`;
type Connection = Pick<PoolClient, "query">;
const id = (value: string) => {
  if (!/^[1-9][0-9]{0,18}$/.test(value || ""))
    throw new PlanError("Choose a valid record.");
  return value;
};
function textField(value: string | undefined, max: number, required = false) {
  const result = (value || "").trim();
  if (
    (required && !result) ||
    result.length > max ||
    /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(result)
  )
    throw new PlanError("Enter valid offer or plan details.");
  return result;
}
function parseAllowances(
  input: Record<string, string>,
  extras = false,
  previousHosting?: HostingAllowances,
): Allowances {
  const fields = [
    ["storageBytes", "storageMB", 1_000_000],
    ["mailMessages", "mailMessages", 1],
    ["transferBytes", "transferGB", 1_000_000_000],
    ["websites", "websites", 1],
    ["editors", "editors", 1],
  ] as const;
  const result = { ...zero };
  for (const [key, field, multiplier] of fields) {
    const value = (input[field] || "").trim();
    if (!value) {
      result[key] = extras ? 0 : null;
      continue;
    }
    // Decimal MB/GB can represent fractional units, but stored bytes must be exact integers.
    if (value.length > 40 || !/^\d+(?:\.\d+)?$/.test(value))
      throw new PlanError(
        "Allowances must be nonnegative numbers within the supported range.",
      );
    const [whole, decimal = ""] = value.split("."),
      fraction = decimal.replace(/0+$/, "");
    const places = String(multiplier).length - 1;
    if (fraction.length > places)
      throw new PlanError("Allowances must use whole bytes or whole counts.");
    const converted =
      BigInt(whole) * BigInt(multiplier) +
      BigInt(fraction.padEnd(places, "0") || "0");
    if (converted > BigInt(Number.MAX_SAFE_INTEGER))
      throw new PlanError("Allowances exceed the supported range.");
    result[key] = Number(converted);
  }
  try {
    result.hosting = parseHostingFields(input, previousHosting, extras);
  } catch {
    throw new PlanError("Enter valid hosting allowances.");
  }
  return result;
}
function effective(base: Allowances, extras: Allowances): Allowances {
  const result = { ...zero };
  for (const key of allowanceKeys) {
    result[key] = base[key] === null ? null : base[key] + (extras[key] || 0);
    if (result[key] !== null && !Number.isSafeInteger(result[key]))
      throw new PlanError("Combined allowance exceeds the supported range.");
  }
  try {
    result.hosting = effectiveHostingAllowances(base.hosting, extras.hosting);
  } catch {
    throw new PlanError(
      "Combined hosting allowance exceeds the supported range.",
    );
  }
  return result;
}
async function actor(
  headers: Headers,
  connection: Connection = database,
  lock = false,
  subject?: string,
) {
  const subjectID =
    subject || (await getBusinessIdentity(headers))?.user.id;
  const user = subjectID && await selectedIdentityDirectory.subject(connection, subjectID, { lock });
  if (!user || user.banned || !user.emailVerified || user.mustChangePassword)
    throw new PlanError("Sign in and complete account setup first.");
  const operator = !(await businessOperator(user.subject)).disabled;
  if (user.role !== "user" && !operator)
    throw new PlanError("Complete operator security setup first.");
  return { id: user.subject, operator };
}
async function tenant(
  user: Awaited<ReturnType<typeof actor>>,
  customerID: string,
  connection: Connection = database,
  lock = false,
  allowArchived = false,
) {
  if (lock)
    await connection.query(
      "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
      [`plan:${id(customerID)}`],
    );
  const row = (
    await connection.query(
      `SELECT c.id,c.name,c.status,t.organization_id FROM webdock_auth.tenant_customer t JOIN webdock_admin.customers c ON c.id=t.customer_id WHERE t.customer_id=$1${lock ? " FOR SHARE OF c" : ""}`,
      [id(customerID)],
    )
  ).rows[0];
  if (!row || (row.status !== "active" && !(allowArchived && user.operator)))
    throw new PlanError("Active tenant not found or access unavailable.");
  const linked=currentConsolePrincipal()?undefined:await linkedMembershipRole(user.id,row.organization_id);
  const membership = consoleMembership(user.id,row.organization_id) ?? (currentConsolePrincipal() ? null : linked!==undefined ? (linked?{role:linked}:null) : (
    await connection.query(
      `SELECT role FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2${lock ? " FOR SHARE" : ""}`,
      [row.organization_id, user.id],
    )
  ).rows[0]);
  if (!user.operator && !membership)
    throw new PlanError("Tenant not found or access unavailable.");
  return {
    ...row,
    admin: !user.operator && ((await organizationPermission(user.id, row.organization_id, "org:billing:manage")) ?? ["admin", "owner"].includes(membership?.role)),
  };
}
function requireOperator(user: Awaited<ReturnType<typeof actor>>) {
  if (!user.operator)
    throw new PlanError("Platform operator access is required.");
}
async function subscription(
  customerID: string,
  connection: Connection = database,
) {
  return (
    await connection.query(
      "SELECT * FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0];
}
function checkRevision(value: string, current: number) {
  if (!/^(0|[1-9]\d*)$/.test(value || "") || Number(value) !== current)
    throw new PlanError("The subscription changed. Refresh and try again.");
}
async function saveSubscription(
  connection: Connection,
  customerID: string,
  plan: {
    id?: string | null;
    name: string;
    description: string;
    base: Allowances;
  },
  extras: Allowances,
  revision: number,
  actorID: string,
) {
  effective(plan.base, extras);
  await connection.query(
    `INSERT INTO webdock_auth.tenant_subscription(customer_id,plan_id,name,description,base,extras,revision,updated_by)
    VALUES($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT(customer_id) DO UPDATE SET plan_id=$2,name=$3,description=$4,base=$5,extras=$6,revision=$7,updated_by=$8,updated_at=now()`,
    [
      customerID,
      plan.id || null,
      plan.name,
      plan.description,
      JSON.stringify(plan.base),
      JSON.stringify(extras),
      revision + 1,
      actorID,
    ],
  );
  await connection.query(
    `INSERT INTO webdock_auth.subscription_history(customer_id,revision,name,description,base,extras,plan_id,actor_id)
    SELECT customer_id,revision,name,description,base,extras,plan_id,updated_by FROM webdock_auth.tenant_subscription WHERE customer_id=$1`,
    [customerID],
  );
}
async function audit(
  connection: Connection,
  actorID: string,
  action: string,
  target: string,
) {
  await connection.query(
    "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,$2,$3,'succeeded')",
    [actorID, `plans-${action}`, target],
  );
}
function offerStatus(
  row: { status: string; expires_at: Date; expected_revision: number },
  revision: number,
) {
  return row.status !== "pending"
    ? row.status
    : row.expires_at.getTime() <= Date.now()
      ? "expired"
      : row.expected_revision !== revision
        ? "stale"
        : "pending";
}
export async function getPlans(headers: Headers): Promise<Plan[]> {
  return withAuthenticatedDatabase(headers, async () => {
    const user = await actor(headers);
    requireOperator(user);
    return readCachedView(
      { kind: "catalog", scope: { subject: user.id, operator: true }, query: "plan-templates", ttlSeconds: 60 },
      async () => (await database.query("SELECT webdock_cache.version($1::text[]) AS version", [["webdock_auth.plan_template"]])).rows[0].version,
      async () => (
        await database.query(
          'SELECT id,name,description,allowances,created_at AS "createdAt" FROM webdock_auth.plan_template ORDER BY created_at DESC,id DESC',
        )
      ).rows.map((row) => ({ ...row, createdAt: row.createdAt.toISOString() })),
    );
  });
}
export async function getTenantPlan(headers: Headers, customerID: string) {
  return withAuthenticatedDatabase(headers, async () => {
    await tenant(await actor(headers), customerID, database, false, true);
    const row = await subscription(customerID),
      revision: number = row?.revision || 0;
    const offers = (
      await database.query(
        "SELECT id,name,status,expires_at,accepted_at,expected_revision FROM webdock_auth.plan_offer WHERE customer_id=$1 ORDER BY created_at DESC LIMIT 100",
        [customerID],
      )
    ).rows;
    return {
      subscription: row
        ? {
            name: row.name as string,
            description: row.description as string,
            base: row.base as Allowances,
            extras: row.extras as Allowances,
            effective: effective(row.base, row.extras),
            revision,
          }
        : null,
      revision,
      offers: offers.map((offer) => ({
        id: offer.id as string,
        name: offer.name as string,
        status: offerStatus(offer, revision),
        expiresAt: offer.expires_at.toISOString() as string,
        acceptedAt: offer.accepted_at?.toISOString() as string | undefined,
      })),
    };
  });
}
export async function managePlans(
  headers: Headers,
  input: Record<string, string>,
): Promise<{ message: string; offerURL?: string }> {
  return withAuthenticatedDatabase(headers, async () => {
    const initialUser = await actor(headers);
    requireOperator(initialUser);
    if (
      ![
        "create-plan",
        "assign",
        "assign-custom",
        "extras",
        "create-offer",
        "revoke-offer",
      ].includes(input.action)
    )
      throw new PlanError("Unknown plan action.");
    const connection = await database.connect();
    try {
      await connection.query("BEGIN");
      const user = await actor(headers, connection, true, initialUser.id);
      requireOperator(user);
      let target: string, offerURL: string | undefined;
      if (input.action === "create-plan") {
        const row = (
          await connection.query(
            "INSERT INTO webdock_auth.plan_template(name,description,allowances,created_by) VALUES($1,$2,$3,$4) RETURNING id",
            [
              textField(input.name, 160, true),
              textField(input.description, 4000),
              JSON.stringify(parseAllowances(input)),
              user.id,
            ],
          )
        ).rows[0];
        target = row.id;
      } else {
        const customerID = id(input.customerID);
        target = customerID;
        await tenant(user, customerID, connection, true);
        if (
          process.env.DATABASE_RLS_SECRET &&
          ["assign", "assign-custom", "create-offer"].includes(input.action) &&
          (
            await connection.query(
              "SELECT webdock_rls.billing_base_locked($1) AS locked",
              [customerID],
            )
          ).rows[0].locked
        )
          throw new PlanError(
            "This allocation is locked by a retained billing agreement. Operator data review is required before changing its ownership.",
          );
        const current = await subscription(customerID, connection),
          revision: number = current?.revision || 0;
        if (input.action === "revoke-offer") {
          const changed = await connection.query(
            "UPDATE webdock_auth.plan_offer SET status='revoked' WHERE id=$1 AND customer_id=$2 AND status='pending' RETURNING id",
            [id(input.offerID), customerID],
          );
          if (!changed.rowCount)
            throw new PlanError("Pending offer not found.");
        } else {
          checkRevision(input.revision, revision);
          if (input.action === "assign-custom") {
            await saveSubscription(
              connection,
              customerID,
              {
                name: textField(input.name, 160, true),
                description: textField(input.description, 4000),
                base: parseAllowances(input, false, current?.base?.hosting),
              },
              current?.extras || zero,
              revision,
              user.id,
            );
          } else if (input.action === "extras") {
            if (!current)
              throw new PlanError(
                "Assign a plan before adding extra allowances.",
              );
            await saveSubscription(
              connection,
              customerID,
              { ...current, id: current.plan_id },
              parseAllowances(input, true, current.extras?.hosting),
              revision,
              user.id,
            );
          } else {
            const plan = input.planID
              ? (
                  await connection.query(
                    "SELECT * FROM webdock_auth.plan_template WHERE id=$1",
                    [id(input.planID)],
                  )
                ).rows[0]
              : null;
            if ((input.planID && !plan) || (input.action === "assign" && !plan))
              throw new PlanError("Plan not found.");
            if (input.action === "assign") {
              await saveSubscription(
                connection,
                customerID,
                {
                  ...plan,
                  base: {
                    ...plan.allowances,
                    hosting: plan.allowances.hosting ?? current?.base?.hosting,
                  },
                },
                current?.extras || zero,
                revision,
                user.id,
              );
            } else {
              const name = textField(input.name || plan?.name, 160, true),
                description = textField(
                  input.description ?? plan?.description,
                  4000,
                );
              const base = parseAllowances(
                input,
                false,
                plan?.allowances?.hosting ?? current?.base?.hosting,
              );
              effective(base, current?.extras || zero);
              const days = input.expiresDays || "14";
              if (!/^\d+$/.test(days) || Number(days) < 1 || Number(days) > 90)
                throw new PlanError(
                  "Offer expiry must be between 1 and 90 days.",
                );
              const token = randomBytes(32).toString("base64url");
              const origin = new URL(
                process.env.STUDIO_UI_ENABLED === "true"
                  ? studioURL()
                  : process.env.BETTER_AUTH_URL || "https://auth.webdock.dev",
              );
              if (
                origin.username ||
                origin.password ||
                (origin.protocol !== "https:" &&
                  !(
                    process.env.NODE_ENV !== "production" &&
                    origin.protocol === "http:" &&
                    ["localhost", "127.0.0.1"].includes(origin.hostname)
                  ))
              )
                throw new PlanError(
                  "A secure canonical authentication URL is required.",
                );
              offerURL = new URL(`/offers/${token}`, origin.origin).href;
              await connection.query(
                "INSERT INTO webdock_auth.plan_offer(customer_id,plan_id,token_hash,name,description,terms,base,offered_extras,expected_revision,expires_at,created_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,now()+$10::int*interval '1 day',$11)",
                [
                  customerID,
                  plan?.id || null,
                  createHash("sha256").update(token).digest("hex"),
                  name,
                  description,
                  textField(input.terms, 8000),
                  JSON.stringify(base),
                  JSON.stringify(current?.extras || zero),
                  revision,
                  Number(days),
                  user.id,
                ],
              );
            }
          }
        }
      }
      await audit(connection, user.id, input.action, target);
      await connection.query("COMMIT");
      return {
        message: offerURL
          ? "Offer created. Copy the private link now; it is only shown once."
          : "Plan change saved.",
        ...(offerURL ? { offerURL } : {}),
      };
    } catch (error) {
      await connection.query("ROLLBACK");
      throw error;
    } finally {
      connection.release();
    }
  });
}
async function offerByToken(token: string, connection: Connection = database) {
  if (!/^[A-Za-z0-9_-]{43}$/.test(token || ""))
    throw new PlanError("Offer not found.");
  const row = (
    await connection.query(
      "SELECT * FROM webdock_auth.plan_offer WHERE token_hash=$1",
      [createHash("sha256").update(token).digest("hex")],
    )
  ).rows[0];
  if (!row) throw new PlanError("Offer not found.");
  return row;
}
export async function getOffer(headers: Headers, token: string) {
  return withAuthenticatedDatabase(headers, async () => {
    const user = await actor(headers),
      row = await offerByToken(token),
      customer = await tenant(user, row.customer_id);
    const current = await subscription(row.customer_id),
      extras: Allowances = row.offered_extras;
    const status = offerStatus(row, current?.revision || 0);
    return {
      customerID: row.customer_id as string,
      customerName: customer.name as string,
      name: row.name as string,
      description: row.description as string,
      terms: row.terms as string,
      base: row.base as Allowances,
      extras,
      effective: effective(row.base, extras),
      status,
      expiresAt: row.expires_at.toISOString() as string,
      canAccept: customer.admin && status === "pending",
    };
  });
}
export async function acceptOffer(
  headers: Headers,
  token: string,
): Promise<{ message: string; customerID: string }> {
  return withAuthenticatedDatabase(headers, async () => {
    const initialUser = await actor(headers);
    const connection = await database.connect();
    try {
      await connection.query("BEGIN");
      const user = await actor(headers, connection, true, initialUser.id),
        initial = await offerByToken(token, connection);
      const customer = await tenant(
        user,
        initial.customer_id,
        connection,
        true,
      );
      if (!customer.admin)
        throw new PlanError(
          "Only this tenant's customer administrator can accept the offer.",
        );
      // Every plan writer takes the same tenant advisory lock, including first assignments and revocations.
      const row = await offerByToken(token, connection);
      if (row.status === "accepted") {
        await connection.query("COMMIT");
        return {
          message: "Offer already accepted.",
          customerID: row.customer_id,
        };
      }
      if (
        process.env.DATABASE_RLS_SECRET &&
        (
          await connection.query(
            "SELECT webdock_rls.billing_base_locked($1) AS locked",
            [row.customer_id],
          )
        ).rows[0].locked
      )
        throw new PlanError(
          "This allocation is locked by a retained billing agreement. Operator data review is required before changing its ownership.",
        );
      const current = await subscription(row.customer_id, connection);
      if (offerStatus(row, current?.revision || 0) !== "pending")
        throw new PlanError(
          "This offer is expired, revoked or stale. Request a new offer.",
        );
      await saveSubscription(
        connection,
        row.customer_id,
        {
          id: row.plan_id,
          name: row.name,
          description: row.description,
          base: row.base,
        },
        row.offered_extras,
        current?.revision || 0,
        user.id,
      );
      await connection.query(
        "UPDATE webdock_auth.plan_offer SET status='accepted',accepted_at=now(),accepted_by=$2 WHERE id=$1",
        [row.id, user.id],
      );
      await audit(connection, user.id, "accept-offer", row.customer_id);
      await connection.query("COMMIT");
      return {
        message:
          "Offer accepted. Your plan allocation has been updated; no payment was charged.",
        customerID: row.customer_id,
      };
    } catch (error) {
      await connection.query("ROLLBACK");
      throw error;
    } finally {
      connection.release();
    }
  });
}
