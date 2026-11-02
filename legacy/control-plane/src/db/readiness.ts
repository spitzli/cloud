import { withDatabaseScope } from "@webdock/database-runtime";
import { is } from "drizzle-orm";
import { PgTable, getTableConfig } from "drizzle-orm/pg-core";
import * as authSchema from "./schema/auth";
import * as registrySchema from "./schema/registry";
type Reader = {
  query: (
    sql: string,
    values?: unknown[],
  ) => Promise<{ rows: Record<string, unknown>[] }>;
};
type Table = { schema: string; name: string; columns: string[] };
export async function verifyRole(db: Reader, expected: string) {
  const row = (
    await db.query(`SELECT current_user AS name,session_user AS session,r.rolsuper AS superuser,r.rolbypassrls AS bypass,
 (EXISTS(SELECT 1 FROM pg_catalog.pg_auth_members m WHERE m.member=r.oid)
 OR EXISTS(SELECT 1 FROM pg_catalog.pg_database d WHERE d.datname=current_database() AND d.datdba=r.oid)
 OR EXISTS(SELECT 1 FROM pg_catalog.pg_namespace n WHERE n.nspname IN ('webdock_auth','webdock_admin','webdock_mail','webdock_rls','webdock_cache') AND (n.nspowner=r.oid OR has_schema_privilege(current_user,n.oid,'CREATE')))
 OR EXISTS(SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN ('webdock_auth','webdock_admin','webdock_mail','webdock_rls','webdock_cache') AND c.relowner=r.oid)
 OR EXISTS(SELECT 1 FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname IN ('webdock_auth','webdock_admin','webdock_mail','webdock_rls','webdock_cache') AND p.proowner=r.oid)) AS owner
 FROM pg_catalog.pg_roles r WHERE r.rolname=current_user`)
  ).rows[0];
  if (
    !row ||
    row.name !== expected ||
    row.session !== expected ||
    row.superuser !== false ||
    row.bypass !== false ||
    row.owner !== false
  )
    throw Error("Database runtime role is unsafe");
}
export async function verifySchema(db: Reader, expected: Table[]) {
  const rows = (
    await db.query(
      `SELECT n.nspname AS schema,c.relname AS name,c.relrowsecurity AS rls,c.relforcerowsecurity AS forced,
 array_agg(a.attname::text) AS columns FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
 JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped
 WHERE n.nspname=ANY($1::text[]) AND c.relkind IN ('r','p') GROUP BY n.nspname,c.relname,c.relrowsecurity,c.relforcerowsecurity`,
      [[...new Set(expected.map((t) => t.schema))]],
    )
  ).rows;
  for (const table of expected) {
    const found = rows.find(
      (row) => row.schema === table.schema && row.name === table.name,
    );
    if (
      !found ||
      found.rls !== true ||
      found.forced !== true ||
      !Array.isArray(found.columns) ||
      table.columns.some(
        (column) => !(found.columns as unknown[]).includes(column),
      )
    )
      throw Error("Required isolated database schema is unavailable");
  }
}
function mappedTables(schema: Record<string, unknown>): Table[] {
  return Object.values(schema)
    .filter((value) => is(value, PgTable))
    .map((table) => {
      const config = getTableConfig(table as PgTable);
      return {
        schema: config.schema || "public",
        name: config.name,
        columns: config.columns.map((column) => column.name),
      };
    });
}
export async function verifyDatabaseReadiness(
  pools: { business: Reader; authentication: Reader; worker: Reader },
  domains: string[],
  registry?: Reader,
) {
  await Promise.all([
    verifyRole(pools.business, "webdock_auth_runtime"),
    verifyRole(pools.authentication, "webdock_auth_bootstrap"),
    verifyRole(pools.worker, "webdock_worker_runtime"),
  ]);
  for (const pool of [
    pools.business,
    pools.worker,
    ...(registry ? [registry] : []),
  ]) {
    const signed = await withDatabaseScope(
      { kind: "system", purpose: "api-readiness" },
      () =>
        pool.query("SELECT webdock_rls.request_scope() IS NOT NULL AS valid"),
    );
    if (signed.rows[0]?.valid !== true)
      throw Error("Database context signature is invalid");
  }
  const expected = domains.includes("auth") ? mappedTables(authSchema) : [];
  if (domains.includes("registry")) {
    if (!registry) throw Error("Registry role required");
    await verifyRole(registry, "webdock_registry_runtime");
    expected.push(...mappedTables({users: registrySchema.users, customers: registrySchema.customers, projects: registrySchema.projects, auditEvents: registrySchema.auditEvents}));
  }
  if (domains.includes("lifecycle"))
    expected.push({
      schema: "webdock_admin",
      name: "project_deletion",
      columns: ["project_id", "claim_token", "lease_until", "completed_steps"],
    });
  if (domains.includes("integrations"))
    expected.push({
      schema: "webdock_auth",
      name: "provider_event_receipt",
      columns: ["provider", "event_id", "body_hash", "result"],
    });
  if (expected.length) await verifySchema(pools.authentication, expected);
}

export async function verifyWorkerReadiness(pool: Reader, domains: string[]) {
  await verifyRole(pool, "webdock_worker_runtime");
  const signed = await withDatabaseScope(
    { kind: "system", purpose: "api-readiness" },
    () => pool.query("SELECT webdock_rls.request_scope() IS NOT NULL AS valid"),
  );
  if (signed.rows[0]?.valid !== true)
    throw Error("Database context signature is invalid");
  const tables: Table[] = [];
  if (domains.some((domain) => ["mail", "storage", "hosting"].includes(domain)))
    tables.push({
      schema: "webdock_auth",
      name: "api_job",
      columns: [
        "id",
        "name",
        "status",
        "claim_token",
        "lease_until",
        "available_at",
      ],
    });
  if (domains.includes("hosting"))
    tables.push({
      schema: "webdock_auth",
      name: "hosting_setup",
      columns: ["id", "customer_id", "project_id", "generation", "lease_until"],
    });
  if (domains.includes("storage"))
    tables.push({
      schema: "webdock_auth",
      name: "tenant_storage",
      columns: ["id", "customer_id"],
    });
  if (domains.includes("mail"))
    tables.push({
      schema: "webdock_auth",
      name: "mail_message",
      columns: ["id", "customer_id"],
    });
  await verifySchema(pool, tables);
}
