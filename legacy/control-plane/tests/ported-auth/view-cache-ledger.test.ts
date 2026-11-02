import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Pool } from "pg";
import { ViewCache, configureViewCache, readCachedView } from "../../src/cache";
import { assertTestDatabase } from "../helpers/database";
assertTestDatabase(process.env.DATABASE_URL!);
const pool = new Pool({ connectionString: process.env.DATABASE_URL, max: 5 });
const domains = ["webdock_cache_fixture.alpha", "webdock_cache_fixture.beta"];
const version = async (source = pool, names = domains) =>
  (
    await source.query("SELECT webdock_cache.version($1::text[]) AS version", [
      names,
    ])
  ).rows[0].version as string;
test.before(async () => {
  await pool.query(
    await readFile(
      new URL("../../drizzle/0036_authorized_view_cache.sql", import.meta.url),
      "utf8",
    ),
  );
  await pool.query(
    "CREATE SCHEMA webdock_cache_fixture; CREATE TABLE webdock_cache_fixture.alpha(id int PRIMARY KEY,value int NOT NULL); CREATE TABLE webdock_cache_fixture.beta(id int PRIMARY KEY,value int NOT NULL)",
  );
  for (const domain of domains) {
    await pool.query("INSERT INTO webdock_cache.domains(name) VALUES($1)", [
      domain,
    ]);
    for (const [op, reference] of [
      ["INSERT", "NEW"],
      ["UPDATE", "NEW"],
      ["DELETE", "OLD"],
    ])
      await pool.query(
        `CREATE TRIGGER invalidate_${op} AFTER ${op} ON ${domain} REFERENCING ${reference} TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION webdock_cache.changed()`,
      );
    await pool.query(
      `CREATE TRIGGER invalidate_truncate AFTER TRUNCATE ON ${domain} FOR EACH STATEMENT EXECUTE FUNCTION webdock_cache.changed()`,
    );
    await pool.query(`INSERT INTO ${domain} VALUES(1,0),(2,0)`);
  }
  await pool.query(
    "GRANT USAGE ON SCHEMA webdock_cache_fixture TO webdock_worker_runtime; GRANT SELECT,UPDATE ON ALL TABLES IN SCHEMA webdock_cache_fixture TO webdock_worker_runtime",
  );
});
test.after(async () => {
  await pool.query("DELETE FROM webdock_cache.events WHERE domain=ANY($1)", [
    domains,
  ]);
  await pool.query("DELETE FROM webdock_cache.domains WHERE name=ANY($1)", [
    domains,
  ]);
  await pool.query("DROP SCHEMA webdock_cache_fixture CASCADE");
  await pool.end();
});
test("source writes invalidate only when committed; empty writes, rollback and read-only access retain versions", async () => {
  const before = await version(),
    c = await pool.connect();
  try {
    await c.query("BEGIN");
    await c.query(
      "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=1",
    );
    assert.equal(await version(), before);
    await c.query("ROLLBACK");
    assert.equal(await version(), before);
    await pool.query(
      "UPDATE webdock_cache_fixture.alpha SET value=1 WHERE false",
    );
    assert.equal(await version(), before);
    await c.query("BEGIN");
    await c.query(
      "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=1",
    );
    await c.query("COMMIT");
    assert.notEqual(await version(), before);
    await c.query("SET ROLE webdock_auth_runtime");
    assert.ok(await version(c as never));
    for (const sql of [
      "SELECT * FROM webdock_cache.events",
      "INSERT INTO webdock_cache.events(domain,transaction_id) VALUES('forged','1')",
      "UPDATE webdock_cache.domains SET epoch=gen_random_uuid()",
      "SELECT webdock_cache.prune()",
    ]) {
      await assert.rejects(c.query(sql));
    }
    await c.query("RESET ROLE");
    await assert.rejects(version(pool, ["unregistered.table"]));
    await assert.rejects(version(pool, []));
  } finally {
    await c.query("ROLLBACK");
    await c.query("RESET ROLE");
    c.release();
  }
});
test("out-of-order commits, worker writes, deletes and truncation advance the version", async () => {
  const a = await pool.connect(),
    b = await pool.connect();
  try {
    await a.query("BEGIN");
    await a.query(
      "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=1",
    );
    await b.query("SET ROLE webdock_worker_runtime");
    await b.query(
      "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=2",
    );
    await b.query("RESET ROLE");
    const laterIDCommitted = await version();
    await a.query("COMMIT");
    assert.notEqual(
      await version(),
      laterIDCommitted,
      "COUNT catches the lower event ID committing later",
    );
    let current = await version();
    await pool.query("DELETE FROM webdock_cache_fixture.alpha WHERE id=2");
    assert.notEqual(await version(), current);
    current = await version();
    await pool.query("TRUNCATE webdock_cache_fixture.beta");
    assert.notEqual(await version(), current);
  } finally {
    await a.query("ROLLBACK");
    await b.query("RESET ROLE");
    a.release();
    b.release();
  }
});
test("opposite table order on independent rows does not acquire shared revision-counter locks", async () => {
  await pool.query(
    "INSERT INTO webdock_cache_fixture.alpha VALUES(2,0) ON CONFLICT DO NOTHING; INSERT INTO webdock_cache_fixture.beta VALUES(1,0),(2,0)",
  );
  const a = await pool.connect(),
    b = await pool.connect();
  try {
    await a.query("BEGIN; SET LOCAL lock_timeout='500ms'");
    await b.query("BEGIN; SET LOCAL lock_timeout='500ms'");
    await a.query(
      "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=1",
    );
    await b.query(
      "UPDATE webdock_cache_fixture.beta SET value=value+1 WHERE id=2",
    );
    await Promise.all([
      a.query("UPDATE webdock_cache_fixture.beta SET value=value+1 WHERE id=1"),
      b.query(
        "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=2",
      ),
    ]);
    await Promise.all([a.query("COMMIT"), b.query("COMMIT")]);
  } finally {
    await a.query("ROLLBACK");
    await b.query("ROLLBACK");
    a.release();
    b.release();
  }
});
test("pruning rotates epochs atomically and late commits still invalidate", async () => {
  const a = await pool.connect(),
    b = await pool.connect();
  try {
    const before = await version();
    await pool.query(
      "UPDATE webdock_cache.events SET created_at=now()-interval '10 minutes' WHERE domain=ANY($1)",
      [domains],
    );
    await a.query("BEGIN");
    await a.query(
      "UPDATE webdock_cache_fixture.alpha SET value=value+1 WHERE id=1",
    );
    await b.query("BEGIN");
    await b.query(
      "SELECT pg_advisory_xact_lock(hashtextextended('webdock-cache-prune',0))",
    );
    assert.equal(
      (await pool.query("SELECT webdock_cache.prune() AS count")).rows[0].count,
      0,
    );
    await b.query("ROLLBACK");
    await b.query("SET ROLE webdock_auth_bootstrap");
    await b.query("SELECT webdock_cache.prune()");
    await b.query("RESET ROLE");
    const pruned = await version();
    assert.notEqual(pruned, before);
    await a.query("COMMIT");
    assert.notEqual(await version(), pruned);
  } finally {
    await a.query("ROLLBACK");
    await b.query("ROLLBACK");
    await b.query("RESET ROLE");
    a.release();
    b.release();
  }
});

test("write-read-rollback never fills or shares a cache; unrelated own writes do not disable other domains", async () => {
  const store = new Map<string, string>();
  let gets = 0,
    sets = 0;
  configureViewCache(
    new ViewCache({
      get: async (key) => {
        gets++;
        return store.get(key) ?? null;
      },
      set: async (key, value) => {
        sets++;
        store.set(key, value);
      },
    }),
    { environment: "ledger-test" },
  );
  const a = await pool.connect();
  const options = {
    kind: "operations" as const,
    scope: { subject: "fixture", sessionID: "fixture" },
    query: { id: 1 },
  };
  const read = async (source: Pick<Pool, "query"> = pool) =>
    readCachedView(
      options,
      () => version(source as never, [domains[0]]),
      async () =>
        (
          await source.query(
            "SELECT value FROM webdock_cache_fixture.alpha WHERE id=1",
          )
        ).rows[0],
    );
  try {
    const committed = await read();
    assert.equal(sets, 1);
    await a.query("BEGIN");
    await a.query(
      "UPDATE webdock_cache_fixture.alpha SET value=999 WHERE id=1",
    );
    assert.equal(await version(a as never, [domains[0]]), null);
    assert.equal((await read(a)).value, 999);
    assert.equal(gets, 1);
    assert.equal(sets, 1);
    await a.query("ROLLBACK");
    assert.deepEqual(await read(), committed);
    await a.query("BEGIN");
    await a.query(
      "UPDATE webdock_cache_fixture.beta SET value=value+1 WHERE id=1",
    );
    assert.ok(await version(a as never, [domains[0]]));
    assert.equal(await version(a as never, domains), null);
    assert.deepEqual(await read(a), committed);
    assert.equal(sets, 1);
    await a.query("ROLLBACK");
  } finally {
    configureViewCache(undefined, { environment: "test" });
    await a.query("ROLLBACK");
    a.release();
  }
});
test("rolled-back pruning leaves the same generation and committed pruning creates a fresh one", async () => {
  await pool.query(
    "UPDATE webdock_cache.events SET created_at=now()-interval '10 minutes' WHERE domain=ANY($1)",
    [domains],
  );
  const before = await version(),
    a = await pool.connect();
  try {
    await a.query("BEGIN");
    await a.query("SELECT webdock_cache.prune()");
    const provisional = await version(a as never);
    assert.notEqual(provisional, before);
    assert.equal(await version(), before);
    await a.query("ROLLBACK");
    assert.equal(await version(), before);
    await pool.query("SELECT webdock_cache.prune()");
    const committed = await version();
    assert.notEqual(committed, before);
    assert.notEqual(committed, provisional);
  } finally {
    await a.query("ROLLBACK");
    a.release();
  }
});
