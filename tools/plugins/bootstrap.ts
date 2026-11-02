/** Explicit offline import. No API startup side effect and no credential defaults. */
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { migrationConfig } from "../migrations/config";
import { initializePluginState } from "../../legacy/control-plane/src/modules/plugins/bootstrap";
const config = migrationConfig(process.env, process.argv[2]);
const mode = process.argv[3];
if (mode !== "fresh" && mode !== "legacy")
  throw Error("Usage: bootstrap.ts exact-host:port/database fresh|legacy");
const selection =
  process.env.WEBDOCK_MODULES === undefined
    ? undefined
    : JSON.parse(process.env.WEBDOCK_MODULES);
const pool = new Pool({
  ...(config.local
    ? { connectionString: config.url.href }
    : verifiedPostgresConfig(config.url.href, config.ca)),
  max: 1,
});
try {
  const row = (
    await pool.query(
      "SELECT current_database() AS database,session_user AS role",
    )
  ).rows[0];
  if (row.database !== decodeURIComponent(config.url.pathname.slice(1)))
    throw Error("Database target mismatch");
  if (!config.local && row.role !== "webdock_offline_migrate")
    throw Error("Offline migration role required");
  await initializePluginState(pool, selection, mode);
  console.log(
    "Installation state initialized or preserved; no runtime enabled by this command.",
  );
} finally {
  await pool.end();
}
