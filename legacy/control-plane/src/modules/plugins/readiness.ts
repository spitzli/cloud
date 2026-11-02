import { withDatabaseScope } from "@webdock/database-runtime";
import type { Pool } from "pg";
import { getTableConfig } from "drizzle-orm/pg-core";
import * as schema from "@webdock/platform-db/schema/plugins";
import { verifySchema } from "../../db/readiness";
import { readInstalledPlugins } from "./store";
/** Live-control startup gate; never initializes or repairs the state store. */
export async function verifyPluginState(
  db: Pick<Pool, "query">,
): Promise<void> {
  const tables = Object.values(schema).map((t) => {
    const c = getTableConfig(t);
    return {
      schema: c.schema!,
      name: c.name,
      columns: c.columns.map((c) => c.name),
    };
  });
  await verifySchema(db, tables);
  const policies = (
    await db.query(
      `SELECT tablename,policyname FROM pg_policies WHERE schemaname='webdock_auth' AND tablename=ANY($1::text[])`,
      [tables.map((t) => t.name)],
    )
  ).rows;
  for (const table of tables)
    for (const role of ["webdock_auth_runtime", "webdock_worker_runtime"])
      for (const suffix of ["read", "insert", "update", "delete"])
        if (
          !policies.some(
            (p) =>
              p.tablename === table.name &&
              p.policyname === `${role}_${suffix}`,
          )
        )
          throw Error("Required plugin isolation policies are unavailable");
  const helpers = (
    await db.query(
      "SELECT to_regprocedure('webdock_rls.plugin_context(text)') IS NOT NULL AND to_regprocedure('webdock_rls.plugin_allows(text,text,text,text)') IS NOT NULL AS present",
    )
  ).rows[0];
  if (helpers?.present !== true)
    throw Error("Required plugin invocation policies are unavailable");
  await readInstalledPlugins(db);
}

export function verifyPluginStartup(db: Pick<Pool, "query">): Promise<void> {
  return withDatabaseScope(
    { kind: "system", purpose: "plugin-public-view" },
    () => verifyPluginState(db),
  );
}
