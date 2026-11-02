/** Explicit retained-schema bridge. Never imported by application startup. */
import { readFile, readdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { Pool, type PoolClient } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { mailboxSchemaSQL } from "../../modules/mail/src/server/native-mailbox-schema";
import { mailSendSchemaSQL } from "../../modules/mail/src/server/native-send-schema";
import { mailBudgetSchemaSQL } from "../../modules/mail/src/server/native-budget";
import { outboxSchemaSQL } from "../../modules/mail/src/server/outbox";
import { installPlatformPolicies } from "../../legacy/control-plane/scripts/lib/install-policies";
import { migrationConfig } from "./config";

const optionalTables = ["domain", "mailbox", "identity", "mailbox_grant", "configuration", "configuration_operation", "access_event", "webmail_launch", "webmail_session", "webmail_write", "api_key", "send_transport", "month_usage", "budget_reservation", "send_job"];

export async function adoptRetainedMail(db: PoolClient, expectedDatabase: string, contextSecret: string) {
  if (Buffer.byteLength(contextSecret) < 32) throw Error("Original signing key required");
  await db.query("BEGIN");
  try {
    await db.query("SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='120s'");
    const identity = (await db.query("SELECT current_database() AS database,session_user AS role,current_user AS active,rolsuper AS superuser,rolbypassrls AS bypass FROM pg_roles WHERE rolname=session_user")).rows[0];
    if (identity?.database !== expectedDatabase || identity.role !== "webdock_offline_migrate" || identity.active !== identity.role || identity.superuser !== false || identity.bypass !== false)
      throw Error("Retained adoption requires dedicated offline login and exact database");
    await db.query("SELECT pg_advisory_xact_lock(hashtext('webdock-api-migration'))");
    if ((await db.query("SELECT to_regclass('webdock_api_migrations.rust_journal') IS NOT NULL AS native_present")).rows[0]?.native_present)
      throw Error("Retained adoption must run before any native migration");
    const provenance = (await db.query(`SELECT
      (SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname='webdock_api_migrations') AS schema_owner,
      (SELECT pg_get_userbyid(c.relowner) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='webdock_api_migrations' AND c.relname='journal' AND c.relkind='r') AS journal_owner,
      (SELECT pg_get_userbyid(c.relowner) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='webdock_rls' AND c.relname='context_key' AND c.relkind='r') AS key_owner,
      (SELECT count(*)::int FROM webdock_rls.context_key) AS key_count,
      (SELECT secret=$1 FROM webdock_rls.context_key WHERE id) AS key_matches,
      to_regclass('webdock_mail.service') IS NOT NULL AND to_regclass('webdock_mail.instance') IS NOT NULL
      AND to_regclass('webdock_auth.tenant_customer') IS NOT NULL AND to_regclass('webdock_auth.mail_sender_domain') IS NOT NULL
      AND to_regclass('webdock_auth.user') IS NOT NULL AND to_regclass('webdock_auth.member') IS NOT NULL
      AND to_regclass('webdock_auth.session') IS NOT NULL AND to_regprocedure('webdock_auth.next_snowflake()') IS NOT NULL AS prerequisites,
      to_regclass('webdock_api_migrations.baseline') IS NOT NULL AS baseline_present`, [contextSecret])).rows[0];
    if (provenance?.schema_owner !== "webdock_platform_owner" || provenance.journal_owner !== "webdock_platform_owner" || provenance.key_owner !== "webdock_policy_owner" || provenance.key_count !== 1 || provenance.key_matches !== true || provenance.prerequisites !== true)
      throw Error("Owned retained history, original signing key and existing Mail service metadata required");
    const directory = new URL("../../legacy/control-plane/drizzle/", import.meta.url);
    const names = (await readdir(directory)).filter(name => /^\d{4}_[a-z0-9_]+\.sql$/.test(name)).sort();
    const expected = new Map(await Promise.all(names.map(async name => [name, createHash("sha256").update(await readFile(new URL(name, directory))).digest("hex")] as const)));
    const applied = (await db.query<{id:string;checksum:string}>("SELECT id,checksum FROM webdock_api_migrations.journal ORDER BY id")).rows;
    if (applied.length !== expected.size || applied.some(row => expected.get(row.id) !== row.checksum))
      throw Error("Retained migration history differs from this release");
    // Old controlled installs predate the baseline marker. Exact owned history is
    // their provenance; never create/stamp a missing marker on an occupied database.
    if (provenance.baseline_present) {
      const baseline = (await db.query("SELECT b.checksum,pg_get_userbyid(c.relowner) AS owner FROM webdock_api_migrations.baseline b CROSS JOIN pg_class c WHERE b.id AND c.oid='webdock_api_migrations.baseline'::regclass")).rows;
      const allowed = (await readFile(new URL("./baseline.sha256", import.meta.url), "utf8")).trim();
      if (baseline.length !== 1 || baseline[0].owner !== "webdock_platform_owner" || baseline[0].checksum !== allowed)
        throw Error("Retained baseline marker differs; explicit version adoption required");
    }
    const before = (await db.query<{name:string;owner:string}>("SELECT c.relname AS name,pg_get_userbyid(c.relowner) AS owner FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='webdock_mail' AND c.relname=ANY($1) AND c.relkind='r'", [optionalTables])).rows;
    if (before.some(table => table.owner !== "webdock_platform_owner")) throw Error("Unexpected optional Mail table owner");
    await db.query("ALTER DEFAULT PRIVILEGES FOR ROLE webdock_offline_migrate REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC");
    await db.query(mailboxSchemaSQL);
    await db.query(mailSendSchemaSQL);
    await db.query(mailBudgetSchemaSQL);
    await db.query(outboxSchemaSQL);
    await installPlatformPolicies(db, expectedDatabase, contextSecret, false);
    await db.query("COMMIT");
    return { adopted: true, createdTables: optionalTables.filter(name => !before.some(table => table.name === name)) };
  } catch (error) {
    await db.query("ROLLBACK");
    throw error;
  }
}

if (import.meta.main) {
  const config = migrationConfig(process.env, process.argv[2]);
  if (config.bootstrap) throw Error("Retained adoption never bootstraps a database");
  const pool = new Pool({ ...(config.local ? {connectionString:config.url.href} : verifiedPostgresConfig(config.url.href,config.ca)), max:1 });
  try {
    const db = await pool.connect();
    try { console.log(JSON.stringify(await adoptRetainedMail(db, decodeURIComponent(config.url.pathname.slice(1)), config.contextSecret))); }
    finally { db.release(); }
  } catch (error) {
    console.error(error instanceof Error && !("code" in error) ? error.message : "Retained adoption failed; transaction rolled back ("+String((error as {code?:string}).code ?? "unknown")+")");
    process.exitCode=1;
  } finally { await pool.end(); }
}
