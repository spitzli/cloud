/** Explicit offline fresh install/upgrade. Never imported by application startup. */
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { applyApiMigrations } from "../../legacy/control-plane/scripts/lib/apply-migrations";
import { installPlatformPolicies } from "../../legacy/control-plane/scripts/lib/install-policies";
import { migrationConfig, runtimeRoles, ownerRoles } from "./config";
const quote = (value: string) => '"' + value.replaceAll('"', '""') + '"';
const config = migrationConfig(process.env, process.argv[2]);
const databaseName = decodeURIComponent(config.url.pathname.slice(1));
const pool = new Pool({
  ...(config.local
    ? { connectionString: config.url.href }
    : verifiedPostgresConfig(config.url.href, config.ca)),
  max: 1,
});
const db = await pool.connect();
let phase = "preflight";
try {
  await db.query("BEGIN");
  await db.query(
    "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='120s'",
  );
  await db.query(
    "SELECT pg_advisory_xact_lock(hashtext('webdock-api-migration'))",
  );
  if (
    (await db.query("SELECT current_database() AS name")).rows[0].name !==
    databaseName
  )
    throw Error("Connected database differs from the explicit target");
  const version = Number(
    (await db.query("SHOW server_version_num")).rows[0].server_version_num,
  );
  if (version < 180000 || version >= 190000)
    throw Error("This release requires PostgreSQL 18");
  if (!config.bootstrap) {
    const login = (
      await db.query(
        "SELECT session_user AS login,current_user AS active,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=session_user",
      )
    ).rows[0];
    if (
      login?.login !== "webdock_offline_migrate" ||
      login.active !== login.login ||
      login.rolsuper ||
      login.rolbypassrls
    )
      throw Error(
        "Upgrades require the dedicated non-superuser offline migrator",
      );
  }
  const baseline = await readFile(
    new URL("./baseline.sql", import.meta.url),
    "utf8",
  );
  const checksum = createHash("sha256").update(baseline).digest("hex");
  if (
    checksum !==
    (
      await readFile(new URL("./baseline.sha256", import.meta.url), "utf8")
    ).trim()
  )
    throw Error("Baseline artifact checksum mismatch");
  if (config.bootstrap) {
    if (
      !(
        await db.query(
          "SELECT rolsuper FROM pg_roles WHERE rolname=current_user",
        )
      ).rows[0]?.rolsuper
    )
      throw Error(
        "Initial empty-database role bootstrap requires the database administrator",
      );
    if (
      (
        await db.query("SELECT 1 FROM pg_roles WHERE rolname=ANY($1)", [
          [...ownerRoles, ...runtimeRoles, "webdock_offline_migrate"],
        ])
      ).rowCount
    )
      throw Error(
        "Webdock roles already exist; bootstrap never rotates or reuses them",
      );
    if (
      (
        await db.query("SELECT 1 FROM pg_namespace WHERE nspname=ANY($1)", [
          [
            "webdock",
            "webdock_auth",
            "webdock_admin",
            "webdock_mail",
            "webdock_rls",
            "webdock_api_migrations",
          ],
        ])
      ).rowCount
    )
      throw Error("Bootstrap requires an empty target database");
    if (
      (
        await db.query(
          "SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p')",
        )
      ).rowCount
    )
      throw Error("Bootstrap refuses existing public tables");
    phase = "roles";
    for (const role of ownerRoles)
      await db.query(
        `CREATE ROLE ${quote(role)} NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`,
      );
    for (const role of [...runtimeRoles, "webdock_offline_migrate"]) {
      const statement = (
        await db.query(
          "SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOCREATEDB NOREPLICATION NOBYPASSRLS ',$1::text,$2::text) AS sql",
          [role, config.passwords[role]],
        )
      ).rows[0].sql;
      await db.query(
        statement +
          (role === "webdock_offline_migrate"
            ? "INHERIT CREATEROLE"
            : "NOINHERIT NOCREATEROLE"),
      );
    }
    for (const role of [...ownerRoles, ...runtimeRoles])
      await db.query(
        `GRANT ${quote(role)} TO webdock_offline_migrate WITH ADMIN OPTION`,
      );
    await db.query(
      `ALTER DATABASE ${quote(databaseName)} OWNER TO webdock_offline_migrate`,
    );
    await db.query(
      "GRANT webdock_website_owner TO webdock_browser_website; CREATE SCHEMA webdock AUTHORIZATION webdock_website_owner; CREATE SCHEMA webdock_rls AUTHORIZATION webdock_policy_owner; CREATE EXTENSION pgcrypto WITH SCHEMA webdock_rls",
    );
    await db.query(
      `DO $$ DECLARE routine record; BEGIN FOR routine IN SELECT proc.oid::regprocedure AS name FROM pg_proc proc JOIN pg_depend d ON d.classid='pg_proc'::regclass AND d.objid=proc.oid AND d.deptype='e' JOIN pg_extension e ON e.oid=d.refobjid WHERE e.extname='pgcrypto' LOOP EXECUTE format('ALTER FUNCTION %s OWNER TO webdock_policy_owner',routine.name); END LOOP; END $$`,
    );
    phase = "baseline";
    await db.query(baseline);
    await db.query(
      "SET row_security=on; SET LOCAL statement_timeout='120s'; INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true) ON CONFLICT DO NOTHING; INSERT INTO webdock_admin.snowflake_state(singleton) VALUES(true) ON CONFLICT DO NOTHING",
    );
    await db.query(
      "CREATE SCHEMA webdock_api_migrations AUTHORIZATION webdock_platform_owner; REVOKE ALL ON SCHEMA webdock_api_migrations FROM PUBLIC; CREATE TABLE webdock_api_migrations.baseline(id boolean PRIMARY KEY CHECK(id),checksum text NOT NULL); ALTER TABLE webdock_api_migrations.baseline OWNER TO webdock_platform_owner",
    );
    await db.query(
      "INSERT INTO webdock_api_migrations.baseline VALUES(true,$1)",
      [checksum],
    );
  } else {
    const recorded = (
      await db.query(
        "SELECT checksum FROM webdock_api_migrations.baseline WHERE id",
      )
    ).rows[0];
    if (recorded?.checksum !== checksum)
      throw Error(
        "Baseline mismatch; use the explicit migration path for this database version",
      );
  }
  // New functions are created by this login before ownership is transferred.
  await db.query(
    "ALTER DEFAULT PRIVILEGES FOR ROLE webdock_offline_migrate REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC",
  );
  phase = "policies";
  await installPlatformPolicies(db, databaseName, config.contextSecret, false);
  phase = "migrations";
  await applyApiMigrations(db);
  await db.query(
    "ALTER TABLE webdock_api_migrations.journal OWNER TO webdock_platform_owner; REVOKE ALL ON webdock_api_migrations.journal FROM PUBLIC",
  );
  phase = "policies";
  await installPlatformPolicies(db, databaseName, config.contextSecret, false);
  await db.query("COMMIT");
  console.log(
    JSON.stringify({
      installed: true,
      bootstrap: config.bootstrap,
      baseline: checksum,
      rls: true,
    }),
  );
} catch (error) {
  await db.query("ROLLBACK");
  // PostgreSQL details can contain secret parameters. Only controlled errors or codes leave the migrator.
  console.error(
    error instanceof Error && !("code" in error)
      ? error.message
      : "Offline installation failed; transaction rolled back (" +
          phase +
          ":" +
          String((error as { code?: string }).code ?? "unknown") +
          ":" +
          String((error as { routine?: string }).routine ?? "unknown") +
          ")",
  );
  process.exitCode = 1;
} finally {
  db.release();
  await pool.end();
}
