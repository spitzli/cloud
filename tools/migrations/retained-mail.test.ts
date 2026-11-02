import { test } from "node:test";
import assert from "node:assert/strict";
import type { PoolClient } from "pg";
import { adoptRetainedMail } from "./retained-mail";

test("retained Mail adoption rejects ordinary runtime and superuser authority before reading keys or changing tables", async () => {
  for (const identity of [
    { database: "platform_test", role: "webdock_auth_bootstrap", active: "webdock_auth_bootstrap", superuser: false, bypass: false },
    { database: "platform_test", role: "postgres", active: "postgres", superuser: true, bypass: true },
    { database: "wrong", role: "webdock_offline_migrate", active: "webdock_offline_migrate", superuser: false, bypass: false },
    { database: "platform_test", role: "webdock_offline_migrate", active: "webdock_offline_migrate", superuser: true, bypass: false },
    { database: "platform_test", role: "webdock_offline_migrate", active: "webdock_offline_migrate", superuser: false, bypass: true },
  ]) {
    const calls: string[] = [];
    const client = { query: async (sql: string) => {
      calls.push(sql);
      return { rows: sql.includes("session_user") ? [identity] : [] };
    }} as unknown as PoolClient;
    await assert.rejects(adoptRetainedMail(client, "platform_test", "existing-context-key-at-least-32-bytes"), /dedicated offline login and exact database/);
    assert.equal(calls.at(-1), "ROLLBACK");
    assert.ok(!calls.some(sql => sql.includes("context_key") || sql.includes("CREATE TABLE")));
  }
});

test("a partial native upgrade is refused before optional Mail creation or broad policy reinstall", async () => {
  const calls: string[] = [];
  const client = { query: async (sql: string) => {
    calls.push(sql);
    if (sql.includes("session_user")) return {rows:[{database:"platform_test",role:"webdock_offline_migrate",active:"webdock_offline_migrate",superuser:false,bypass:false}]};
    if (sql.includes("native_present")) return {rows:[{native_present:true}]};
    return {rows:[]};
  }} as unknown as PoolClient;
  await assert.rejects(adoptRetainedMail(client, "platform_test", "existing-context-key-at-least-32-bytes"), /before any native migration/);
  assert.equal(calls.at(-1), "ROLLBACK");
  assert.ok(!calls.some(sql => sql.includes("CREATE TABLE")));
});

test("retained adoption never invents a missing signing key or trusts foreign provenance", async () => {
  for (const changed of [{key_count:0}, {key_matches:false}, {schema_owner:"postgres"}, {key_owner:"postgres"}, {prerequisites:false}]) {
    const calls: string[]=[];
    const client={query:async(sql:string)=>{
      calls.push(sql);
      if(sql.includes("session_user"))return {rows:[{database:"platform_test",role:"webdock_offline_migrate",active:"webdock_offline_migrate",superuser:false,bypass:false}]};
      if(sql.includes("native_present"))return {rows:[{native_present:false}]};
      if(sql.includes("schema_owner"))return {rows:[{schema_owner:"webdock_platform_owner",journal_owner:"webdock_platform_owner",key_owner:"webdock_policy_owner",key_count:1,key_matches:true,prerequisites:true,...changed}]};
      return {rows:[]};
    }} as unknown as PoolClient;
    await assert.rejects(adoptRetainedMail(client,"platform_test","existing-context-key-at-least-32-bytes"),/Owned retained history/);
    assert.equal(calls.at(-1),"ROLLBACK");
    assert.ok(!calls.some(sql=>sql.includes("CREATE TABLE")));
  }
});
