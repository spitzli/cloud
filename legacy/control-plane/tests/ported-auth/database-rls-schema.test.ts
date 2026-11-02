import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildDatabaseRls,
  type RlsTable,
} from "../../src/db/database-rls-schema";
const table = (
  name: string,
  columns: string[],
  schema = "webdock_auth",
): RlsTable => ({ schema, name, columns });

test("unknown tables are protected but have no application policies or grants", () => {
  const result = buildDatabaseRls([table("new_secret", ["customer_id"])]);
  assert.deepEqual(result.unknownTables, ["webdock_auth.new_secret"]);
  assert.match(
    result.sql,
    /ALTER TABLE "webdock_auth"\."new_secret" FORCE ROW LEVEL SECURITY/,
  );
  assert.doesNotMatch(
    result.sql,
    /CREATE POLICY webdock_auth_runtime_read ON "webdock_auth"\."new_secret"/,
  );
  assert.doesNotMatch(
    result.sql,
    /GRANT SELECT,INSERT,UPDATE,DELETE ON "webdock_auth"\."new_secret"/,
  );
});

test("tenant and project rows enforce both read and write tenancy and consistent parents", () => {
  const { sql, unknownTables } = buildDatabaseRls([
    table("hosting_database_offer", ["customer_id", "project_id"]),
  ]);
  assert.deepEqual(unknownTables, []);
  assert.match(
    sql,
    /customer_allowed\("hosting_database_offer"\."customer_id"::text\)/,
  );
  assert.match(
    sql,
    /WITH CHECK\(\(webdock_rls.customer_permission\(\"hosting_database_offer\"\.\"customer_id\"::text,'org:databases:manage'\)\) AND/,
  );
  assert.match(
    sql,
    /project_matches\("hosting_database_offer"\."project_id"::text,"hosting_database_offer"\."customer_id"::text\)/,
  );
});

test("child tables inherit actual parent visibility and require parent write authority", () => {
  const { sql } = buildDatabaseRls([
    table("hosting_database_preview", ["database_id"]),
  ]);
  assert.match(
    sql,
    /FROM "webdock_auth"\."hosting_database" parent WHERE parent\."id"::text="hosting_database_preview"\."database_id"::text/,
  );
  assert.match(sql, /webdock_rls.customer_permission\(parent.customer_id,'org:databases:manage'\)/);
});

test("signed context binds caller login, expiry and exact serialized payload", () => {
  const { sql } = buildDatabaseRls([]);
  assert.match(sql, /hmac\(raw,secret_value,'sha256'\)/);
  assert.match(sql, /envelope->>'aud' IS DISTINCT FROM session_user::text/);
  assert.match(
    sql,
    /expiry>extract\(epoch FROM statement_timestamp\(\)\)\+120/,
  );
  assert.match(sql, /envelope->>'v' IS DISTINCT FROM '1'/);
  assert.match(
    sql,
    /REVOKE ALL ON FUNCTION webdock_rls.request_scope\(\) FROM PUBLIC/,
  );
  assert.doesNotMatch(sql, /GRANT EXECUTE[^;]*webdock_browser_website/);
});

test("agent NULL cluster cannot turn a row-specific check into global authority", () => {
  const { sql } = buildDatabaseRls([]);
  assert.match(sql, /worker\(purposes text\[\], cluster text\)/);
  assert.match(sql, /cluster IS NOT NULL AND webdock_rls.worker\(purposes\)/);
  assert.doesNotMatch(sql, /cluster IS NULL OR/);
});

test("Mail keys cannot bypass tenant/connection checks or self-escalate sending permission", () => {
  const { sql } = buildDatabaseRls([
    table("mail_api_key", [
      "id",
      "customer_id",
      "connection_id",
      "permission",
      "revoked_at",
      "key_hash",
    ]),
  ]);
  assert.match(
    sql,
    /k.connection_id=webdock_rls.request_scope\(\)->>'connectionID'/,
  );
  assert.match(sql, /k.revoked_at IS NULL/);
  assert.match(
    sql,
    /mail_key\("mail_api_key"\."customer_id"::text,"mail_api_key"\."connection_id"::text,true,true\)/,
  );
});

test("archive has no runtime access and independent customer schemas are rejected", () => {
  const { sql } = buildDatabaseRls([
    table("retired_content", ["tenant_id"], "public"),
  ]);
  assert.doesNotMatch(
    sql,
    /CREATE POLICY webdock_auth_runtime_read ON "public"/,
  );
  assert.throws(
    () => buildDatabaseRls([table("content", [], "spitzli")]),
    /Out-of-scope/,
  );
  assert.throws(
    () => buildDatabaseRls([table("content", []), table("content", [])]),
    /Duplicate/,
  );
  assert.throws(
    () => buildDatabaseRls([table("tenant_customer", ["customer_id"])]),
    /Missing metadata columns/,
  );
});

test("webhook ingestion cannot mutate dispatch credentials or worker enrollment", () => {
  const { sql } = buildDatabaseRls([
    table("mail_connection_secret", ["customer_id", "connection_id"]),
    table("git_worker", ["credential_hash", "enabled"]),
    table("git_connection", ["customer_id"]),
  ]);
  const statements = sql.split(";");
  const credentialPolicy = statements.find((s) =>
    s.includes(
      'webdock_worker_runtime_insert ON "webdock_auth"."mail_connection_secret"',
    ),
  )!;
  assert.ok(credentialPolicy);
  assert.doesNotMatch(credentialPolicy, /mail-callback/);
  const workerPolicy = statements.find((s) =>
    s.includes('webdock_worker_runtime_insert ON "webdock_auth"."git_worker"'),
  )!;
  assert.ok(workerPolicy);
  assert.doesNotMatch(workerPolicy, /git-webhook/);
  const connectionPolicy = statements.find((s) =>
    s.includes(
      'webdock_worker_runtime_update ON "webdock_auth"."git_connection"',
    ),
  )!;
  assert.match(connectionPolicy, /git-webhook/);
});

test("read locks do not authorize updates unless the original row is writable", () => {
  const { sql } = buildDatabaseRls([
    table("tenant_storage", ["id", "customer_id"]),
  ]);
  assert.match(
    sql,
    /FOR UPDATE TO webdock_auth_runtime USING\(webdock_rls.customer_allowed\("tenant_storage"\."customer_id"::text\)\) WITH CHECK\(webdock_rls.customer_allowed\("tenant_storage"\."customer_id"::text,true\)\)/,
  );
  assert.match(sql, /SECURITY INVOKER SET search_path=pg_catalog,webdock_rls/);
  assert.match(
    sql,
    /IF pg_has_role\(current_user,'webdock_auth_runtime','USAGE'\) THEN IF coalesce\(\(webdock_rls.customer_allowed\(OLD\."customer_id"::text,true\)\)/,
  );
  assert.match(
    sql,
    /BEFORE UPDATE ON "webdock_auth"\."tenant_storage" FOR EACH ROW/,
  );
});

test("backend OLD guard uses maintenance authority, not fresh backend insertion authority", () => {
  const { sql } = buildDatabaseRls([
    table("hosting_database_backend", ["id", "cluster_id", "kind", "state"]),
  ]);
  const body = sql.slice(
    sql.indexOf('CREATE OR REPLACE FUNCTION webdock_rls."guard_update_'),
  );
  assert.match(body, /webdock_rls.backend_owned\(OLD\."id"::text,true\)/);
  assert.doesNotMatch(body, /OLD\."state"::text='provisioning'/);
});

test("managed viewer sessions are gateway-only and launches cannot be rewritten by users", () => {
  const { sql, unknownTables } = buildDatabaseRls([
    table("managed_database_launch", [
      "token_hash",
      "database_id",
      "backend_id",
      "database_revision",
      "credential_revision",
      "subject",
      "session_id",
      "expires_at",
      "consumed_at",
    ]),
    table("managed_database_session", [
      "token_hash",
      "id",
      "database_id",
      "backend_id",
      "database_revision",
      "credential_revision",
      "subject",
      "session_id",
      "expires_at",
    ]),
  ]);
  assert.deepEqual(unknownTables, []);
  assert.match(
    sql,
    /CREATE POLICY webdock_auth_runtime_insert ON "webdock_auth"\."managed_database_launch"[^;]*customer_permission\(d.customer_id,'org:databases:manage'\)/,
  );
  assert.match(
    sql,
    /CREATE POLICY webdock_auth_runtime_update ON "webdock_auth"\."managed_database_launch"[^;]*WITH CHECK\(false\)/,
  );
  assert.match(
    sql,
    /CREATE POLICY webdock_auth_runtime_insert ON "webdock_auth"\."managed_database_session"[^;]*WITH CHECK\(false\)/,
  );
  assert.match(
    sql,
    /CREATE POLICY webdock_worker_runtime_read ON "webdock_auth"\."managed_database_session"[^;]*database-gateway/,
  );
});
