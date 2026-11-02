/** Read-only startup guard regression, disposable operator fixture only. */
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { Pool } from "pg";
import { spawnSync } from "node:child_process";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { withDatabaseScope } from "@webdock/database-runtime";
const target = new URL(process.env.CORE_INVENTORY_OWNER_URL || "");
if (process.env.WEBDOCK_TEST_OPERATOR !== "true" || target.hostname !== "127.0.0.1" || target.pathname !== "/webdock_operator_test") throw Error("Disposable operator inventory fixture required");
const { verifyModuleShutdown, assertCoreInventoryEmpty } = await import("../../legacy/control-plane/src/module-preflight");
const { databasePools } = await import("../../legacy/control-plane/src/db/runtime");
const owner = new Pool({connectionString:target.href,max:1});
try {
  await verifyModuleShutdown();
  await withDatabaseScope({kind:"system",purpose:"api-scheduler"}, () => assert.rejects(assertCoreInventoryEmpty(databasePools.worker), /verified installation preflight read authority/));
  await owner.query("INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version) VALUES('mail','builtin','enabled','enabled','1.0.0')");
  await assert.rejects(verifyModuleShutdown(), /builtin|retains resources/);
  await owner.query("UPDATE webdock_auth.plugin_installation SET state='disabled',desired_state='draining' WHERE id='mail'");
  await assert.rejects(verifyModuleShutdown(), /builtin|retains resources/);
  await owner.query("UPDATE webdock_auth.plugin_installation SET state='disabled',desired_state='disabled' WHERE id='mail'");
  await verifyModuleShutdown();
  const customer=(await owner.query("INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Core inventory fixture') RETURNING id")).rows[0].id;
  await owner.query("INSERT INTO webdock_auth.tenant_storage(id,customer_id,label,environment,store_id,prefix,encrypted_token) VALUES($1,$2,'retained fixture','preview',$1,'','fixture')",[randomUUID(),customer]);
  await assert.rejects(verifyModuleShutdown(), /storage.*retains resources/);
  const reservation=createServer();
  await new Promise<void>(resolve=>reservation.listen(0,"127.0.0.1",resolve));
  const startupPort=(reservation.address() as {port:number}).port;
  await new Promise<void>(resolve=>reservation.close(()=>resolve()));
  const startup=spawnSync(process.execPath,["--no-env-file","legacy/control-plane/src/core.ts"],{
    cwd:fileURLToPath(new URL("../../",import.meta.url)),encoding:"utf8",timeout:10000,
    env:{...process.env,PORT:String(startupPort),WEBDOCK_API_DOMAINS:"auth,portal,registry",WEBDOCK_API_ORIGIN:"https://api.example.test"},
  });
  assert.notEqual(startup.status,0);
  assert.match(startup.stderr,/Module storage retains resources/);
  try {
    await withDatabaseScope({kind:"system",purpose:"installation-preflight"}, () => databasePools.worker.query("UPDATE webdock_auth.tenant_storage SET label='unexpected mutation' WHERE customer_id=$1",[customer]));
  } catch(error) { assert.equal((error as {code: string}).code,"42501"); }
  assert.equal((await owner.query("SELECT label FROM webdock_auth.tenant_storage WHERE customer_id=$1",[customer])).rows[0].label,"retained fixture");
  await owner.query("DELETE FROM webdock_auth.tenant_storage WHERE customer_id=$1",[customer]);
  await owner.query("INSERT INTO webdock_mail.service(customer_id,instance_key,host_id,state) VALUES($1,$2,'fixture','suspended')",[customer,randomUUID()]);
  await assert.rejects(verifyModuleShutdown(), /mail.*retains resources/);
  await owner.query("DELETE FROM webdock_mail.service WHERE customer_id=$1",[customer]);
  await owner.query("INSERT INTO webdock_auth.hosting_turbosmtp(customer_id,label,encrypted_secret) VALUES($1,'fixture','retained')",[customer]);
  await assert.rejects(verifyModuleShutdown(), /mail.*retains resources/);
  await owner.query("DELETE FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",[customer]);
  await owner.query("INSERT INTO webdock_auth.hosting_cluster(id,name,provider,region,location_evidence) VALUES(webdock_auth.next_snowflake(),'retained fixture','k3s','eu','fixture')");
  await assert.rejects(verifyModuleShutdown(), /hosting.*retains resources/);
  await owner.query("DELETE FROM webdock_auth.hosting_cluster WHERE name='retained fixture'");
  const project=(await owner.query("INSERT INTO webdock_admin.projects(id,name,customer_id) VALUES(webdock_auth.next_snowflake(),'inventory fixture',$1) RETURNING id",[customer])).rows[0].id;
  await owner.query("INSERT INTO webdock_auth.database_binding(id,project_id,name,environment) VALUES(webdock_auth.next_snowflake(),$1,'fixture','development')",[project]);
  await assert.rejects(verifyModuleShutdown(), /databases.*retains resources/);
  await owner.query("DELETE FROM webdock_auth.database_binding WHERE project_id=$1",[project]);
  await verifyModuleShutdown();
  await owner.query("DROP POLICY installation_preflight_read ON webdock_auth.tenant_storage");
  await assert.rejects(verifyModuleShutdown(), /preflight RLS migration/);
  await owner.query("CREATE POLICY installation_preflight_read ON webdock_auth.tenant_storage FOR SELECT TO webdock_worker_runtime USING(webdock_rls.worker(ARRAY['installation-preflight']::text[]))");
  await verifyModuleShutdown();
  console.log("Core inventory rejects enabled/draining metadata, retained storage, suspended Mail, legacy Mail credentials, hosting infrastructure, legacy database bindings and missing read authority; actual core startup refuses the retained resource; its read scope cannot write; resolved empty inventory passes.");
} finally {await owner.end();await databasePools.close();}
