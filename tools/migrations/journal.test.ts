import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { createHash } from "node:crypto";
import type { PoolClient } from "pg";
import { applyApiMigrations } from "../../legacy/control-plane/scripts/lib/apply-migrations";
test("migration journal refuses removed or changed history and keeps legacy equal-number files independent", async () => {
  const dir = await mkdtemp(join(tmpdir(), "webdock-journal-"));
  try {
    await writeFile(join(dir, "0022_first.sql"), "SELECT 1");
    await writeFile(join(dir, "0022_second.sql"), "SELECT 2");
    let applied: { id: string; checksum: string }[] = [];
    const executed: string[] = [];
    const db = {
      query: async (sql: string, values?: string[]) => {
        if (sql.startsWith("SELECT id,checksum")) return { rows: applied };
        if (sql.startsWith("INSERT INTO webdock_api_migrations"))
          applied.push({ id: values![0], checksum: values![1] });
        if (sql === "SELECT 1" || sql === "SELECT 2") executed.push(sql);
        return { rows: [] };
      },
    } as unknown as PoolClient;
    const url = pathToFileURL(dir + "/");
    await applyApiMigrations(db, url);
    assert.deepEqual(executed, ["SELECT 1", "SELECT 2"]);
    assert.equal(applied.length, 2);
    await applyApiMigrations(db, url);
    assert.equal(executed.length, 2);
    applied.push({ id: "0030_missing.sql", checksum: "unknown" });
    await assert.rejects(
      applyApiMigrations(db, url),
      /missing from this release/,
    );
    assert.equal(executed.length, 2);
    applied.pop();
    await writeFile(join(dir, "0022_first.sql"), "SELECT 3");
    await assert.rejects(applyApiMigrations(db, url), /checksum changed/);
    assert.equal(executed.length, 2);
    assert.equal(
      applied[0].checksum,
      createHash("sha256").update("SELECT 1").digest("hex"),
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
