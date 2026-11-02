/** EU runner only: two generated logins in one fresh owned loopback PostgreSQL fixture. */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import { Pool, type PoolClient } from "pg";
import { inspectNeonEnrollmentRecovery, recoverNeonEnrollment } from "./neon-enrollment-recovery";

function fixtureURL(value: string | undefined) {
  let url: URL;
  try { url = new URL(value ?? ""); } catch { throw Error("Explicit owned loopback fixture URL required"); }
  assert.ok(["postgres:", "postgresql:"].includes(url.protocol));
  assert.ok(["127.0.0.1", "localhost"].includes(url.hostname));
  assert.ok(/^\/[a-z][a-z0-9_]*_test$/.test(url.pathname));
  assert.equal(url.search + url.hash, "");
  return url;
}

test("restricted offline owner repairs a dropped signup response with FORCE RLS and provider column-only SELECT", {
  skip: process.env.WEBDOCK_TEST_NEON_RECOVERY !== "true",
  timeout: 30000,
}, async () => {
  const url = fixtureURL(process.env.WEBDOCK_TEST_NEON_RECOVERY_DATABASE_URL);
  const bootstrapURL = fixtureURL(process.env.WEBDOCK_TEST_NEON_RECOVERY_BOOTSTRAP_DATABASE_URL);
  assert.equal(decodeURIComponent(url.username), "neondb_owner");
  assert.notEqual(bootstrapURL.username, url.username);
  assert.equal(bootstrapURL.host + bootstrapURL.pathname, url.host + url.pathname, "bootstrap must target the same owned fixture");
  const database = url.pathname.slice(1);
  const pool = new Pool({ connectionString: url.href, max: 3 });
  const bootstrapPool = new Pool({ connectionString: bootstrapURL.href, max: 2 });
  const client = await pool.connect(), admin = await bootstrapPool.connect();
  const origin = "https://ep-fixture.neonauth.c-7.eu-central-1.aws.neon.tech/neondb/auth";
  const providerSubject = "2b28db90-addd-431e-82f1-de0657dbea6e";
  const target = { userID: "101", providerSubject, providerOrigin: origin };
  const server = createServer(async (request, response) => {
    let provider: PoolClient | undefined;
    try {
      if (request.method !== "POST" || request.url !== "/sign-up/email") { response.writeHead(404).end(); return; }
      // The fixture provider's separate connection commits before losing its HTTP response.
      provider = await bootstrapPool.connect();
      await provider.query("BEGIN; SET LOCAL ROLE owned_neon_provider");
      await provider.query(`INSERT INTO neon_auth."user"(id,email,"emailVerified",banned) VALUES($1,'operator@example.invalid',false,false)`, [providerSubject]);
      await provider.query(`INSERT INTO neon_auth.account(id,"userId","providerId",password) VALUES('301',$1,'credential','synthetic-provider-credential')`, [providerSubject]);
      await provider.query("COMMIT");
      request.socket.destroy();
    } catch { await provider?.query("ROLLBACK").catch(() => {}); response.writeHead(422).end(); }
    finally { provider?.release(); }
  });
  try {
    const custody = (await client.query("SELECT session_user AS role,current_user AS active,current_database() AS database,rolsuper,rolbypassrls,rolcreaterole,rolinherit FROM pg_roles WHERE rolname=session_user")).rows[0];
    assert.deepEqual(custody, { role: "neondb_owner", active: "neondb_owner", database, rolsuper: false, rolbypassrls: false, rolcreaterole: false, rolinherit: false });
    assert.equal((await admin.query("SELECT rolsuper FROM pg_roles WHERE rolname=session_user")).rows[0].rolsuper, true, "bootstrap is separate fixture-only custody");
    assert.equal((await admin.query("SELECT to_regnamespace('webdock_auth') AS schema")).rows[0].schema, null, "refuse an occupied fixture");
    await admin.query(`
      DO $$ DECLARE role_name text; BEGIN
       FOREACH role_name IN ARRAY ARRAY['webdock_platform_owner','webdock_policy_owner','webdock_auth_bootstrap','webdock_auth_runtime','webdock_registry_runtime','webdock_worker_runtime','owned_neon_provider'] LOOP
        IF NOT EXISTS(SELECT FROM pg_roles WHERE rolname=role_name) THEN
         EXECUTE format('CREATE ROLE %I NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE NOINHERIT',role_name);
        END IF;
       END LOOP;
      END $$;
      CREATE SCHEMA webdock_auth AUTHORIZATION webdock_platform_owner;
      CREATE SCHEMA neon_auth AUTHORIZATION owned_neon_provider;
      CREATE SCHEMA webdock_rls AUTHORIZATION webdock_policy_owner;
      GRANT USAGE ON SCHEMA webdock_rls TO webdock_platform_owner;
      GRANT USAGE ON SCHEMA webdock_auth TO webdock_auth_bootstrap;
      CREATE TABLE webdock_auth."user"(id text PRIMARY KEY,email text NOT NULL,role text,"emailVerified" boolean,banned boolean,"twoFactorEnabled" boolean,"mustChangePassword" boolean);
      CREATE TABLE webdock_auth.account(id text PRIMARY KEY,"userId" text REFERENCES webdock_auth."user", "providerId" text,password text);
      CREATE TABLE webdock_auth."twoFactor"(id text PRIMARY KEY,"userId" text,secret text,"backupCodes" text);
      CREATE TABLE webdock_auth.passkey(id text PRIMARY KEY,"userId" text,"publicKey" text,counter int);
      CREATE TABLE webdock_auth.member(id text PRIMARY KEY,"userId" text,role text);
      CREATE TABLE webdock_auth.project_grant(id text PRIMARY KEY,user_id text,enabled boolean);
      CREATE TABLE webdock_auth.session(id text PRIMARY KEY,"userId" text);
      CREATE TABLE neon_auth."user"(id uuid PRIMARY KEY,email text,"emailVerified" boolean,banned boolean);
      CREATE TABLE neon_auth.account(id text PRIMARY KEY,"userId" uuid REFERENCES neon_auth."user","providerId" text,password text);
      ALTER TABLE neon_auth."user" OWNER TO owned_neon_provider;
      ALTER TABLE neon_auth.account OWNER TO owned_neon_provider;
      REVOKE ALL ON SCHEMA neon_auth FROM PUBLIC;
      REVOKE ALL ON neon_auth."user",neon_auth.account FROM PUBLIC;
      -- Exact 0069 provider grants: account.id and every provider write remain unavailable.
      GRANT USAGE ON SCHEMA neon_auth TO webdock_platform_owner;
      GRANT SELECT(id,email,"emailVerified",banned) ON neon_auth."user" TO webdock_platform_owner;
      GRANT SELECT("userId","providerId",password) ON neon_auth.account TO webdock_platform_owner;
      CREATE TABLE webdock_auth.neon_identity(user_id text PRIMARY KEY REFERENCES webdock_auth."user",provider_origin text NOT NULL,provider_subject uuid NOT NULL UNIQUE,enabled boolean NOT NULL DEFAULT true,revision int NOT NULL DEFAULT 1,linked_by text NOT NULL,linked_at timestamptz NOT NULL DEFAULT statement_timestamp());
      DO $$ DECLARE table_name text; BEGIN
       FOREACH table_name IN ARRAY ARRAY['user','account','twoFactor','passkey','member','project_grant','session','neon_identity'] LOOP
        EXECUTE format('ALTER TABLE webdock_auth.%I OWNER TO webdock_platform_owner',table_name);
        EXECUTE format('ALTER TABLE webdock_auth.%I ENABLE ROW LEVEL SECURITY',table_name);
        EXECUTE format('ALTER TABLE webdock_auth.%I FORCE ROW LEVEL SECURITY',table_name);
        EXECUTE format('REVOKE ALL ON webdock_auth.%I FROM PUBLIC',table_name);
        EXECUTE format('CREATE POLICY platform_owner ON webdock_auth.%I TO webdock_platform_owner USING(true) WITH CHECK(true)',table_name);
       END LOOP;
      END $$;
      CREATE FUNCTION webdock_rls.local_identity_authority(text) RETURNS boolean LANGUAGE sql AS 'SELECT true';
      ALTER FUNCTION webdock_rls.local_identity_authority(text) OWNER TO webdock_platform_owner;
      REVOKE ALL ON FUNCTION webdock_rls.local_identity_authority(text) FROM PUBLIC;
      INSERT INTO webdock_auth."user" VALUES('101','operator@example.invalid','operator',true,false,true,false);
      INSERT INTO webdock_auth.account VALUES('201','101','credential','synthetic-core-credential');
      INSERT INTO webdock_auth."twoFactor" VALUES('401','101','synthetic-encrypted-factor','synthetic-encrypted-backups');
      INSERT INTO webdock_auth.passkey VALUES('501','101','retained-public-key',7);
      INSERT INTO webdock_auth.member VALUES('601','101','owner');
      INSERT INTO webdock_auth.project_grant VALUES('701','101',true);
    `);
    await admin.query(await readFile(new URL("../../crates/persistence/migrations/0074_neon_enrollment_recovery.sql", import.meta.url), "utf8"));
    await assert.rejects(inspectNeonEnrollmentRecovery(client, database, origin, target), "missing SET membership must fail closed");
    await admin.query("GRANT webdock_platform_owner TO neondb_owner WITH ADMIN FALSE, INHERIT FALSE, SET TRUE");
    await assert.rejects(client.query('SELECT * FROM webdock_auth."user"'), (error: any) => error.code === "42501", "NOINHERIT custody must not silently become table-owner access");
    await client.query("BEGIN; SET LOCAL ROLE webdock_platform_owner");
    const privileges = (await client.query(`SELECT current_user AS active,rolsuper,rolbypassrls,rolcanlogin,
      has_column_privilege(current_user,'neon_auth.account','id','SELECT') AS account_id,
      has_table_privilege(current_user,'neon_auth.account','UPDATE') AS provider_update,
      has_table_privilege(current_user,'neon_auth.account','INSERT') AS provider_insert,
      has_table_privilege(current_user,'neon_auth.account','DELETE') AS provider_delete,
      has_table_privilege(current_user,'neon_auth.user','UPDATE') AS provider_user_update
      FROM pg_roles WHERE rolname=current_user`)).rows[0];
    await client.query("ROLLBACK");
    assert.deepEqual(privileges, { active: "webdock_platform_owner", rolsuper: false, rolbypassrls: false, rolcanlogin: false, account_id: false, provider_update: false, provider_insert: false, provider_delete: false, provider_user_update: false });
    for (const sql of ['SELECT id FROM neon_auth.account', 'UPDATE neon_auth.account SET password=password', 'SELECT "userId" FROM neon_auth.account FOR SHARE']) {
      await client.query("BEGIN; SET LOCAL ROLE webdock_platform_owner");
      try { await assert.rejects(client.query(sql), (error: any) => error.code === "42501"); }
      finally { await client.query("ROLLBACK"); }
    }
    const coreOwners = (await admin.query("SELECT c.relname,pg_get_userbyid(c.relowner) AS owner,c.relrowsecurity,c.relforcerowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='webdock_auth' AND c.relname IN ('user','account','neon_identity','neon_enrollment_recovery')")).rows;
    assert.equal(coreOwners.length, 4);
    assert.ok(coreOwners.every(row => row.owner === "webdock_platform_owner" && row.relrowsecurity && row.relforcerowsecurity));
    server.listen(0, "127.0.0.1"); await once(server, "listening");
    const address = server.address(); assert.ok(address && typeof address === "object");
    await assert.rejects(fetch(`http://127.0.0.1:${address.port}/sign-up/email`, { method: "POST" }));
    const mappings = async () => (await admin.query("SELECT count(*)::int AS n FROM webdock_auth.neon_identity")).rows[0].n;
    assert.equal(await mappings(), 0);
    const preserved = async () => (await admin.query(`SELECT jsonb_build_object(
      'user',(SELECT jsonb_agg(to_jsonb(t)) FROM webdock_auth."user" t),
      'credentials',(SELECT jsonb_agg(to_jsonb(t)) FROM webdock_auth.account t),
      'providerCredentials',(SELECT jsonb_agg(to_jsonb(t)) FROM neon_auth.account t),
      'factors',(SELECT jsonb_agg(to_jsonb(t)) FROM webdock_auth."twoFactor" t),
      'passkeys',(SELECT jsonb_agg(to_jsonb(t)) FROM webdock_auth.passkey t),
      'memberships',(SELECT jsonb_agg(to_jsonb(t)) FROM webdock_auth.member t),
      'grants',(SELECT jsonb_agg(to_jsonb(t)) FROM webdock_auth.project_grant t),
      'sessions',(SELECT count(*) FROM webdock_auth.session)) AS value`)).rows[0].value;
    const before = await preserved();
    const inspected = await inspectNeonEnrollmentRecovery(client, database, origin, target);
    assert.equal((await client.query("SELECT current_user AS active")).rows[0].active, "neondb_owner", "SET LOCAL must not leak to the next operation");
    const review = { ...target, snapshotSha256: inspected.snapshotSha256, evidenceRef: "fixture:dropped-response", evidenceSha256: "c".repeat(64), reviewerRef: "disposable-fixture-owner" };
    assert.equal(JSON.stringify(inspected).includes("synthetic-core-credential"), false);
    assert.equal(JSON.stringify(inspected).includes("synthetic-provider-credential"), false);
    await admin.query("UPDATE webdock_auth.account SET password='changed' WHERE id='201'");
    await assert.rejects(recoverNeonEnrollment(client, database, origin, review));
    await admin.query("UPDATE webdock_auth.account SET password='synthetic-core-credential' WHERE id='201'");
    await admin.query(`CREATE FUNCTION webdock_auth.fixture_reject_recovery_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture audit unavailable'; END $$;
      CREATE TRIGGER fixture_reject_recovery_audit BEFORE INSERT ON webdock_auth.neon_enrollment_recovery FOR EACH ROW EXECUTE FUNCTION webdock_auth.fixture_reject_recovery_audit()`);
    await assert.rejects(recoverNeonEnrollment(client, database, origin, review));
    assert.equal(await mappings(), 0, "failed audit must roll back the binding");
    await admin.query("DROP TRIGGER fixture_reject_recovery_audit ON webdock_auth.neon_enrollment_recovery; DROP FUNCTION webdock_auth.fixture_reject_recovery_audit()");
    const other = await pool.connect();
    try {
      const results = await Promise.allSettled([recoverNeonEnrollment(client, database, origin, review), recoverNeonEnrollment(other, database, origin, review)]);
      assert.equal(results.filter(r => r.status === "fulfilled" && r.value.status === "recovered").length, 1);
    } finally { other.release(); }
    assert.equal((await recoverNeonEnrollment(client, database, origin, review)).status, "already-recovered");
    assert.deepEqual(await preserved(), before);
    assert.deepEqual((await admin.query("SELECT owner_role,reviewer_ref FROM webdock_auth.neon_enrollment_recovery")).rows, [{ owner_role: "neondb_owner", reviewer_ref: "disposable-fixture-owner" }]);
    for (const sql of ["UPDATE webdock_auth.neon_enrollment_recovery SET reviewer_ref='rewritten'", "DELETE FROM webdock_auth.neon_enrollment_recovery", "TRUNCATE webdock_auth.neon_enrollment_recovery"]) {
      await assert.rejects(admin.query(sql), (error: any) => error.code === "23514");
    }
    await client.query("SET ROLE webdock_platform_owner");
    try { await assert.rejects(inspectNeonEnrollmentRecovery(client, database, origin, target), "incoming SET ROLE must be rejected"); }
    finally { await client.query("RESET ROLE"); }
    await admin.query("SET SESSION AUTHORIZATION webdock_auth_bootstrap");
    try {
      await assert.rejects(inspectNeonEnrollmentRecovery(admin, database, origin, target), "incoming runtime login must be rejected before role assumption");
      await assert.rejects(admin.query("SELECT * FROM webdock_auth.neon_enrollment_recovery"), (error: any) => error.code === "42501");
    } finally { await admin.query("RESET SESSION AUTHORIZATION"); }
    await admin.query("UPDATE webdock_auth.neon_identity SET enabled=false,revision=2 WHERE user_id='101'");
    await assert.rejects(recoverNeonEnrollment(client, database, origin, review));
  } finally {
    server.closeAllConnections();
    if (server.listening) await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    client.release(); admin.release();
    await Promise.all([pool.end(), bootstrapPool.end()]);
  }
});
