type Reader = {
  query(sql: string, values?: unknown[]): Promise<{rows: Record<string, unknown>[]}>
};
/** Tables and predicates come only from the trusted installation catalog. */
export async function assertModuleCanDisable(db: Reader, module: string, checks: readonly (readonly [string, string])[]) {
  for (const [table, predicate] of checks) {
    const relation = (await db.query("SELECT to_regclass($1)::text AS relation", [table])).rows[0];
    if (!relation || !Object.hasOwn(relation, "relation"))
      throw Error(`Module ${module} preflight did not return a schema result`);
    // A fresh core installation has no product tables. Existing tables still require the full check.
    if (relation.relation === null) continue;
    const [schema, name] = table.split(".");
    const policy = await db.query(
      "SELECT has_table_privilege(session_user,format('%I.%I',$1::text,$2::text),'SELECT') AND EXISTS(SELECT 1 FROM pg_policies WHERE schemaname=$1 AND tablename=$2 AND cmd='SELECT' AND 'webdock_worker_runtime'=ANY(roles) AND qual LIKE '%installation-preflight%') AS present",
      [schema, name],
    );
    if (policy.rows[0]?.present !== true)
      throw Error(`Module ${module} shutdown requires the installation preflight RLS migration`);
    const result = await db.query(`SELECT EXISTS(SELECT 1 FROM ${table} WHERE ${predicate}) AS retained`);
    if (result.rows[0]?.retained !== false)
      throw Error(`Module ${module} retains resources or operations; drain and resolve them before disabling`);
  }
}
