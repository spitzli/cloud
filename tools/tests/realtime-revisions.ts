/** Runs only against the explicitly disposable local native fixture, inside a rolled-back transaction. */
import {Pool} from 'pg';
import assert from 'node:assert/strict';
const url=new URL(process.env.SPITZLI_TEST_MIGRATION_DATABASE_URL??'');
assert.equal(url.hostname,'127.0.0.1');assert.equal(url.pathname,'/webdock_rls_test');
const db=new Pool({connectionString:url.href,max:1});
try{
 await db.query('BEGIN');
 const before=(await db.query('SELECT topic,revision::text FROM webdock_live.change_revision ORDER BY topic')).rows;
 assert.deepEqual(before.map(x=>x.topic),['operator-customers','workspace']);
 await db.query('UPDATE webdock_auth.member SET role=role WHERE false');
 const after=(await db.query('SELECT topic,revision::text FROM webdock_live.change_revision ORDER BY topic')).rows;
 for(let i=0;i<before.length;i++)assert.equal(BigInt(after[i].revision),BigInt(before[i].revision)+1n);
 await db.query('ROLLBACK');
 const runtimeUrl=new URL(process.env.DATABASE_URL??'');
 assert.equal(runtimeUrl.hostname,url.hostname);assert.equal(runtimeUrl.port,url.port);assert.equal(runtimeUrl.pathname,url.pathname);assert.equal(runtimeUrl.username,'webdock_auth_runtime');
 const runtime=new Pool({connectionString:runtimeUrl.href,max:1});
 try{
  await assert.rejects(runtime.query("UPDATE webdock_live.change_revision SET revision=0"),(e:any)=>e.code==='42501');
  await assert.rejects(runtime.query('SELECT * FROM webdock_live.change_revision'),(e:any)=>e.code==='42501');
 }finally{await runtime.end()}
 console.log('Revision triggers invalidate both fixed topics; runtime roles cannot read or tamper with counters; all mutations rolled back.');
}finally{await db.query('ROLLBACK');await db.end()}
