import { databaseURL } from "../src/db/client";
/** Offline migration step. Run only against an explicitly identified restored target. */
import { readFileSync, statSync } from "node:fs";
import { parseEnv } from "node:util";
import pg from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import {installPlatformPolicies} from "./lib/install-policies";

const file = process.argv[2];
if (!file || statSync(file).mode & 0o077)
  throw Error("A private migration environment file is required");
const env = parseEnv(readFileSync(file, "utf8"));
if (!env.DATABASE_URL) throw Error("DATABASE_URL is required");
const url = databaseURL(env.DATABASE_URL);
const expected = env.MIGRATION_TARGET_DATABASE;
if (
  !expected ||
  url.hostname !== env.MIGRATION_TARGET_HOST ||
  decodeURIComponent(url.pathname.slice(1)) !== expected
)
  throw Error("Explicit target host and database must match");
if (url.hostname.includes("neon.tech"))
  throw Error("Cannot install target policies on the retained source");
if (
  !env.DATABASE_CONTEXT_SECRET ||
  Buffer.byteLength(env.DATABASE_CONTEXT_SECRET) < 32
)
  throw Error("A private context signing key of at least 32 bytes is required");
const local = url.hostname === "127.0.0.1" && expected === "webdock_rls_test";
if (
  !local &&
  (!env.DATABASE_CA_CERT ||
    expected !== "db_103217546714419200" ||
    url.hostname !== "db-103217546630533120.pg.webdock.dev")
)
  throw Error(
    "Only the authorized Webdock target with verified CA is permitted",
  );
const pool = new pg.Pool({
  ...(local
    ? { connectionString: url.href }
    : verifiedPostgresConfig(url.href, env.DATABASE_CA_CERT)),
  max: 1,
});
const db = await pool.connect();
try {
  await db.query("BEGIN");
  await db.query(
    "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='120s'",
  );
  const result = await installPlatformPolicies(db, expected, env.DATABASE_CONTEXT_SECRET);
  await db.query("COMMIT");
  console.log(JSON.stringify(result));
} catch (error) {
  await db.query("ROLLBACK");
  // SQL errors can contain supplied values: print the error code, never signing material.
  if (error instanceof Error && !("code" in error))
    console.error(error.message);
  else {
    console.error(
      "Policy installation failed; transaction rolled back.",
      (error as { code?: string }).code,
    );
    if ((error as { code?: string }).code === "42501")
      console.error((error as Error).message);
  }
  process.exitCode = 1;
} finally {
  db.release();
  await pool.end();
}
