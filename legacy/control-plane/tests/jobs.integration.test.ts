import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { Pool } from "pg";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";
import { schedule, claim, complete } from "../src/jobs";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(fixtureFile ? test : test.skip)(
  "durable jobs claim once, reclaim expired leases and reject stale completion",
  async () => {
    const raw = (process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL)!;
    const url = new URL(raw);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated RLS fixture required");
    const fixture = JSON.parse(await readFile(fixtureFile!, "utf8"));
    const owner = new Pool({ connectionString: raw });
    const worker = new ScopedPool(
      { connectionString: fixture.roles.webdock_worker_runtime, max: 2 },
      { secret: fixture.contextSecret, audience: "webdock_worker_runtime" },
    );
    const run = <T>(fn: () => T) =>
      withDatabaseScope({ kind: "system", purpose: "api-scheduler" }, fn);
    try {
      const slot = new Date("2026-01-01T00:00:00Z");
      await run(() => schedule(worker, "storage.refresh", slot));
      await run(() => schedule(worker, "storage.refresh", slot));
      const claims = await Promise.all([
        run(() => claim(worker)),
        run(() => claim(worker)),
      ]);
      expect(claims.filter(Boolean)).toHaveLength(1);
      const first = claims.find(Boolean)!;
      await owner.query(
        "UPDATE webdock_auth.api_job SET lease_until=now()-interval '1 second' WHERE id=$1",
        [first.id],
      );
      const second = (await run(() => claim(worker)))!;
      expect(second.id).toBe(first.id);
      expect(second.claimToken).not.toBe(first.claimToken);
      expect(await run(() => complete(worker, first))).toBe(false);
      expect(await run(() => complete(worker, second))).toBe(true);
      expect(await run(() => claim(worker))).toBeNull();
    } finally {
      await owner.query(
        "DELETE FROM webdock_auth.api_job WHERE name='storage.refresh' AND slot='2026-01-01T00:00:00Z'",
      );
      await worker.end();
      await owner.end();
    }
  },
);
