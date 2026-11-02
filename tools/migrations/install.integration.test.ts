import { test } from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { execFileSync, spawnSync, spawn } from "node:child_process";
import { Pool } from "pg";
import { runtimeRoles } from "./config.ts";
import { fileURLToPath } from "node:url";
import { readFile } from "node:fs/promises";
const root = fileURLToPath(new URL("../../", import.meta.url));
test(
  "empty installation, retained-data upgrade and signing-key refusal use restricted roles",
  { skip: process.env.WEBDOCK_TEST_INSTALL !== "true", timeout: 120000 },
  async () => {
    const endpoint = execFileSync(
      "docker",
      ["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"],
      { encoding: "utf8" },
    ).trim();
    assert.ok(endpoint.startsWith("unix://"));
    const env: NodeJS.ProcessEnv = { ...process.env, DOCKER_HOST: endpoint };
    delete env.DOCKER_CONTEXT;
    const name = "webdock-install-test-" + randomBytes(6).toString("hex"),
      password = randomBytes(32).toString("hex");
    const docker = (args: string[]) =>
      execFileSync("docker", args, { encoding: "utf8", env }).trim();
    docker([
      "run",
      "-d",
      "--name",
      name,
      "--label",
      "webdock.install-fixture=" + name,
      "-e",
      "POSTGRES_PASSWORD=" + password,
      "-e",
      "POSTGRES_DB=webdock_install_test",
      "-p",
      "127.0.0.1::5432",
      "postgres:18-alpine",
    ]);
    let owner: Pool | undefined;
    try {
      const port = docker(["port", name, "5432/tcp"]).split(":").at(-1)!;
      const url = new URL(
        `postgresql://postgres:${password}@127.0.0.1:${port}/webdock_install_test`,
      );
      owner = new Pool({ connectionString: url.href, max: 1 });
      for (let n = 0; ; n++) {
        try {
          await owner.query("SELECT 1");
          break;
        } catch {
          if (n >= 60) throw Error("Fixture startup failed");
          await new Promise((r) => setTimeout(r, 250));
        }
      }
      const passwords = Object.fromEntries(
        [...runtimeRoles, "webdock_offline_migrate"].map((role) => [
          role,
          randomBytes(32).toString("hex"),
        ]),
      );
      const base = {
        PATH: process.env.PATH!,
        HOME: process.env.HOME!,
        DATABASE_URL_UNPOOLED: url.href,
        DATABASE_CONTEXT_SECRET: randomBytes(48).toString("hex"),
      };
      const target = `127.0.0.1:${port}/webdock_install_test`;
      const run = (values: Record<string, string>) =>
        spawnSync(
          process.execPath,
          ["--no-env-file", "tools/migrations/install.ts", target],
          { cwd: root, encoding: "utf8", env: values },
        );
      const fresh = run({
        ...base,
        WEBDOCK_BOOTSTRAP_EMPTY_DATABASE: "true",
        MIGRATION_ROLE_PASSWORDS: JSON.stringify(passwords),
      });
      assert.equal(fresh.status, 0, fresh.stdout + fresh.stderr);
      const roleURL = (role: string) => {
        const copy = new URL(url);
        copy.username = role;
        copy.password = passwords[role];
        return copy.href;
      };
      const minimal = spawnSync(
        process.execPath,
        [
          "--no-env-file",
          "-e",
          `const {verifyModuleShutdown}=await import('./legacy/control-plane/src/module-preflight');const {databasePools}=await import('./legacy/control-plane/src/db/runtime');try{await verifyModuleShutdown()}finally{await databasePools.close()}`,
        ],
        {
          cwd: root,
          encoding: "utf8",
          env: {
            PATH: process.env.PATH!,
            HOME: process.env.HOME!,
            NODE_ENV: "development",
            DATABASE_URL: roleURL("webdock_auth_runtime"),
            AUTH_DATABASE_URL: roleURL("webdock_auth_bootstrap"),
            DATABASE_WORKER_URL: roleURL("webdock_worker_runtime"),
            DATABASE_RLS_SECRET: base.DATABASE_CONTEXT_SECRET,
            WEBDOCK_MODULES: JSON.stringify({ configVersion: 1, modules: {} }),
          },
        },
      );
      assert.equal(minimal.status, 0, minimal.stdout + minimal.stderr);
      const first = (
        await owner.query(
          "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Preserved fixture') RETURNING id",
        )
      ).rows[0].id;
      const roleRows = (
        await owner.query(
          "SELECT rolname,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=ANY($1)",
          [runtimeRoles],
        )
      ).rows;
      assert.equal(roleRows.length, runtimeRoles.length);
      assert.ok(roleRows.every((r) => !r.rolsuper && !r.rolbypassrls));
      url.username = "webdock_offline_migrate";
      url.password = passwords.webdock_offline_migrate;
      // Fresh bootstrap already journals every migration as postgres. Exercise a
      // genuinely new function under the restricted upgrade login as well.
      const provisioningSQL = await readFile(
        new URL("../../legacy/control-plane/drizzle/0048_plugin_app_provisioning.sql", import.meta.url),
        "utf8",
      );
      const restricted = new Pool({ connectionString: url.href, max: 1 });
      const migration = await restricted.connect();
      try {
        await migration.query("BEGIN");
        assert.equal((await migration.query(
          "SELECT has_schema_privilege('webdock_platform_owner','webdock_rls','CREATE') AS allowed",
        )).rows[0].allowed, false);
        await migration.query("DROP FUNCTION webdock_rls.plugin_provisioning_status(text) CASCADE");
        for (const [, policy, table] of provisioningSQL.matchAll(/^CREATE POLICY (\w+) ON ([\w.]+)/gm))
          await migration.query(`DROP POLICY IF EXISTS ${policy} ON ${table}`);
        await migration.query("SAVEPOINT without_owner_create");
        await assert.rejects(
          migration.query(provisioningSQL.replace(
            "GRANT USAGE,CREATE ON SCHEMA webdock_rls TO webdock_platform_owner;",
            "",
          )),
          (error: any) => error.code === "42501" && /webdock_rls/.test(error.message),
        );
        await migration.query("ROLLBACK TO SAVEPOINT without_owner_create");
        await migration.query(provisioningSQL);
        assert.deepEqual((await migration.query(
          "SELECT p.proowner::regrole::text AS owner,has_schema_privilege('webdock_platform_owner','webdock_rls','CREATE') AS can_create FROM pg_proc p WHERE p.oid='webdock_rls.plugin_provisioning_status(text)'::regprocedure",
        )).rows[0], { owner: "webdock_platform_owner", can_create: false });
      } finally {
        await migration.query("ROLLBACK");
        migration.release();
        await restricted.end();
      }
      const initializePlugins = (modules: Record<string, unknown>) => spawnSync(
        process.execPath,
        ["--no-env-file", "tools/plugins/bootstrap.ts", target, "fresh"],
        {cwd:root,encoding:"utf8",env:{...base,DATABASE_URL_UNPOOLED:url.href,WEBDOCK_MODULES:JSON.stringify({configVersion:1,modules})}},
      );
      const initialized = initializePlugins({storage:{enabled:true}});
      assert.equal(initialized.status,0,initialized.stdout+initialized.stderr);
      const repeated = initializePlugins({});
      assert.equal(repeated.status,0,repeated.stdout+repeated.stderr);
      assert.deepEqual((await owner.query("SELECT id FROM webdock_auth.plugin_installation WHERE state='enabled' ORDER BY id")).rows,[{id:"storage"}]);
      // Both processes share the actual advisory lock and immutable journal.
      const concurrentUpgrade = () => new Promise<void>((resolve, reject) => {
        const child = spawn(process.execPath,
          ["--no-env-file", "tools/migrations/install.ts", target],
          { cwd: root, env: { ...base, DATABASE_URL_UNPOOLED: url.href }, stdio: ["ignore", "pipe", "pipe"] });
        let output = "";
        child.stdout.on("data", chunk => { output += chunk; });
        child.stderr.on("data", chunk => { output += chunk; });
        child.on("error", reject);
        child.on("close", code => code === 0 ? resolve() : reject(Error(output)));
      });
      await Promise.all([concurrentUpgrade(), concurrentUpgrade()]);
      const upgrade = run({ ...base, DATABASE_URL_UNPOOLED: url.href });
      assert.equal(upgrade.status, 0, upgrade.stdout + upgrade.stderr);
      assert.equal(
        (
          await owner.query(
            "SELECT name FROM webdock_admin.customers WHERE id=$1",
            [first],
          )
        ).rows[0].name,
        "Preserved fixture",
      );
      // An upgraded offline login must not inherit default PUBLIC execution on new routines.
      await owner.query(
        "BEGIN; SET LOCAL ROLE webdock_offline_migrate; CREATE FUNCTION webdock_auth.offline_privilege_fixture() RETURNS integer LANGUAGE sql AS 'SELECT 1'",
      );
      const publicFunction = (
        await owner.query(
          "SELECT count(*)::int AS n FROM pg_proc p CROSS JOIN LATERAL aclexplode(coalesce(p.proacl,acldefault('f',p.proowner))) acl WHERE p.oid='webdock_auth.offline_privilege_fixture()'::regprocedure AND acl.grantee=0 AND acl.privilege_type='EXECUTE'",
        )
      ).rows[0].n;
      assert.equal(publicFunction, 0);
      await owner.query("ROLLBACK");
      const clerk = (
        await owner.query(
          "SELECT relrowsecurity,relforcerowsecurity FROM pg_class WHERE oid='webdock_auth.clerk_identity_mapping'::regclass",
        )
      ).rows[0];
      assert.equal(clerk.relrowsecurity, true);
      assert.equal(clerk.relforcerowsecurity, true);
      await owner.query("ALTER ROLE webdock_offline_migrate SUPERUSER");
      const elevated = run({ ...base, DATABASE_URL_UNPOOLED: url.href });
      assert.notEqual(elevated.status, 0);
      assert.match(elevated.stderr, /dedicated non-superuser/);
      await owner.query("ALTER ROLE webdock_offline_migrate NOSUPERUSER");
      await owner.query(
        "INSERT INTO webdock_api_migrations.journal(id,checksum) VALUES('9999_missing_fixture.sql','fixture')",
      );
      const missing = run({ ...base, DATABASE_URL_UNPOOLED: url.href });
      assert.notEqual(missing.status, 0);
      assert.match(missing.stderr, /missing from this release/);
      await owner.query(
        "DELETE FROM webdock_api_migrations.journal WHERE id='9999_missing_fixture.sql'",
      );
      for (const secret of [
        ...Object.values(passwords),
        base.DATABASE_CONTEXT_SECRET,
      ])
        assert.ok(
          !(
            fresh.stdout +
            fresh.stderr +
            upgrade.stdout +
            upgrade.stderr +
            elevated.stdout +
            elevated.stderr +
            missing.stdout +
            missing.stderr
          ).includes(secret),
        );
      const rejected = run({
        ...base,
        DATABASE_URL_UNPOOLED: url.href,
        DATABASE_CONTEXT_SECRET: "different".repeat(8),
      });
      assert.notEqual(rejected.status, 0);
      assert.match(rejected.stderr, /signing key differs/);
      const journal = (
        await owner.query(
          "SELECT count(*)::int AS n FROM webdock_api_migrations.journal",
        )
      ).rows[0].n;
      assert.ok(journal >= 35);
    } finally {
      await owner?.end();
      assert.equal(
        docker([
          "inspect",
          "--format",
          '{{index .Config.Labels "webdock.install-fixture"}}',
          name,
        ]),
        name,
      );
      docker(["rm", "-f", name]);
    }
  },
);
