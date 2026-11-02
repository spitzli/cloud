import { Pool, type PoolClient } from "pg";
import { RegistryError as APIError } from "../registry/service";
import { databaseURL } from "../../db/client";

const managedSchemaName = (value: string) =>
  /^[a-z][a-z0-9_]{0,45}$/.test(value) &&
  value !== "public" &&
  value !== "information_schema" &&
  !/^(?:pg_|webdock(?:_|$))/.test(value);

/** Only a dedicated, schema-owning runtime credential may be used, never the platform owner. */
export function managedDatabase(
  schema: string,
  raw = process.env.WEBDOCK_MANAGED_DATABASES || "{}",
) {
  if (!managedSchemaName(schema))
    throw new APIError(
      "This database is not registered for managed deletion.",
      409,
    );
  let config: Record<string, { url?: string }>;
  try {
    config = JSON.parse(raw);
  } catch {
    throw new APIError("Managed database deletion is not configured.", 503);
  }
  const entry = config?.[schema];
  if (!entry?.url)
    throw new APIError(
      "Managed database deletion is not configured for this website.",
      409,
    );
  const url = databaseURL(entry.url);
  if (
    !["postgres:", "postgresql:"].includes(url.protocol) ||
    decodeURIComponent(url.username) !== `${schema}_runtime` ||
    !url.password ||
    !url.hostname.endsWith(".neon.tech") ||
    url.searchParams.get("sslmode") !== "verify-full"
  )
    throw new APIError(
      "The managed database credential is not safely scoped.",
      409,
    );
  return { url: url.toString(), role: `${schema}_runtime` };
}
export async function inspectManagedDatabase(
  schema: string,
  remove = false,
  allowMissing = false,
) {
  const config = managedDatabase(schema);
  const pool = new Pool({
    connectionString: config.url,
    max: 1,
    connectionTimeoutMillis: 10000,
  });
  try {
    const client = await pool.connect();
    try {
      return await inspectManagedConnection(
        client,
        schema,
        config.role,
        remove,
        allowMissing,
      );
    } finally {
      client.release();
    }
  } finally {
    await pool.end();
  }
}

/** Internal transaction helper; callers must provide a dedicated scoped role. */
export async function inspectManagedConnection(
  client: PoolClient,
  schema: string,
  role: string,
  remove = false,
  allowMissing = false,
) {
  if (!managedSchemaName(schema) || role !== `${schema}_runtime`)
    throw new APIError("Invalid managed database mapping.", 409);
  try {
    await client.query("BEGIN");
    await client.query("SET LOCAL lock_timeout='5s'");
    await client.query("SET LOCAL statement_timeout='30s'");
    const actor = (
      await client.query(
        `SELECT r.rolname, r.rolsuper, r.rolcreaterole, r.rolcreatedb, r.rolreplication, r.rolbypassrls,
      EXISTS(SELECT 1 FROM pg_auth_members m JOIN pg_roles grantee ON grantee.oid=m.member
        WHERE m.member=r.oid OR (m.roleid=r.oid AND NOT grantee.rolsuper AND grantee.oid<>(SELECT datdba FROM pg_database WHERE datname=current_database()))) AS memberships,
      has_database_privilege(current_user,current_database(),'CREATE') AS database_create,
      EXISTS(SELECT 1 FROM pg_namespace n WHERE n.nspname<>$1 AND n.nspname NOT LIKE 'pg_temp_%' AND n.nspname NOT LIKE 'pg_toast_temp_%'
        AND (n.nspowner=r.oid OR has_schema_privilege(current_user,n.oid,'CREATE'))) AS other_schema_write
      FROM pg_roles r WHERE r.rolname=current_user`,
        [schema],
      )
    ).rows[0];
    if (
      !actor ||
      actor.rolname !== role ||
      actor.rolsuper ||
      actor.rolcreaterole ||
      actor.rolcreatedb ||
      actor.rolreplication ||
      actor.rolbypassrls ||
      actor.memberships ||
      actor.database_create ||
      actor.other_schema_write
    )
      throw new APIError(
        "The database role has privileges beyond this website.",
        409,
      );
    const { rows } = await client.query(
      "SELECT n.oid, r.rolname AS owner, current_user AS actor FROM pg_namespace n JOIN pg_roles r ON r.oid=n.nspowner WHERE n.nspname=$1",
      [schema],
    );
    if (!rows.length) {
      if (!allowMissing)
        throw new APIError(
          "The registered database schema was not found.",
          409,
        );
      await client.query("ROLLBACK");
      return { schema, alreadyAbsent: true };
    }
    if (rows[0].owner !== role || rows[0].actor !== role)
      throw new APIError(
        "Database ownership does not match this website.",
        409,
      );
    // Reject schema sharing, including PUBLIC access. With only the dedicated role
    // admitted, other application roles cannot attach new references during removal.
    // Database owners/superusers remain trusted infrastructure administrators.
    const shared = await client.query(
      `SELECT 1 FROM pg_namespace n
      CROSS JOIN LATERAL aclexplode(coalesce(n.nspacl,acldefault('n',n.nspowner))) a
      WHERE n.oid=$1 AND a.grantee<>n.nspowner LIMIT 1`,
      [rows[0].oid],
    );
    if (shared.rowCount)
      throw new APIError(
        "The database schema is accessible to another role.",
        409,
      );
    if (remove) {
      const relations = await client.query(
        `SELECT c.relname FROM pg_class c WHERE c.relnamespace=$1 AND c.relkind IN ('r','p','v','m','f') ORDER BY c.oid`,
        [rows[0].oid],
      );
      const quote = (name: string) => '"' + name.replaceAll('"', '""') + '"';
      // Lock ordinary/partitioned tables and views before inspecting dependencies;
      // concurrent FK/view creation needs conflicting relation locks.
      if (relations.rows.length)
        await client.query(
          "LOCK TABLE " +
            relations.rows
              .map((r) => quote(schema) + "." + quote(r.relname))
              .join(",") +
            " IN ACCESS EXCLUSIVE MODE",
        );
    }
    // Follow the schema dependency graph, including views and FK constraints. Never cascade into another application.
    const dependencies = await client.query(
      `WITH RECURSIVE edges(classid,objid,objsubid,refclassid,refobjid) AS (
      SELECT classid,objid,objsubid,refclassid,refobjid FROM pg_depend
      UNION ALL SELECT refclassid,refobjid,refobjsubid,classid,objid FROM pg_depend WHERE deptype IN ('i','a','e')
    ), objects(classid,objid,objsubid) AS (
      SELECT 'pg_namespace'::regclass::oid,$1::oid,0
      UNION SELECT d.classid,d.objid,d.objsubid FROM edges d JOIN objects o ON d.refclassid=o.classid AND d.refobjid=o.objid
    ) SELECT DISTINCT i.schema FROM objects o CROSS JOIN LATERAL pg_identify_object(o.classid,o.objid,o.objsubid) i
      WHERE (i.schema IS NOT NULL AND i.schema<>$2 AND i.schema NOT LIKE 'pg_toast%') OR o.classid='pg_extension'::regclass`,
      [rows[0].oid, schema],
    );
    if (dependencies.rowCount)
      throw new APIError(
        "Another application depends on this database. Deletion is blocked.",
        409,
      );
    if (remove) await client.query(`DROP SCHEMA "${schema}" CASCADE`);
    await client.query(remove ? "COMMIT" : "ROLLBACK");
    return { schema, alreadyAbsent: false };
  } catch (error) {
    await client.query("ROLLBACK").catch(() => {});
    throw error;
  }
}
