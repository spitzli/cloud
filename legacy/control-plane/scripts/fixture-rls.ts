/** Creates a constrained-role copy only inside this workspace's disposable local Docker fixture. */
import { readFile, writeFile, access } from "node:fs/promises";
import { parseEnv } from "node:util";
import { fileURLToPath } from "node:url";
import { createHash, randomBytes } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { Pool } from "pg";
import { assertTestDatabase } from "../tests/helpers/database";
const root = fileURLToPath(new URL("../", import.meta.url)),
  metadataPath = root + ".api-fixture.json";
const metadata = JSON.parse(await readFile(metadataPath, "utf8")) as {
  name: string;
  token: string;
  envHash: string;
  rlsFiles?: Record<string, string>;
};
const source = await readFile(root + ".env.test.local", "utf8");
if (createHash("sha256").update(source).digest("hex") !== metadata.envHash)
  throw Error("Use an unchanged fixture created by test:setup");
const env = parseEnv(source);
if (!env.API_TEST_DATABASE_URL) throw Error("Fixture database URL required");
const url = new URL(env.API_TEST_DATABASE_URL);
assertTestDatabase(url.href);
for (const file of [
  ".rls-fixture.json",
  ".env.rls.test.local",
  ".rls-installer.env",
])
  if (
    await access(root + file).then(
      () => true,
      () => false,
    )
  )
    throw Error("Existing RLS fixture configuration preserved");
const endpoint =
  process.env.DOCKER_HOST ||
  execFileSync(
    "docker",
    [
      "context",
      "inspect",
      ...(process.env.DOCKER_CONTEXT ? [process.env.DOCKER_CONTEXT] : []),
      "--format",
      "{{.Endpoints.docker.Host}}",
    ],
    { encoding: "utf8" },
  ).trim();
if (!endpoint.startsWith("unix://"))
  throw Error("Local Docker engine required");
const dockerEnv: NodeJS.ProcessEnv = { ...process.env, DOCKER_HOST: endpoint };
delete dockerEnv.DOCKER_CONTEXT;
const docker = (args: string[]) =>
  execFileSync("docker", args, { env: dockerEnv, encoding: "utf8" }).trim();
if (
  !/^webdock-api-fixture-[a-f0-9]{12}$/.test(metadata.name) ||
  docker([
    "inspect",
    "--format",
    '{{index .Config.Labels "webdock.api-fixture"}}',
    metadata.name,
  ]) !== metadata.token ||
  docker(["port", metadata.name, "5432/tcp"]) !== "127.0.0.1:" + url.port
)
  throw Error("Fixture ownership or endpoint mismatch");
const adminURL = new URL(url);
adminURL.pathname = "/postgres";
const admin = new Pool({ connectionString: adminURL.href, max: 1 });
const target = new URL(url);
target.pathname = "/webdock_rls_test";
const logins = [
  "webdock_auth_bootstrap",
  "webdock_auth_runtime",
  "webdock_worker_runtime",
  "webdock_registry_runtime",
  "webdock_admin_runtime",
  "webdock_runtime",
  "webdock_browser_website",
  "webdock_cms_archive_runtime",
];
const owners = [
  "webdock_platform_owner",
  "webdock_policy_owner",
  "webdock_website_owner",
];
const fixture = {
  contextSecret: randomBytes(48).toString("base64url"),
  roles: {} as Record<string, string>,
};
try {
  const version = Number(
    (await admin.query("SHOW server_version_num")).rows[0].server_version_num,
  );
  if (version < 180000 || version >= 190000)
    throw Error("Use the PostgreSQL18 fixture");
  if (
    (
      await admin.query(
        "SELECT 1 FROM pg_database WHERE datname='webdock_rls_test'",
      )
    ).rowCount
  )
    throw Error(
      "Existing RLS database preserved; reset the owned fixture explicitly",
    );
  const present = (
    await admin.query(
      "SELECT rolname,rolcanlogin FROM pg_roles WHERE rolname=ANY($1)",
      [[...owners, ...logins]],
    )
  ).rows;
  if (
    present.some(
      (role) => role.rolname !== "webdock_auth_runtime" || role.rolcanlogin,
    )
  )
    throw Error("Unexpected existing roles; refusing credential changes");
  await admin.query(
    "CREATE DATABASE webdock_rls_test TEMPLATE webdock_api_test",
  );
  const db = new Pool({ connectionString: target.href, max: 1 });
  try {
    await db.query("BEGIN");
    for (const name of [...owners, ...logins]) {
      if (!present.some((role) => role.rolname === name))
        await db.query(`CREATE ROLE ${name}`);
      const password = randomBytes(32).toString("hex");
      await db.query(
        `ALTER ROLE ${name} NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION ${logins.includes(name) ? "LOGIN PASSWORD '" + password + "'" : "NOLOGIN"}`,
      );
      if (logins.includes(name)) {
        const roleURL = new URL(target);
        roleURL.username = name;
        roleURL.password = password;
        fixture.roles[name] = roleURL.href;
      }
    }
    await db.query(
      "CREATE SCHEMA webdock; CREATE TABLE webdock.payload_kv(id text PRIMARY KEY); CREATE TABLE public.fixture_archive(id text PRIMARY KEY); INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true) ON CONFLICT DO NOTHING; INSERT INTO webdock_admin.snowflake_state(singleton) VALUES(true) ON CONFLICT DO NOTHING;",
    );
    await db.query(
      "CREATE SCHEMA webdock_rls AUTHORIZATION webdock_policy_owner; CREATE EXTENSION pgcrypto WITH SCHEMA webdock_rls;",
    );
    await db.query(
      `DO $$ DECLARE routine record;BEGIN FOR routine IN SELECT p.oid::regprocedure AS signature FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_depend d ON d.classid='pg_catalog.pg_proc'::regclass AND d.objid=p.oid JOIN pg_catalog.pg_extension e ON e.oid=d.refobjid WHERE e.extname='pgcrypto' AND d.deptype='e' LOOP EXECUTE format('ALTER FUNCTION %s OWNER TO webdock_policy_owner',routine.signature);END LOOP;END $$;`,
    );
    await db.query("COMMIT");
  } catch (error) {
    await db.query("ROLLBACK");
    throw error;
  } finally {
    await db.end();
  }
  const values = {
    NODE_ENV: "development",
    BETTER_AUTH_URL: "http://localhost:3125",
    BETTER_AUTH_SECRET: env.BETTER_AUTH_SECRET,
    API_TEST_OWNER_URL: target.href,
    WEBDOCK_RLS_FIXTURE_FILE: root + ".rls-fixture.json",
  };
  const artifacts = {
    ".rls-fixture.json": JSON.stringify(fixture),
    ".env.rls.test.local":
      Object.entries(values)
        .map(([key, value]) => key + "=" + value)
        .join("\n") + "\n",
    ".rls-installer.env": `DATABASE_URL=${target.href}\nMIGRATION_TARGET_HOST=127.0.0.1\nMIGRATION_TARGET_DATABASE=webdock_rls_test\nDATABASE_CONTEXT_SECRET=${fixture.contextSecret}\n`,
  };
  for (const [name, contents] of Object.entries(artifacts))
    await writeFile(root + name, contents, { mode: 0o600, flag: "wx" });
  metadata.rlsFiles = Object.fromEntries(
    Object.entries(artifacts).map(([name, value]) => [
      name,
      createHash("sha256").update(value).digest("hex"),
    ]),
  );
  await writeFile(metadataPath, JSON.stringify(metadata), { mode: 0o600 });
  const childEnv: NodeJS.ProcessEnv = {
    PATH: process.env.PATH,
    HOME: process.env.HOME,
    NODE_ENV: "development",
    DATABASE_URL_UNPOOLED: target.href,
  };
  for (const args of [
    [
      "--no-env-file",
      "scripts/install-platform-rls.ts",
      root + ".rls-installer.env",
    ],
    [
      "--no-env-file",
      "scripts/migrate.ts",
      `${target.hostname}:${target.port}${target.pathname}`,
    ],
    [
      "--no-env-file",
      "scripts/install-platform-rls.ts",
      root + ".rls-installer.env",
    ],
  ]) {
    const result = spawnSync(process.execPath, args, {
      cwd: root,
      env: childEnv,
      stdio: "inherit",
    });
    if (result.status !== 0)
      throw Error(
        "Fixture schema/policy setup failed; retain private files for diagnosis",
      );
  }
  console.log(
    "Restricted-role API fixture ready; use test:rls. No external provider was contacted.",
  );
} finally {
  await admin.end();
}
