import { readFile, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import type { PoolClient } from "pg";
/** Caller holds the offline transaction/advisory lock. */
export async function applyApiMigrations(
  db: PoolClient,
  directory = new URL("../../drizzle/", import.meta.url),
) {
  await db.query(
    "CREATE SCHEMA IF NOT EXISTS webdock_api_migrations; REVOKE ALL ON SCHEMA webdock_api_migrations FROM PUBLIC; CREATE TABLE IF NOT EXISTS webdock_api_migrations.journal(id text PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())",
  );
  const ids = (await readdir(directory))
    .filter((name) => /^\d{4}_[a-z0-9_]+\.sql$/.test(name))
    .sort();
  const applied = (
    await db.query<{ id: string; checksum: string }>(
      "SELECT id,checksum FROM webdock_api_migrations.journal ORDER BY id",
    )
  ).rows;
  if (applied.some((row) => !ids.includes(row.id)))
    throw Error("Applied migration is missing from this release");
  const previous = new Map(applied.map((row) => [row.id, row.checksum]));
  for (const id of ids) {
    const sql = await readFile(new URL(id, directory), "utf8"),
      checksum = createHash("sha256").update(sql).digest("hex");
    const old = previous.get(id);
    if (old) {
      if (old !== checksum)
        throw Error("Applied migration checksum changed: " + id);
      continue;
    }
    await db.query(sql);
    await db.query(
      "INSERT INTO webdock_api_migrations.journal(id,checksum) VALUES($1,$2)",
      [id, checksum],
    );
  }
}
