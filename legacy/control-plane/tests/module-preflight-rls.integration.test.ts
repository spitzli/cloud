import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { Pool } from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createDatabases } from "../src/db/client";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(fixtureFile ? test : test.skip)(
  "installation preflight sees all customer storage and cannot mutate it",
  async () => {
    const url = new URL(
      (process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL)!,
    );
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Only isolated RLS fixture is allowed");
    const fixture = JSON.parse(await readFile(fixtureFile!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const pools = createDatabases({
      environment: "test",
      businessURL: fixture.roles.webdock_auth_runtime,
      authURL: fixture.roles.webdock_auth_bootstrap,
      workerURL: fixture.roles.webdock_worker_runtime,
      signingSecret: fixture.contextSecret,
      max: 1,
    });
    const ids = [randomUUID(), randomUUID()];
    try {
      const customers = await owner.query<{ customer_id: string }>(
        "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Preflight A'),(webdock_auth.next_snowflake(),'Preflight B') RETURNING id AS customer_id",
      );
      expect(customers.rows.length).toBe(2);
      for (let i = 0; i < 2; i++)
        await owner.query(
          "INSERT INTO webdock_auth.tenant_storage(id,customer_id,label,environment,store_id,prefix,encrypted_token) VALUES($1,$2,'preflight fixture','preview',$1,'','fixture')",
          [ids[i], customers.rows[i]!.customer_id],
        );
      const rows = await withDatabaseScope(
        { kind: "system", purpose: "installation-preflight" },
        () =>
          pools.worker.query(
            "SELECT id FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
            [ids],
          ),
      );
      expect(rows.rows.length).toBe(2);
      const unrelated = await withDatabaseScope(
        { kind: "system", purpose: "api-scheduler" },
        () =>
          pools.worker.query(
            "SELECT id FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
            [ids],
          ),
      );
      expect(unrelated.rows).toEqual([]);
      // Row visibility from SELECT must not confer write authority.
      try {
        await withDatabaseScope(
          { kind: "system", purpose: "installation-preflight" },
          () =>
            pools.worker.query(
              "UPDATE webdock_auth.tenant_storage SET label='mutated' WHERE id=ANY($1)",
              [ids],
            ),
        );
      } catch (error) {
        expect((error as { code: string }).code).toBe("42501");
      }
      expect(
        (
          await owner.query(
            "SELECT label FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
            [ids],
          )
        ).rows.every((row) => row.label === "preflight fixture"),
      ).toBe(true);
    } finally {
      await owner.query(
        "DELETE FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
        [ids],
      );
      await pools.close();
      await owner.end();
    }
  },
);

(fixtureFile ? test : test.skip)(
  "database shutdown counts queued setup configuration before a database exists",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Only isolated RLS fixture is allowed");
    const { modulePreflightChecks } = await import("../src/db/module-preflight-tables");
    const predicate = modulePreflightChecks.databases.find(([table]) => table === "webdock_auth.hosting_setup")?.[1];
    expect(predicate).toBeDefined();
    const owner = new Pool({ connectionString: url.href, max: 1 });
    try {
      const result = await owner.query(
        `SELECT configuration #>> '{database,mode}' AS mode FROM (VALUES ('{"database":{"mode":"new"}}'::jsonb), ('{"database":{"mode":"existing"}}'::jsonb), ('{"database":{"mode":"none"}}'::jsonb), ('{}'::jsonb)) AS setup(configuration) WHERE ${predicate}`,
      );
      expect(result.rows.map(row => row.mode)).toEqual(["new", "existing"]);
    } finally { await owner.end(); }
  },
);
