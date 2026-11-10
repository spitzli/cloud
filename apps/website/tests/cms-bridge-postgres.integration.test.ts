import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import test from "node:test";
import { Pool } from "pg";
import { drizzle } from "@payloadcms/db-postgres/drizzle/node-postgres";
import type { MigrateUpArgs } from "@payloadcms/db-postgres";
import {
  up,
  down,
} from "../src/migrations-instance/20261018_180000_cms_bridge.ts";
import { PostgresBridgeTransfers } from "@spitzli/cms-bridge/postgres";
import { projectBridgeUser } from "../../../packages/cms-bridge/src/auth.ts";
import { bridgeRequestDigest } from "@spitzli/cms-bridge/server";

test(
  "website bridge migration and transfers work with restricted PostgreSQL roles and remain portable",
  {
    skip: process.env.RUN_WEBSITE_BRIDGE_POSTGRES !== "1",
    timeout: 60000,
  },
  async () => {
    const docker = (args: string[]) =>
      execFileSync("docker", args, {
        encoding: "utf8",
        stdio: ["pipe", "pipe", "pipe"],
      }).trim();
    const endpoint =
      process.env.DOCKER_HOST ||
      docker(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"]);
    assert.ok(
      endpoint.startsWith("unix://"),
      "A local Docker engine is required",
    );
    const name = `webdock-website-bridge-${randomBytes(6).toString("hex")}`;
    const password = randomBytes(24).toString("hex");
    const pools: Pool[] = [];
    let created = false;
    try {
      docker([
        "run",
        "-d",
        "--name",
        name,
        "-e",
        `POSTGRES_PASSWORD=${password}`,
        "-p",
        "127.0.0.1::5432",
        "postgres:18.6",
      ]);
      created = true;
      const port = Number(docker(["port", name, "5432/tcp"]).split(":").at(-1));
      const pool = (user: string, database = "postgres") => {
        const db = new Pool({
          host: "127.0.0.1",
          port,
          user,
          password,
          database,
          max: 1,
          connectionTimeoutMillis: 1000,
        });
        pools.push(db);
        return db;
      };
      const root = pool("postgres");
      for (let attempt = 0; ; attempt++) {
        try {
          await root.query("SELECT 1");
          break;
        } catch (error) {
          if (attempt === 150) throw error;
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
      const logins = [
        "webdock_runtime",
        "webdock_browser_website",
        "webdock_auth_runtime",
        "webdock_worker_runtime",
        "webdock_admin_runtime",
        "webdock_registry_runtime",
        "webdock_cms_archive_runtime",
        "standalone_owner",
      ];
      for (const role of logins)
        await root.query(
          `CREATE ROLE ${role} LOGIN PASSWORD '${password}' NOSUPERUSER NOBYPASSRLS NOCREATEROLE NOCREATEDB`,
        );
      await root.query(
        "CREATE ROLE webdock_website_owner NOLOGIN NOSUPERUSER NOBYPASSRLS; GRANT webdock_website_owner TO webdock_browser_website",
      );
      await root.query("CREATE DATABASE website_bridge_test");
      const admin = pool("postgres", "website_bridge_test");
      await admin.query(`
      CREATE SCHEMA webdock AUTHORIZATION webdock_website_owner;
      CREATE SCHEMA webdock_auth;
      CREATE TABLE webdock_auth.identity_secret(id text);
      REVOKE ALL ON SCHEMA public,webdock_auth FROM PUBLIC;
      SET ROLE webdock_website_owner;
      CREATE TABLE webdock.users(id serial PRIMARY KEY, auth_subject text UNIQUE, email text UNIQUE NOT NULL, name text, role text NOT NULL);
      ALTER TABLE webdock.users ENABLE ROW LEVEL SECURITY;
      ALTER TABLE webdock.users FORCE ROW LEVEL SECURITY;
      CREATE POLICY website_owner ON webdock.users TO webdock_website_owner USING(true) WITH CHECK(true);
      CREATE POLICY website_runtime ON webdock.users TO webdock_runtime USING(true) WITH CHECK(true);
      GRANT USAGE ON SCHEMA webdock TO webdock_runtime;
      GRANT SELECT,INSERT,UPDATE,DELETE ON webdock.users TO webdock_runtime;
      GRANT USAGE,SELECT ON SEQUENCE webdock.users_id_seq TO webdock_runtime;
      RESET ROLE;
    `);
      const migrate = async (db: Pool, direction = up) => {
        const client = await db.connect();
        try {
          await client.query("BEGIN");
          await direction({ db: drizzle(client) } as unknown as MigrateUpArgs);
          await client.query("COMMIT");
        } catch (error) {
          await client.query("ROLLBACK");
          throw error;
        } finally {
          client.release();
        }
      };
      const runtime = pool("webdock_runtime", "website_bridge_test");
      await assert.rejects(migrate(runtime));
      const owner = pool("webdock_browser_website", "website_bridge_test");
      await migrate(owner);
      const tables = (
        await admin.query(`SELECT c.relname,c.relrowsecurity,c.relforcerowsecurity,r.rolname AS owner
      FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_roles r ON r.oid=c.relowner
      WHERE n.nspname='webdock' AND c.relkind='r' ORDER BY c.relname`)
      ).rows;
      assert.equal(tables.length, 3);
      for (const table of tables) {
        assert.equal(table.relrowsecurity, true, table.relname);
        assert.equal(table.relforcerowsecurity, true, table.relname);
        assert.equal(table.owner, "webdock_website_owner", table.relname);
      }
      assert.deepEqual(
        (
          await runtime.query(
            "SELECT rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
          )
        ).rows,
        [{ rolsuper: false, rolbypassrls: false }],
      );
      await assert.rejects(
        runtime.query("SELECT * FROM webdock_auth.identity_secret"),
        { code: "42501" },
      );
      for (const role of logins.filter(
        (role) =>
          !["webdock_runtime", "webdock_browser_website"].includes(role),
      )) {
        const denied = pool(role, "website_bridge_test");
        await assert.rejects(
          denied.query("SELECT * FROM webdock.cms_bridge_transfers"),
          { code: "42501" },
        );
        await assert.rejects(
          denied.query("SELECT * FROM webdock.cms_bridge_transfer_chunks"),
          { code: "42501" },
        );
      }
      const scope = {
        instance: "website",
        project: "project",
        subject: "subject-1",
        role: "editor" as const,
        grantEpoch: "1",
      };
      // Exercise the real runtime grants and forced user policy used by Payload's subject projection.
      const payload = {
        find: async ({ where }: { where: { authSubject: { equals: string } } }) => ({
          docs: (
            await runtime.query(
              'SELECT id,auth_subject AS "authSubject",email,name,role,disabled FROM webdock.users WHERE auth_subject=$1',
              [where.authSubject.equals],
            )
          ).rows,
        }),
        create: async ({ data }: { data: Record<string, unknown> }) => {
          await runtime.query(
            "INSERT INTO webdock.users(auth_subject,email,name,role,disabled) VALUES($1,$2,$3,$4,$5)",
            [data.authSubject, data.email, data.name, data.role, data.disabled],
          );
        },
      };
      assert.equal(
        (await projectBridgeUser(payload as never, scope)).role,
        "editor",
      );
      await runtime.query("UPDATE webdock.users SET role='reader'");
      assert.equal(
        (await projectBridgeUser(payload as never, scope)).role,
        "reader",
      );
      await runtime.query("UPDATE webdock.users SET disabled=true");
      await assert.rejects(
        projectBridgeUser(payload as never, scope),
        /disabled/,
      );
      const transfers = new PostgresBridgeTransfers(runtime);
      const bytes = Buffer.from(
        JSON.stringify({
          method: "POST",
          path: "/api/cms",
          contentType: "application/json",
          bodyBase64: Buffer.from("{}").toString("base64"),
        }),
      );
      const { transfer } = await transfers.begin(scope, {
        op: "begin",
        size: bytes.length,
        sha256: bridgeRequestDigest(bytes),
        kind: "json",
      });
      await assert.rejects(
        transfers.append(
          { ...scope, subject: "subject-2" },
          { op: "append", transfer, offset: 0, data: bytes.toString("base64") },
        ),
        /unavailable/,
      );
      await transfers.append(scope, {
        op: "append",
        transfer,
        offset: 0,
        data: bytes.toString("base64"),
      });
      let calls = 0;
      const execute = async () => {
        calls++;
        return Response.json({ saved: true });
      };
      await transfers.commit(scope, transfer, "https://site.invalid", execute);
      await transfers.commit(scope, transfer, "https://site.invalid", execute);
      assert.equal(calls, 1);
      assert.equal((await transfers.read(scope, transfer, 0)).done, true);
      assert.equal(
        (
          await owner.query(
            "SELECT count(*)::int AS count FROM webdock.cms_bridge_transfers",
          )
        ).rows[0].count,
        1,
      );
      await owner.query("DELETE FROM webdock.cms_bridge_transfers");
      // Existing FORCE RLS still protects the owner if its explicit policy is absent.
      await owner.query(
        "DROP POLICY website_owner ON webdock.cms_bridge_transfers; SET ROLE webdock_website_owner",
      );
      await assert.rejects(
        owner.query(
          "INSERT INTO webdock.cms_bridge_transfers(id,instance,project,subject,role,grant_epoch,expires,size,sha256,kind,state) VALUES(gen_random_uuid(),'i','p','s','editor','1',1,0,'','json','uploading')",
        ),
        { code: "42501" },
      );
      await owner.query("RESET ROLE");
      await migrate(owner, down);
      assert.equal(
        (
          await owner.query(
            "SELECT to_regclass('webdock.cms_bridge_transfers') AS table",
          )
        ).rows[0].table,
        null,
      );
      await root.query(
        "CREATE DATABASE website_bridge_portable OWNER standalone_owner",
      );
      const standalone = pool("standalone_owner", "website_bridge_portable");
      await standalone.query(
        "CREATE SCHEMA webdock; CREATE TABLE webdock.users(id integer PRIMARY KEY)",
      );
      await migrate(standalone);
      assert.equal(
        (
          await standalone.query(
            "SELECT has_table_privilege('webdock_runtime','webdock.cms_bridge_transfers','SELECT') AS granted",
          )
        ).rows[0].granted,
        false,
      );
      await standalone.query(
        "INSERT INTO webdock.users(id,disabled) VALUES(1,false)",
      );
      await migrate(standalone, down);
    } finally {
      await Promise.all(pools.map((pool) => pool.end()));
      if (created) docker(["rm", "-f", "-v", name]);
    }
  },
);
