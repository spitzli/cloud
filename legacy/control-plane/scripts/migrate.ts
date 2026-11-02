import { databaseURL } from "../src/db/client";
import {applyApiMigrations} from "./lib/apply-migrations";
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
const raw = process.env.DATABASE_URL_UNPOOLED;
if (!raw) throw Error("Explicit offline migration URL required");
const url = databaseURL(raw),
  target = `${url.hostname}:${url.port || 5432}${url.pathname}`;
if (process.argv[2] !== target)
  throw Error("Pass the exact migration host:port/database");
const local =
  url.hostname === "127.0.0.1" &&
  ["/webdock_api_test", "/webdock_rls_test"].includes(url.pathname);
if (
  !local &&
  ([
    "postgres",
    "webdock_auth_runtime",
    "webdock_auth_bootstrap",
    "webdock_worker_runtime",
    "webdock_admin_runtime",
    "webdock_registry_runtime",
  ].includes(url.username) ||
    !process.env.DATABASE_CA_CERT)
)
  throw Error("Use a dedicated offline migrator and verified CA");
const pool = new Pool({
  ...(local
    ? { connectionString: raw }
    : verifiedPostgresConfig(raw, process.env.DATABASE_CA_CERT)),
  max: 1,
});
const db = await pool.connect();
try {
  if (
    (await db.query("SELECT current_database() AS name")).rows[0].name !==
    decodeURIComponent(url.pathname.slice(1))
  )
    throw Error("Connected migration database differs from explicit target");
  await db.query("BEGIN");
  await db.query(
    "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='120s'",
  );
  await db.query(
    "SELECT pg_advisory_xact_lock(hashtext('webdock-api-migration'))",
  );
  await applyApiMigrations(db);
  await db.query("COMMIT");
  console.log("API migrations applied to explicit target.");
} catch (error) {
  await db.query("ROLLBACK");
  throw error;
} finally {
  db.release();
  await pool.end();
}
