import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { Client } from "pg";
import type { PoolClient } from "pg";
import {
  inspectNeonEnrollmentRecovery,
  recoverNeonEnrollment,
  neonRecoveryConfig,
} from "./neon-enrollment-recovery";
import { snapshotDigest } from "./retained-neon";

const origin = "https://ep-owned.neonauth.c-7.eu-central-1.aws.neon.tech/neondb/auth";
const target = { userID: "101", providerSubject: "2b28db90-addd-431e-82f1-de0657dbea6e", providerOrigin: origin };
const state = () => ({
  core: { id: "101", email: "operator@example.invalid", role: "operator", verified: true, banned: false, local: true, mfa: true, mustChangePassword: false, credentials: [{ id: "201", digest: "a".repeat(64), usable: true }] },
  provider: { id: target.providerSubject, email: "operator@example.invalid", verified: false, banned: false, credentials: [{ providerId: "credential", digest: "b".repeat(64), usable: true }] },
  mappings: [],
});
const review = () => ({ ...target, snapshotSha256: snapshotDigest(state()), evidenceRef: "incident:owned-orphan-1", evidenceSha256: "c".repeat(64), reviewerRef: "owner-reviewed-incident-1" });

// PostgreSQL itself is exercised separately; this double isolates admission and transaction decisions.
function database(options: { role?: string; active?: string; name?: string; value?: any; audit?: any[]; failAudit?: boolean; platformLogin?: boolean; platformSuper?: boolean; platformBypass?: boolean } = {}) {
  const queries: string[] = [];
  let value = structuredClone(options.value ?? state());
  let audit = structuredClone(options.audit ?? []);
  let saved: { value: any; audit: any[] } | undefined;
  const db = { async query(sql: string, params: any[] = []) {
    queries.push(sql);
    if (sql.startsWith("BEGIN")) saved = structuredClone({ value, audit });
    if (sql === "ROLLBACK" && saved) ({ value, audit } = saved);
    if (sql.includes("session_user AS role")) return { rows: [{ role: options.role ?? "neondb_owner", active: options.active ?? options.role ?? "neondb_owner", database: options.name ?? "owned_test" }] };
    if (sql.includes("r.rolcanlogin")) return { rows: [{ active: "webdock_platform_owner", rolcanlogin: options.platformLogin ?? false, rolsuper: options.platformSuper ?? false, rolbypassrls: options.platformBypass ?? false }] };
    if (sql.includes("AS state")) return { rows: [{ state: structuredClone(value) }] };
    if (sql.startsWith("SELECT") && sql.includes("neon_enrollment_recovery")) return { rows: structuredClone(audit) };
    if (sql.startsWith("INSERT INTO webdock_auth.neon_identity")) {
      value.mappings.push({ user_id: params[0], provider_origin: params[1], provider_subject: params[2], revision: 1, enabled: true, linked_by: "offline-owner:neondb_owner", linked_at: "2026-11-01T12:00:00+00:00" });
    }
    if (sql.startsWith("INSERT INTO webdock_auth.neon_enrollment_recovery")) {
      if (options.failAudit) throw Error("fixture audit failure");
      audit.push({ user_id: params[0], provider_origin: params[1], provider_subject: params[2], snapshot_sha256: params[3], mapping_sha256: params[4], evidence_ref: params[5], evidence_sha256: params[6], reviewer_ref: params[7], owner_role: "neondb_owner" });
    }
    return { rows: [], rowCount: 1 };
  } } as unknown as PoolClient;
  return { db, queries, current: () => ({ value, audit }) };
}

test("owner recovery preserves a sole operator and records one exact mapping without granting a session", async () => {
  const fixture = database();
  const inspected = await inspectNeonEnrollmentRecovery(fixture.db, "owned_test", origin, target);
  assert.equal(inspected.snapshotSha256, review().snapshotSha256);
  assert.equal(JSON.stringify(inspected).includes("password"), false);
  const result = await recoverNeonEnrollment(fixture.db, "owned_test", origin, review());
  assert.equal(result.status, "recovered");
  assert.deepEqual(fixture.current().value.core, state().core);
  assert.deepEqual(fixture.current().value.provider, state().provider);
  assert.equal(fixture.current().audit.length, 1);
  assert.equal(fixture.current().value.mappings[0].provider_subject, target.providerSubject);
  assert.ok(fixture.queries.findIndex(sql => sql.includes("session_user AS role")) < fixture.queries.indexOf("SET LOCAL ROLE webdock_platform_owner"));
  assert.equal(fixture.queries.some(sql => /^INSERT INTO webdock_auth\.session\b|^UPDATE\b/.test(sql)), false);
  assert.equal((await recoverNeonEnrollment(fixture.db, "owned_test", origin, review())).status, "already-recovered");
  await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, { ...review(), evidenceRef: "another-review" }));
});

test("runtime credentials, SET ROLE and another database cannot perform owner recovery", async () => {
  for (const options of [{ role: "webdock_auth_bootstrap" }, { role: "webdock_offline_migrate", active: "neondb_owner" }, { active: "webdock_platform_owner" }, { name: "another_test" }]) {
    const fixture = database(options);
    await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, review()));
    assert.equal(fixture.current().audit.length, 0);
    assert.equal(fixture.current().value.mappings.length, 0);
    assert.equal(fixture.queries.includes("SET LOCAL ROLE webdock_platform_owner"), false);
  }
});

test("recovery refuses a login-enabled, superuser or RLS-bypassing platform owner", async () => {
  for (const options of [{ platformLogin: true }, { platformSuper: true }, { platformBypass: true }]) {
    const fixture = database(options);
    await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, review()));
    assert.equal(fixture.current().value.mappings.length, 0);
  }
});

test("changed credentials, admission and exact identity invalidate the reviewed snapshot", async () => {
  for (const mutate of [
    (v: any) => v.core.credentials[0].digest = "d".repeat(64),
    (v: any) => v.provider.credentials[0].digest = "d".repeat(64),
    (v: any) => v.core.banned = true,
    (v: any) => v.core.local = false,
    (v: any) => v.core.verified = false,
    (v: any) => v.core.role = "unapproved",
    (v: any) => v.core.mfa = false,
    (v: any) => v.provider.banned = true,
    (v: any) => v.provider.email = "another@example.invalid",
    (v: any) => v.provider.id = "00000000-0000-0000-0000-000000000001",
    (v: any) => v.core.credentials.push(v.core.credentials[0]),
    (v: any) => v.provider.credentials = [],
    (v: any) => v.mappings.push({ user_id: "101", provider_subject: target.providerSubject, revision: 2, enabled: true }),
  ]) {
    const value = state(); mutate(value);
    const fixture = database({ value });
    await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, review()));
    assert.equal(fixture.current().audit.length, 0);
  }
});

test("invalid repair fields and foreign origins never reach persistence", async () => {
  for (const change of [{ userID: 101 }, { providerSubject: "not-a-uuid" }, { providerOrigin: origin + "?token=bad" }, { providerOrigin: origin.replace("neon.tech", "example.invalid") }, { providerOrigin: origin.replace("https://", "http://") }, { providerOrigin: "https://u:p@" + origin.slice(8) }, { evidenceSha256: "bad" }, { reviewerRef: "owner\nforged" }, { password: "must-not-be-accepted" }]) {
    const fixture = database();
    await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, { ...review(), ...change }));
    assert.equal(fixture.queries.length, 0);
  }
  await assert.rejects(recoverNeonEnrollment(database().db, "owned_test", origin.replace("ep-owned", "ep-other"), review()));
});

test("audit failure rolls back the identity mapping", async () => {
  const fixture = database({ failAudit: true });
  await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, review()));
  assert.equal(fixture.current().value.mappings.length, 0);
  assert.equal(fixture.current().audit.length, 0);
});

test("an exact committed retry rejects a later revision, disabled binding or changed owner audit", async () => {
  for (const mutate of [
    (v: any) => v.value.mappings[0].revision = 2,
    (v: any) => v.value.mappings[0].enabled = false,
    (v: any) => v.value.mappings[0].linked_at = "2026-11-02T12:00:00+00:00",
    (v: any) => v.audit[0].owner_role = "webdock_auth_bootstrap",
  ]) {
    const fixture = database();
    await recoverNeonEnrollment(fixture.db, "owned_test", origin, review());
    mutate(fixture.current());
    await assert.rejects(recoverNeonEnrollment(fixture.db, "owned_test", origin, review()));
    assert.equal(fixture.current().audit.length, 1);
  }
});

test("offline configuration pins owner, destination, verified transport and Managed origin", () => {
  const env = { DATABASE_URL_UNPOOLED: "postgresql://neondb_owner:private@127.0.0.1:5432/owned_test", NEON_AUTH_BASE_URL: origin };
  assert.equal(neonRecoveryConfig(env, "127.0.0.1:5432/owned_test").database, "owned_test");
  for (const changed of [{ DATABASE_URL_UNPOOLED: env.DATABASE_URL_UNPOOLED.replace("neondb_owner", "webdock_auth_bootstrap") }, { DATABASE_URL_UNPOOLED: env.DATABASE_URL_UNPOOLED.replace("127.0.0.1", "db.example.invalid") }, { NEON_AUTH_BASE_URL: origin + "/" }]) {
    assert.throws(() => neonRecoveryConfig({ ...env, ...changed }, "127.0.0.1:5432/owned_test"));
  }
  assert.throws(() => neonRecoveryConfig(env, "127.0.0.1:5432/other_test"));
});

test("owner recovery rejects URL query overrides before selecting a database transport", () => {
  const parameters=(client:Client)=>(client as Client & {connectionParameters:{host:string;port:number;database:string}}).connectionParameters;
  const base="postgresql://neondb_owner:synthetic@127.0.0.1:5432/owned_test";
  const redirected=new Client({connectionString:base+"?host=remote.example.invalid&port=6432"});
  assert.equal(parameters(redirected).host,"remote.example.invalid");
  assert.equal(parameters(redirected).port,6432);
  for(const query of ["host=remote.example.invalid&port=6432","%68ost=remote.example.invalid","user=another_owner","dbname=another_test","sslmode=disable","options=-c%20role%3Danother_owner"]){
    assert.throws(()=>neonRecoveryConfig({DATABASE_URL_UNPOOLED:base+"?"+query,NEON_AUTH_BASE_URL:origin},"127.0.0.1:5432/owned_test"));
  }
  const safe=neonRecoveryConfig({DATABASE_URL_UNPOOLED:base,NEON_AUTH_BASE_URL:origin},"127.0.0.1:5432/owned_test");
  const client=new Client({connectionString:safe.url.href});
  assert.equal(parameters(client).host,"127.0.0.1");
  assert.equal(parameters(client).database,safe.database);
});

test("the reviewed default port cannot be replaced by an ambient PGPORT",()=>{
  const previous=process.env.PGPORT;
  try {
    process.env.PGPORT="6543";
    const config=neonRecoveryConfig({DATABASE_URL_UNPOOLED:"postgresql://neondb_owner:synthetic@127.0.0.1/owned_test",NEON_AUTH_BASE_URL:origin},"127.0.0.1:5432/owned_test");
    const client=new Client({connectionString:config.url.href}) as Client & {connectionParameters:{port:number}};
    assert.equal(client.connectionParameters.port,5432);
  } finally {if(previous===undefined)delete process.env.PGPORT;else process.env.PGPORT=previous;}
});

test("the CLI rejects runtime custody without printing the supplied credential", () => {
  const result = spawnSync(process.execPath, ["--no-env-file", new URL("./neon-enrollment-recovery-cli.ts", import.meta.url).pathname, "inspect", "127.0.0.1:5432/owned_test", "nonexistent-review.json"], {
    encoding: "utf8",
    env: { PATH: process.env.PATH, DATABASE_URL_UNPOOLED: "postgresql://webdock_auth_bootstrap:fixture-secret-must-not-print@127.0.0.1:5432/owned_test", NEON_AUTH_BASE_URL: origin },
  });
  assert.equal(result.status, 1);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, /Offline Neon enrollment recovery failed/);
  assert.equal((result.stdout + result.stderr).includes("fixture-secret-must-not-print"), false);
  assert.equal((result.stdout + result.stderr).includes("postgresql://"), false);
});
