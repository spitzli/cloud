import type { PoolClient } from "pg";
import { capacityRlsSQL } from "@webdock/mail-core/capacity";
import { buildDatabaseRls } from "../../src/db/database-rls-schema";
import { databaseClusterRlsSQL } from "../../src/db/database-cluster-rls";
import { databaseMailRlsSQL } from "../../src/db/database-mail-rls";
const schemas = [
  "public",
  "webdock",
  "webdock_admin",
  "webdock_auth",
  "webdock_mail",
];
const q = (s: string) => '"' + s.replaceAll('"', '""') + '"';
const loginRoles = [
  "webdock_auth_bootstrap",
  "webdock_auth_runtime",
  "webdock_worker_runtime",
  "webdock_admin_runtime",
  "webdock_registry_runtime",
  "webdock_runtime",
  "webdock_browser_website",
  "webdock_cms_archive_runtime",
];
const owners = [
  "webdock_platform_owner",
  "webdock_policy_owner",
  "webdock_website_owner",
];

/** Offline caller owns transaction, advisory lock and exact-target authentication. */
export async function installPlatformPolicies(
  db: PoolClient,
  expected: string,
  contextSecret: string,
  requireAllSchemas = true,
) {
  const identity = (await db.query("SELECT current_database() AS name"))
    .rows[0];
  if (identity.name !== expected)
    throw Error("Connected database differs from guarded target");
  const roles = (
    await db.query(
      "SELECT rolname,rolsuper,rolbypassrls,rolcreatedb,rolcreaterole,rolreplication,rolcanlogin FROM pg_roles WHERE rolname=ANY($1)",
      [[...loginRoles, ...owners]],
    )
  ).rows;
  if (
    roles.length !== loginRoles.length + owners.length ||
    roles.some(
      (r) =>
        r.rolsuper ||
        r.rolbypassrls ||
        r.rolcreatedb ||
        r.rolcreaterole ||
        r.rolreplication ||
        r.rolcanlogin !== loginRoles.includes(r.rolname),
    )
  )
    throw Error(
      "Provision separate restricted runtime and NOLOGIN owner roles first",
    );
  for (const role of owners) {
    const inherited = (
      await db.query(
        "SELECT rolname FROM pg_roles WHERE oid <> $1::regrole AND pg_has_role($1,oid,'MEMBER')",
        [role],
      )
    ).rows;
    if (
      inherited.some(
        (r) =>
          role !== "webdock_platform_owner" ||
          r.rolname !== "webdock_website_owner",
      )
    )
      throw Error("Unexpected owner role membership on " + role);
  }
  await db.query(
    `REVOKE CREATE,TEMPORARY ON DATABASE ${q(expected)} FROM PUBLIC,${loginRoles.join(",")}`,
  );
  for (const role of loginRoles) {
    const memberships = (
      await db.query(
        "SELECT rolname FROM pg_roles WHERE oid <> $1::regrole AND pg_has_role($1,oid,'MEMBER')",
        [role],
      )
    ).rows.map((r) => r.rolname);
    if (
      memberships.some(
        (r) =>
          role !== "webdock_browser_website" || r !== "webdock_website_owner",
      )
    )
      throw Error("Unexpected inherited privilege on " + role);
    const schema = [
      "webdock_admin_runtime",
      "webdock_registry_runtime",
    ].includes(role)
      ? "webdock_admin"
      : role === "webdock_runtime" || role === "webdock_browser_website"
        ? "webdock"
        : role === "webdock_cms_archive_runtime"
          ? "pg_catalog"
          : "webdock_auth";
    await db.query(
      `ALTER ROLE ${q(role)} IN DATABASE ${q(expected)} SET search_path=${schema},pg_catalog`,
    );
    await db.query(`GRANT CONNECT ON DATABASE ${q(expected)} TO ${q(role)}`);
  }
  await db.query(
    "CREATE SCHEMA IF NOT EXISTS webdock_rls AUTHORIZATION webdock_policy_owner",
  );
  const extension = (
    await db.query(
      "SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='pgcrypto'",
    )
  ).rows[0];
  if (extension?.nspname !== "webdock_rls")
    throw Error(
      "Provision pgcrypto in the private policy schema through the root bootstrap first",
    );
  const cryptoOwners = (
    await db.query(
      "SELECT DISTINCT r.rolname FROM pg_proc p JOIN pg_roles r ON r.oid=p.proowner JOIN pg_depend d ON d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e' JOIN pg_extension e ON e.oid=d.refobjid WHERE e.extname='pgcrypto'",
    )
  ).rows;
  if (cryptoOwners.some((r) => r.rolname !== "webdock_policy_owner"))
    throw Error(
      "Root bootstrap must assign pgcrypto functions to the private policy owner",
    );
  await db.query(
    "REVOKE ALL ON SCHEMA webdock_rls FROM PUBLIC; REVOKE ALL ON ALL FUNCTIONS IN SCHEMA webdock_rls FROM PUBLIC",
  );
  await db.query(
    `CREATE TABLE IF NOT EXISTS webdock_rls.context_key(id boolean PRIMARY KEY DEFAULT true CHECK(id),secret text NOT NULL); ALTER TABLE webdock_rls.context_key OWNER TO webdock_policy_owner; REVOKE ALL ON webdock_rls.context_key FROM PUBLIC,${loginRoles.join(",")}`,
  );
  const previous = (
    await db.query("SELECT secret FROM webdock_rls.context_key WHERE id")
  ).rows[0];
  if (previous && previous.secret !== contextSecret)
    throw Error(
      "Existing signing key differs; rotation requires coordinated deployment",
    );
  if (!previous)
    await db.query("INSERT INTO webdock_rls.context_key(secret) VALUES($1)", [
      contextSecret,
    ]);
  await db.query(
    "GRANT EXECUTE ON FUNCTION webdock_rls.hmac(text,text,text) TO webdock_policy_owner",
  );
  const tables = (
    await db.query(
      `SELECT n.nspname AS schema,c.relname AS name,array_agg(a.attname::text ORDER BY a.attnum) AS columns
    FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
    WHERE n.nspname=ANY($1) AND c.relkind IN ('r','p') GROUP BY n.nspname,c.relname ORDER BY 1,2`,
      [schemas],
    )
  ).rows;
  if (
    !(
      requireAllSchemas
        ? schemas
        : ["webdock_auth", "webdock_admin", "webdock_mail"]
    ).every((s) => tables.some((t) => t.schema === s))
  )
    throw Error(
      "Restore all selected platform schemas before installing policies",
    );
  const plan = buildDatabaseRls(tables);
  if (plan.unknownTables.length)
    throw Error("Unclassified tables: " + plan.unknownTables.join(", "));
  // Acquire the complete table lock set before changing grants. A busy old writer
  // must make the offline cutover fail immediately, not deadlock mid-installation.
  await db.query(
    `LOCK TABLE ${tables.map((t) => q(t.schema) + "." + q(t.name)).join(",")} IN ACCESS EXCLUSIVE MODE NOWAIT`,
  );
  // ALTER FUNCTION OWNER requires CREATE for the new owner, even when the
  // offline caller already inherits both owner roles. The website tables briefly
  // move through platform ownership and need the same temporary schema privilege.
  await db.query(
    "GRANT USAGE,CREATE ON SCHEMA webdock_rls,webdock TO webdock_platform_owner",
  );
  for (const schema of schemas)
    await db.query(
      `REVOKE ALL ON SCHEMA ${q(schema)} FROM PUBLIC,${loginRoles.join(",")}`,
    );
  for (const table of tables) {
    const columns = table.columns.map(q).join(",");
    await db.query(
      `REVOKE SELECT(${columns}),INSERT(${columns}),UPDATE(${columns}),REFERENCES(${columns}) ON ${q(table.schema)}.${q(table.name)} FROM PUBLIC,${loginRoles.join(",")},webdock_policy_owner`,
    );
  }
  await db.query(plan.sql);
  await db.query(capacityRlsSQL);
  await db.query(databaseClusterRlsSQL);
  await db.query(databaseMailRlsSQL);
  // The website browser can administer only its own schema, never platform identities.
  await db.query(
    "GRANT webdock_website_owner TO webdock_browser_website; ALTER SCHEMA webdock OWNER TO webdock_website_owner",
  );
  for (const t of tables.filter((t) => t.schema === "webdock")) {
    const table = q(t.schema) + "." + q(t.name);
    await db.query(
      `ALTER TABLE ${table} OWNER TO webdock_website_owner; CREATE POLICY website_owner ON ${table} TO webdock_website_owner USING(true) WITH CHECK(true)`,
    );
  }
  for (const schema of schemas) {
    await db.query(
      `REVOKE ALL ON SCHEMA ${q(schema)} FROM PUBLIC; REVOKE ALL ON ALL SEQUENCES IN SCHEMA ${q(schema)} FROM PUBLIC,${loginRoles.join(",")}; REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA ${q(schema)} FROM PUBLIC,${loginRoles.join(",")}`,
    );
  }
  const routines = (
    await db.query(
      "SELECT n.nspname AS schema,p.proname,pg_get_function_identity_arguments(p.oid) AS args,p.prokind FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=ANY($1)",
      [schemas],
    )
  ).rows;
  for (const routine of routines) {
    if (!["f", "p"].includes(routine.prokind))
      throw Error("Unexpected routine kind");
    const routineName = `${q(routine.schema)}.${q(routine.proname)}(${routine.args})`;
    await db.query(
      `ALTER ${routine.prokind === "p" ? "PROCEDURE" : "FUNCTION"} ${routineName} OWNER TO ${routine.schema === "webdock" ? "webdock_website_owner" : "webdock_platform_owner"}`,
    );
  }
  const sequences = (
    await db.query(
      "SELECT n.nspname AS schema,c.relname AS name FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind='S' AND n.nspname=ANY($1)",
      [schemas],
    )
  ).rows;
  for (const s of sequences) {
    const sequence = q(s.schema) + "." + q(s.name);
    // Owned sequences follow table ownership; ALTER OWNER is safe for detached sequences too.
    await db.query(
      `ALTER SEQUENCE ${sequence} OWNER TO ${s.schema === "webdock" ? "webdock_website_owner" : "webdock_platform_owner"}`,
    );
    const roles =
      s.schema === "webdock"
        ? ["webdock_runtime", "webdock_browser_website"]
        : s.schema === "webdock_admin"
          ? ["webdock_admin_runtime"]
          : s.schema === "webdock_auth"
            ? [
                "webdock_auth_bootstrap",
                "webdock_auth_runtime",
                "webdock_worker_runtime",
              ]
            : s.schema === "webdock_mail"
              ? ["webdock_auth_runtime", "webdock_worker_runtime"]
              : [];
    if (roles.length)
      await db.query(
        `GRANT USAGE,SELECT ON SEQUENCE ${sequence} TO ${roles.join(",")}`,
      );
  }
  const types = (
    await db.query(
      "SELECT n.nspname AS schema,t.typname FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname=ANY($1) AND t.typtype IN ('e','d')",
      [schemas],
    )
  ).rows;
  for (const type of types)
    await db.query(
      `ALTER TYPE ${q(type.schema)}.${q(type.typname)} OWNER TO ${type.schema === "webdock" ? "webdock_website_owner" : "webdock_platform_owner"}`,
    );
  for (const schema of schemas)
    await db.query(
      `ALTER SCHEMA ${q(schema)} OWNER TO ${schema === "webdock" ? "webdock_website_owner" : "webdock_platform_owner"}`,
    );
  await db.query(`GRANT EXECUTE ON FUNCTION webdock_auth.next_snowflake() TO webdock_auth_bootstrap,webdock_auth_runtime,webdock_worker_runtime;
    GRANT EXECUTE ON FUNCTION webdock_admin.next_snowflake() TO webdock_admin_runtime,webdock_registry_runtime;
    GRANT EXECUTE ON FUNCTION webdock_admin.lock_hosting_project(varchar) TO webdock_auth_runtime,webdock_worker_runtime;
    ALTER DEFAULT PRIVILEGES FOR ROLE webdock_website_owner IN SCHEMA webdock GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO webdock_runtime,webdock_browser_website;
    ALTER DEFAULT PRIVILEGES FOR ROLE webdock_website_owner IN SCHEMA webdock GRANT USAGE,SELECT ON SEQUENCES TO webdock_runtime,webdock_browser_website;
    ALTER DEFAULT PRIVILEGES FOR ROLE webdock_website_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
    ALTER DEFAULT PRIVILEGES FOR ROLE webdock_platform_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
    ALTER DEFAULT PRIVILEGES FOR ROLE webdock_policy_owner REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;`);
  await db.query(
    "REVOKE CREATE ON SCHEMA webdock_rls FROM webdock_platform_owner; REVOKE USAGE,CREATE ON SCHEMA webdock FROM webdock_platform_owner",
  );

  return {
    installed: true,
    tables: tables.length,
    forceRls: true,
    archiveRuntimeAccess: false,
  };
}
