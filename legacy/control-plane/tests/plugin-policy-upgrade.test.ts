import { test } from "node:test";
import assert from "node:assert/strict";
import { buildDatabaseRls } from "../src/db/database-rls-schema";

test("retained plugin documents stay denied until invocation policies can be installed", () => {
  const plan = buildDatabaseRls([
    { schema: "webdock_auth", name: "plugin_document", columns: ["plugin_id", "customer_id", "project_id", "key", "value", "revision"] },
    { schema: "webdock_auth", name: "plugin_document_receipt", columns: ["plugin_id", "customer_id", "project_id", "operation_id", "key", "revision"] },
    { schema: "webdock_auth", name: "hosting_plugin_provisioning", columns: ["id", "plugin_id", "customer_id", "project_id", "setup_id"] },
    { schema: "webdock_auth", name: "hosting_plugin_provisioning_request", columns: ["id", "plugin_id", "customer_id", "project_id"] },
  ]);
  assert.deepEqual(plan.unknownTables, []);
  assert.doesNotMatch(plan.sql, /webdock_rls\.plugin_allows\(/);
  assert.doesNotMatch(plan.sql, /webdock_rls\.plugin_context\(/);
  assert.match(plan.sql, /ENABLE ROW LEVEL SECURITY/);
});

test("native identity security tables remain authentication-only during policy reinstallation", () => {
  const names = ["identity_mfa_challenge", "identity_totp_replay", "identity_mfa_enrollment", "identity_recovery_token", "identity_delivery_job", "identity_passkey_ceremony"];
  const plan = buildDatabaseRls(names.map(name => ({ schema: "webdock_auth", name, columns: ["id", "user_id"] })));
  assert.deepEqual(plan.unknownTables, []);
  for (const name of names) {
    assert.match(plan.sql, new RegExp(`GRANT SELECT,INSERT,UPDATE,DELETE ON "webdock_auth"\\."${name}" TO webdock_auth_bootstrap`));
    for (const role of ["webdock_auth_runtime", "webdock_worker_runtime"]) {
      assert.ok(plan.sql.includes(`CREATE POLICY ${role}_read ON "webdock_auth"."${name}" FOR SELECT TO ${role} USING(false)`));
      assert.ok(plan.sql.includes(`CREATE POLICY ${role}_insert ON "webdock_auth"."${name}" FOR INSERT TO ${role} WITH CHECK(false)`));
    }
  }
});

test("policy reinstallation cannot let agent control enroll or revoke its own transport", () => {
  const plan=buildDatabaseRls([
    {schema:"webdock_auth",name:"agent_transport_enrollment",columns:["installation","agent_id","generation","cluster_id","credential_hash","transport","unix_uid","certificate_sha256","revoked","last_sequence","last_seen"]},
    {schema:"webdock_auth",name:"agent_runtime_dispatch",columns:["operation_id","generation","installation","agent_id","agent_generation","request_sha256","command","receipt_sha256","completed_at","created_at"]},
  ]);
  assert.deepEqual(plan.unknownTables,[]);
  assert.ok(plan.sql.includes('GRANT SELECT,UPDATE(last_sequence,last_seen) ON "webdock_auth"."agent_transport_enrollment" TO webdock_auth_bootstrap'));
  assert.ok(!plan.sql.includes('GRANT SELECT,INSERT,UPDATE,DELETE ON "webdock_auth"."agent_transport_enrollment" TO webdock_auth_bootstrap'));
  assert.ok(plan.sql.includes('CREATE POLICY webdock_auth_bootstrap_insert ON "webdock_auth"."agent_transport_enrollment" FOR INSERT TO webdock_auth_bootstrap WITH CHECK(false)'));
});

test("repository service leases retain their distinct worker-only admission purpose", () => {
  const plan = buildDatabaseRls([{schema:"webdock_auth",name:"repository_lease",columns:["id","customer_id","project_id","plugin_id","source_id","expires_at","state"]}]);
  assert.deepEqual(plan.unknownTables, []);
  assert.ok(plan.sql.includes("repository-admission"));
  assert.ok(plan.sql.includes('CREATE POLICY webdock_auth_runtime_read ON "webdock_auth"."repository_lease" FOR SELECT TO webdock_auth_runtime USING(false)'));
  assert.ok(!plan.sql.includes('GRANT SELECT,INSERT,UPDATE,DELETE ON "webdock_auth"."repository_lease" TO webdock_auth_bootstrap'));
});

test("tenant Mail provider approval keeps sending worker read-only and has no agent grant", () => {
  const plan=buildDatabaseRls([{schema:"webdock_mail",name:"provider_binding",columns:["customer_id","plugin_id","revision","transport_revision","revoked_at","updated_at"]}]);
  assert.deepEqual(plan.unknownTables,[]);
  assert.ok(plan.sql.includes('CREATE POLICY webdock_worker_runtime_read ON "webdock_mail"."provider_binding" FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY[\'mail-send-gateway\']::text[]))'));
  assert.ok(plan.sql.includes('REVOKE INSERT,UPDATE,DELETE ON "webdock_mail"."provider_binding" FROM webdock_worker_runtime'));
  assert.ok(plan.sql.includes('REVOKE DELETE ON "webdock_mail"."provider_binding" FROM webdock_auth_runtime'));
  assert.ok(!plan.sql.includes('webdock_rls.agent_customer("provider_binding"'));
});


test("retained policy installer preserves the frozen native invocation fence", async () => {
  const { readFile } = await import("node:fs/promises");
  const { pluginInvocationFenceSQL } = await import("../src/modules/plugins/scope-rls");
  const migration=await readFile(new URL("../../../crates/plugin-host/migrations/0001_invocation_fence.sql",import.meta.url),"utf8");
  assert.equal(pluginInvocationFenceSQL.trim(),migration.trim());
});

test("tenant Mail policy helper survives retained policy reinstallation", async () => {
 const { readFile } = await import("node:fs/promises");
 const { mailPluginPolicySQL } = await import("../src/modules/plugins/scope-rls");
 const migration=await readFile(new URL("../../../crates/plugin-host/migrations/0002_mail_policy.sql",import.meta.url),"utf8");
 assert.equal(mailPluginPolicySQL.trim(),migration.trim());
 const plan=buildDatabaseRls([{schema:"webdock_mail",name:"provider_binding",columns:["customer_id","plugin_id","revision","transport_revision","revoked_at"]}]);
 assert.ok(plan.sql.includes(mailPluginPolicySQL));
});

test("Mail provider attempts stay sending-only and cannot be pre-attempted or deleted", () => {
 const plan=buildDatabaseRls([{schema:"webdock_mail",name:"provider_lease",columns:["id","customer_id","job_id","attempted_at","revoked_at"]}]);
 assert.deepEqual(plan.unknownTables,[]);
 assert.ok(plan.sql.includes('REVOKE ALL ON "webdock_mail"."provider_lease" FROM webdock_auth_runtime,webdock_auth_bootstrap,webdock_registry_runtime'));
 assert.ok(plan.sql.includes('REVOKE DELETE ON "webdock_mail"."provider_lease" FROM webdock_worker_runtime'));
 assert.ok(plan.sql.includes('"provider_lease"."attempted_at"::text IS NULL'));
 assert.ok(plan.sql.includes("CREATE POLICY webdock_worker_runtime_read ON \"webdock_mail\".\"provider_lease\" FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['mail-send-gateway']::text[]))"));
 assert.ok(!plan.sql.includes('webdock_rls.agent_customer("provider_lease"'));
});
