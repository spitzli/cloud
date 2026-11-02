import { afterAll, expect, test } from "bun:test";
import { Pool } from "pg";
import { sql } from "drizzle-orm";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createDatabases } from "../src/db/client";
import { assertTestDatabase } from "./helpers/database";
const url = process.env.API_TEST_DATABASE_URL;
const integration = url ? test : test.skip;
let cleanup: (() => Promise<void>) | undefined;
afterAll(async () => {
  await cleanup?.();
});
test("test harness rejects remote and ambiguous database URLs", () => {
  for (const candidate of [
    "postgres://u:p@remote/db",
    "postgres://u:p@127.0.0.1/postgres",
    "postgres://u:p@127.0.0.1/webdock_api_test?host=remote",
  ])
    expect(() => assertTestDatabase(candidate)).toThrow();
});
integration(
  "Drizzle retains transaction-local signed scope and rolls back on pooled reuse",
  async () => {
    assertTestDatabase(url!);
    const pools = createDatabases({
      environment: "test",
      businessURL: url!,
      authURL: url!,
      workerURL: url!,
      signingSecret: "test-signing-key-that-is-at-least-32-bytes",
      max: 1,
    });
    cleanup = pools.close;
    const get = async () =>
      (
        await pools.orm.execute<{ context: string | null }>(
          sql`select current_setting('webdock.rls_context', true) as context`,
        )
      ).rows[0].context;
    const one = await withDatabaseScope(
      { kind: "session", subject: "tenant-a", sessionID: "session-a" },
      async () => {
        const result = await get();
        await expect(
          pools.orm.transaction(async (tx) => {
            await tx.execute(sql`select 1`);
            throw Error("rollback");
          }),
        ).rejects.toThrow("rollback");
        return result;
      },
    );
    expect(JSON.parse(one!).scope.subject).toBe("tenant-a");
    const two = await withDatabaseScope(
      { kind: "session", subject: "tenant-b", sessionID: "session-b" },
      get,
    );
    expect(JSON.parse(two!).scope.subject).toBe("tenant-b");
    expect(await get()).toBe("");
    const worker = await withDatabaseScope(
      { kind: "system", purpose: "fixture" },
      async () =>
        (
          await pools.workerOrm.execute<{ context: string }>(
            sql`select current_setting('webdock.rls_context',true) as context`,
          )
        ).rows[0].context,
    );
    expect(JSON.parse(worker).aud).toBe("webdock_worker_runtime");
  },
);
test("production never silently falls back to unsigned business access", () => {
  expect(() =>
    createDatabases({
      environment: "production",
      businessURL: "postgres://user:pass@db.invalid/db",
      authURL: "",
      workerURL: "",
      max: 1,
    }),
  ).toThrow();
});
