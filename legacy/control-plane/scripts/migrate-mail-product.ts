// Offline only. Runtime URL is never promoted to migration authority implicitly.
import { createHash } from "node:crypto";
import { Pool } from "pg";
import { mailProductMigrations } from "../src/modules/mail/product/schema";
const raw = process.env.DATABASE_URL_UNPOOLED;
if (!raw) throw Error("DATABASE_URL_UNPOOLED is required");
const url = new URL(raw);
const target = `${url.hostname}:${url.port || "5432"}${url.pathname}`;
if (process.argv[2] !== target)
  throw Error(
    "Pass the exact host:port/database as the explicit migration target",
  );
const role = process.argv[3];
if (!role || !/^[a-z][a-z0-9_]{0,62}$/.test(role))
  throw Error("An explicit runtime role is required");
const pool = new Pool({ connectionString: raw, max: 1 });
const db = await pool.connect();
try {
  await db.query("BEGIN");
  await db.query(
    "SELECT pg_advisory_xact_lock(hashtext('webdock-mail-product-migration'))",
  );
  await db.query(
    `CREATE TABLE IF NOT EXISTS webdock_auth.mail_product_migration(id text PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())`,
  );
  const reports = [];
  for (const [id, sql] of mailProductMigrations) {
    const checksum = createHash("sha256").update(sql).digest("hex");
    const old = (
      await db.query(
        "SELECT checksum FROM webdock_auth.mail_product_migration WHERE id=$1",
        [id],
      )
    ).rows[0];
    if (old && old.checksum !== checksum)
      throw Error("Applied migration checksum differs; add a new migration");
    if (!old) {
      await db.query(sql);
      await db.query(
        "INSERT INTO webdock_auth.mail_product_migration(id,checksum) VALUES($1,$2)",
        [id, checksum],
      );
    }
    for (const match of sql.matchAll(
      /CREATE TABLE IF NOT EXISTS webdock_auth\.(\w+)/g,
    ))
      await db.query(
        `GRANT SELECT,INSERT,UPDATE,DELETE ON webdock_auth."${match[1]}" TO "${role}"`,
      );
    reports.push({ id, checksum, applied: !old });
  }
  await db.query("COMMIT");
  console.log(
    JSON.stringify({
      target,
      ...reports[0],
      migrations: reports,
      runtimeRole: role,
    }),
  );
} catch (error) {
  await db.query("ROLLBACK");
  throw error;
} finally {
  db.release();
  await pool.end();
}
