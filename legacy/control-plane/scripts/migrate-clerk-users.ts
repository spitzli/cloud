import { migrateClerkMailGrants } from "./clerk-mail-grants";
import { z } from "zod";
import { readFile } from "node:fs/promises";
import { createClerkClient, type ClerkClient } from "@clerk/backend";
import { Pool, type PoolClient } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
const id = z.string().regex(/^[1-9][0-9]{0,18}$/);
export const migrationManifest = z
  .object({
    environment: z.enum(["development", "production"]),
    applicationID: z.literal("app_3KjGKUGb7dYUMgkjKC0eCWghnOB"),
    instanceID: z.string().startsWith("ins_"),
    issuer: z.string().url(),
    accounts: z
      .array(
        z
          .object({
            localID: id,
            kind: z.enum(["operator", "customer"]),
            clerkUserID: z.string().startsWith("user_").optional(),
            firstName: z.string().trim().min(1).max(256).optional(),
            lastName: z.string().trim().min(1).max(256).optional(),
          })
          .strict(),
      )
      .length(4),
    teams: z
      .array(
        z
          .object({
            organizationID: id,
            customerID: id,
            clerkOrganizationID: z.string().startsWith("org_").optional(),
            members: z
              .array(
                z
                  .object({
                    localID: id,
                    role: z.enum(["owner", "admin", "member"]),
                  })
                  .strict(),
              )
              .min(1),
          })
          .strict(),
      )
      .length(2),
  })
  .strict()
  .superRefine((m, ctx) => {
    const operators = m.accounts.filter((a) => a.kind === "operator");
    if (
      operators.length !== 1 ||
      new Set(m.accounts.map((a) => a.localID)).size !== 4 ||
      new Set(m.teams.map((t) => t.organizationID)).size !== 2 ||
      new Set(m.teams.map((t) => t.customerID)).size !== 2
    )
      ctx.addIssue({
        code: "custom",
        message:
          "Four distinct identities and two distinct teams are required.",
      });
    const members = m.teams.flatMap((t) => t.members);
    if (
      members.some(
        (a) =>
          !m.accounts.some(
            (b) => b.localID === a.localID && b.kind === "customer",
          ),
      ) ||
      m.accounts
        .filter((a) => a.kind === "customer")
        .some((a) => !members.some((b) => b.localID === a.localID)) ||
      m.teams.some(
        (t) =>
          new Set(t.members.map((a) => a.localID)).size !== t.members.length ||
          !t.members.some((a) => a.role !== "member"),
      )
    )
      ctx.addIssue({
        code: "custom",
        message:
          "Review customer membership and administrator roles; never add the operator.",
      });
    if (
      new URL(m.issuer).origin !== m.issuer ||
      !m.issuer.startsWith("https://")
    )
      ctx.addIssue({
        code: "custom",
        message: "Use the exact HTTPS Clerk issuer.",
      });
  });
class MigrationError extends Error {}
const stop = (message: string): never => {
  throw new MigrationError(message);
};
type Manifest = z.infer<typeof migrationManifest>;
/** Explicit four-row batch, not an all-user importer. No password or session queries. */
export async function migrateClerkUsers(
  raw: unknown,
  db: Pick<PoolClient, "query">,
  provider: ClerkClient,
  apply = false,
  databaseEnvironment: "development" | "production" = "development",
) {
  const manifest = migrationManifest.parse(raw),
    instance = await provider.instance.get();
  if (instance.environmentType !== manifest.environment || manifest.environment !== databaseEnvironment)
    stop("Provider environment does not match the reviewed database environment.");
  if (instance.id !== manifest.instanceID)
    stop("The API key does not match the reviewed instance.");
  const report: Array<{
    kind: string;
    localID: string;
    providerID?: string;
    action: string;
  }> = [];
  const locals = (
    await db.query(
      'SELECT id,name,email,role,banned,"emailVerified" FROM webdock_auth."user" WHERE id=ANY($1::text[])',
      [manifest.accounts.map((a) => a.localID)],
    )
  ).rows;
  if (
    locals.length !== 4 ||
    new Set(locals.map((u) => String(u.email).toLowerCase())).size !== 4
  )
    stop("Four existing users with distinct login identifiers are required.");
  // Validate the entire reviewed local inventory before any provider mutation.
  for (const account of manifest.accounts) {
    const local = locals.find((u) => u.id === account.localID);
    if (
      !local ||
      (account.kind === "operator"
        ? local.role !== "operator"
        : local.role !== "user")
    )
      stop("Local role differs for " + account.localID);
  }
  for (const team of manifest.teams) {
    const local = (
      await db.query(
        "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        [team.customerID],
      )
    ).rows[0];
    if (local?.organization_id !== team.organizationID)
      stop("Customer mapping differs for " + team.customerID);
    const members = (
      await db.query(
        'SELECT "userId" AS id,role FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=ANY($2::text[])',
        [team.organizationID, manifest.accounts.filter((a) => a.kind === "customer").map((a) => a.localID)],
      )
    ).rows;
    if (
      members.length !== team.members.length ||
      team.members.some(
        (m) => !members.some((r) => r.id === m.localID && r.role === m.role),
      )
    )
      stop("Reviewed membership differs for " + team.organizationID);
    if (
      !team.members.some(
        (m) =>
          m.role !== "member" &&
          !locals.find((u) => u.id === m.localID)?.banned,
      )
    )
      stop(
        "An enabled customer administrator is required for " +
          team.organizationID,
      );
  }
  // Linking an operator changes mailbox authorization globally, not only in reviewed teams.
  const nativeMailSchema = (await db.query("SELECT to_regclass('webdock_mail.mailbox_grant') IS NOT NULL AS present")).rows[0]?.present;
  if (nativeMailSchema && (await db.query(
    "SELECT g.mailbox_id FROM webdock_mail.mailbox_grant g WHERE g.subject=ANY($1::text[]) AND g.enabled AND g.clerk_grant IS NULL AND NOT(g.customer_id=ANY($2::text[])) LIMIT 1",
    [manifest.accounts.map(account => account.localID), manifest.teams.map(team => team.customerID)],
  )).rows.length)
    stop("Enabled mailbox grants outside the reviewed teams require an explicit migration decision before identity linking.");
  const users = new Map<string, string>();
  const organizations = new Map<string, string>();
  for (const account of manifest.accounts) {
    const local = locals.find((u) => u.id === account.localID)!;
    const link = (
      await db.query(
        "SELECT clerk_user_id FROM webdock_auth.clerk_identity WHERE issuer=$1 AND user_id=$2",
        [manifest.issuer, account.localID],
      )
    ).rows[0];
    if (
      link &&
      account.clerkUserID &&
      link.clerk_user_id !== account.clerkUserID
    )
      stop("Conflicting identity mapping for " + account.localID);
    const explicit = link?.clerk_user_id || account.clerkUserID;
    const matches = explicit
      ? [await provider.users.getUser(explicit)]
      : (
          await provider.users.getUserList({
            externalId: [account.localID],
            limit: 2,
          })
        ).data;
    if (matches.length > 1)
      stop("Ambiguous provider identity for " + account.localID);
    let user = matches[0];
    const emailMatches = (
      await provider.users.getUserList({
        emailAddress: [local.email],
        limit: 2,
      })
    ).data;
    if (emailMatches.some((u) => u.id !== user?.id))
      stop(
        "Email conflict for " +
          account.localID +
          "; choose distinct identifiers explicitly.",
      );
    if (user) {
      if (user.externalId && user.externalId !== account.localID)
        stop("External ID conflict for " + account.localID);
      const address = user.emailAddresses.find(
        (e) =>
          e.emailAddress.toLowerCase() === String(local.email).toLowerCase(),
      );
      if (
        !address ||
        (address.verification?.status === "verified") !== local.emailVerified
      )
        stop("Review email verification for " + account.localID);
      if (!user.externalId) {
        if (!explicit) stop("Explicit link required for " + account.localID);
        if (apply)
          user = await provider.users.updateUser(user.id, {
            externalId: account.localID,
          });
      }
    } else if (apply) {
      user = await provider.users.createUser({
        externalId: account.localID,
        emailAddress: [local.email],
        emailAddressIdentificationStatus: [
          local.emailVerified ? "verified" : "reserved",
        ],
        firstName: account.firstName ?? local.name,
        ...(account.lastName ? { lastName: account.lastName } : {}),
        skipPasswordRequirement: true,
      });
    }
    report.push({
      kind: "user",
      localID: account.localID,
      providerID: user?.id,
      action: link ? "unchanged" : user ? "link" : "create",
    });
    if (!user) continue;
    users.set(account.localID, user.id);
    if(apply && (user.deleteSelfEnabled!==false || user.createOrganizationEnabled!==false))await provider.users.updateUser(user.id,{deleteSelfEnabled:false,createOrganizationEnabled:false});
    if (apply) {
      await db.query(
        "INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id,enabled) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        [manifest.issuer, user.id, account.localID, !local.banned],
      );
      const saved = (
        await db.query(
          "SELECT user_id FROM webdock_auth.clerk_identity WHERE issuer=$1 AND clerk_user_id=$2",
          [manifest.issuer, user.id],
        )
      ).rows[0];
      if (saved?.user_id !== account.localID)
        stop("Concurrent mapping conflict for " + account.localID);
    }
  }
  for (const team of manifest.teams) {
    const local = (
      await db.query("SELECT name FROM webdock_auth.organization WHERE id=$1", [
        team.organizationID,
      ])
    ).rows[0];
    const link = (
      await db.query(
        "SELECT clerk_organization_id FROM webdock_auth.clerk_organization WHERE issuer=$1 AND organization_id=$2",
        [manifest.issuer, team.organizationID],
      )
    ).rows[0];
    if (
      link &&
      team.clerkOrganizationID &&
      link.clerk_organization_id !== team.clerkOrganizationID
    )
      stop("Conflicting team mapping for " + team.organizationID);
    const explicit = link?.clerk_organization_id || team.clerkOrganizationID,
      slug = "webdock-" + team.organizationID;
    let organization;
    try {
      organization = await provider.organizations.getOrganization(
        explicit ? { organizationId: explicit } : { slug },
      );
    } catch (error) {
      if ((error as { status?: number }).status !== 404) throw error;
      if (explicit) stop("Reviewed provider team is missing.");
    }
    if (
      organization &&
      !explicit &&
      organization.privateMetadata.webdockOrganizationID !== team.organizationID
    )
      stop("Provider team conflict for " + team.organizationID);
    if (!organization && apply)
      organization = await provider.organizations.createOrganization({
        name: local.name,
        slug,
        privateMetadata: {
          webdockOrganizationID: team.organizationID,
          webdockCustomerID: team.customerID,
        },
      });
    report.push({
      kind: "team",
      localID: team.organizationID,
      providerID: organization?.id,
      action: link ? "unchanged" : organization ? "link" : "create",
    });
    if (!organization) continue;
    organizations.set(team.organizationID, organization.id);
    const existing = await provider.organizations.getOrganizationMembershipList(
      { organizationId: organization.id, limit: 100 },
    );
    if (
      existing.totalCount > 100 ||
      existing.data.some(
        (m) =>
          !team.members.some(
            (a) => users.get(a.localID) === m.publicUserData?.userId,
          ),
      )
    )
      stop("Unexpected provider membership for " + team.organizationID);
    for (const member of [...team.members].sort(
      (a, b) => Number(a.role === "member") - Number(b.role === "member"),
    )) {
      const userId = users.get(member.localID);
      if (!userId) continue;
      const role = member.role === "member" ? "org:member" : "org:admin",
        old = existing.data.find((m) => m.publicUserData?.userId === userId);
      if(link&&(!old||old.role!==role))stop('Provider membership changed for '+member.localID+'; an old migration manifest must not restore removed access.');
      if (apply && !old)
        await provider.organizations.createOrganizationMembership({
          organizationId: organization.id,
          userId,
          role,
        });
      else if (apply && old?.role !== role)
        await provider.organizations.updateOrganizationMembership({
          organizationId: organization.id,
          userId,
          role,
        });
    }
    if (apply) {
      await db.query(
        "INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
        [manifest.issuer, organization.id, team.organizationID],
      );
      const saved = (
        await db.query(
          "SELECT organization_id FROM webdock_auth.clerk_organization WHERE issuer=$1 AND clerk_organization_id=$2",
          [manifest.issuer, organization.id],
        )
      ).rows[0];
      if (saved?.organization_id !== team.organizationID)
        stop("Concurrent team mapping conflict for " + team.organizationID);
    }
  }
  const mailboxGrants = await migrateClerkMailGrants({db, issuer: manifest.issuer, users,
    teams: manifest.teams.map(team => ({organizationID: team.organizationID, clerkOrganizationID: organizations.get(team.organizationID)})),
    provider, dryRun: !apply, subjects: manifest.accounts.map(account => account.localID)});
  return {
    mailboxGrants,
    apply,
    applicationID: manifest.applicationID,
    instanceID: manifest.instanceID,
    accounts: 4,
    teams: 2,
    report,
  };
}
if (import.meta.main) {
  let db: Pool | undefined;
  try {
    const arg = (name: string) => {
      const i = process.argv.indexOf(name);
      return i < 0 ? undefined : process.argv[i + 1];
    };
    const file = arg("--manifest"),
      instance = arg("--instance"),
      app = arg("--app");
    if (!file || !instance || app !== "app_3KjGKUGb7dYUMgkjKC0eCWghnOB")
      stop(
        "Supply --manifest, --instance and the exact --app. Dry-run is the default.",
      );
    const manifest = migrationManifest.parse(
      JSON.parse(await readFile(file!, "utf8")),
    );
    if (
      manifest.instanceID !== instance ||
      manifest.applicationID !== app ||
      manifest.issuer !== process.env.CLERK_ISSUER
    )
      stop("Explicit target does not match the reviewed manifest and issuer.");
    const publishableKey = process.env.CLERK_PUBLISHABLE_KEY;
    const prefix = manifest.environment === "production" ? "pk_live_" : "pk_test_";
    const secretPrefix = manifest.environment === "production" ? "sk_live_" : "sk_test_";
    if (!publishableKey?.startsWith(prefix) || !process.env.CLERK_SECRET_KEY?.startsWith(secretPrefix) ||
        "https://" + Buffer.from(publishableKey.slice(prefix.length), "base64").toString().replace(/\$$/, "") !== manifest.issuer)
      stop("Provider key environment and issuer do not match the reviewed target.");
    const raw = process.env.DATABASE_URL_UNPOOLED;
    if (!raw) stop("Explicit offline database connection required.");
    const target = new URL(raw!),
      local =
        target.hostname === "127.0.0.1" &&
        ["/webdock_api_test", "/webdock_rls_test"].includes(target.pathname);
    if (
      arg("--database") !==
      target.hostname + ":" + (target.port || "5432") + target.pathname
    )
      stop("Confirm --database host:port/database.");
    if (
      !local &&
      (!process.env.DATABASE_CA_CERT ||
        [
          "postgres",
          "webdock_auth_runtime",
          "webdock_auth_bootstrap",
          "webdock_worker_runtime",
        ].includes(target.username))
    )
      stop("Use the dedicated offline migrator with a verified CA.");
    db = new Pool({
      ...(local
        ? { connectionString: raw }
        : verifiedPostgresConfig(raw!, process.env.DATABASE_CA_CERT)),
      max: 1,
    });
    const provider = createClerkClient({
      secretKey: process.env.CLERK_SECRET_KEY,
      telemetry: { disabled: true },
    });
    console.log(
      JSON.stringify(
        await migrateClerkUsers(
          manifest,
          db,
          provider,
          process.argv.includes("--apply"),
          local ? "development" : "production",
        ),
      ),
    );
  } catch (error) {
    // Provider messages can contain identifiers; expose only bounded diagnostic codes.
    const diagnostic = error as { status?: unknown; code?: unknown; errors?: Array<{ code?: unknown }> };
    const safeCode = (value: unknown) => typeof value === "string" && /^[a-zA-Z0-9_]{1,80}$/.test(value) ? value : undefined;
    console.error(JSON.stringify({event: "clerk_import_failed", status: typeof diagnostic.status === "number" ? diagnostic.status : undefined,
      code: safeCode(diagnostic.code), providerCodes: Array.isArray(diagnostic.errors) ? diagnostic.errors.map(item => safeCode(item.code)).filter(Boolean) : undefined}));
    console.error(
      error instanceof MigrationError
        ? error.message
        : "Migration stopped. Review the private manifest, database and provider state; no automatic retry was performed.",
    );
    process.exitCode = 1;
  } finally {
    await db?.end();
  }
}
