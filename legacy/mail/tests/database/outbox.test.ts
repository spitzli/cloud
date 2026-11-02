import test from 'node:test';
import assert from 'node:assert/strict';
import {Pool} from 'pg';
import {outboxSchemaSQL,enqueueMail,claimSend,beginHandoff,finishHandoff,cancelSend} from '@webdock/mail-core/outbox';
const url=new URL(process.env.DATABASE_URL!);
if(url.hostname!=='127.0.0.1' || url.pathname!=='/webdock_admin_test' || process.env.AUTH_TEST_MAIL!=='true')throw Error('Disposable local database required');
const pool=new Pool({connectionString:url.toString()});
test.after(()=>pool.end());
async function tx<T>(run:(db:import('pg').PoolClient)=>Promise<T>){const db=await pool.connect();try{await db.query('BEGIN');const result=await run(db);await db.query('COMMIT');return result;}catch(error){await db.query('ROLLBACK');throw error;}finally{db.release();}}
test('durable outbox fences concurrent workers, idempotent requests and uncertain handoffs',async()=>{
 await pool.query(outboxSchemaSQL);
 const customer=(await pool.query("INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Outbox fixture') RETURNING id")).rows[0].id;
 const input={customerID:customer,sourceID:'key:fixture',idempotencyKey:'same-request',requestHash:'a'.repeat(64),payloadRef:crypto.randomUUID(),payloadBytes:100,route:{kind:'direct',revision:'1'}};
 try{
  const queued=await tx(db=>enqueueMail(db,input));
  const again=await tx(db=>enqueueMail(db,input));assert.equal(again.id,queued.id);assert.equal(again.created,false);
  await assert.rejects(tx(db=>enqueueMail(db,{...input,requestHash:'b'.repeat(64)})));
  const claims=await Promise.all(Array.from({length:8},()=>tx(db=>claimSend(db))));
  const held=claims.filter(Boolean);assert.equal(held.length,1);const first=held[0]!;
  await pool.query("UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",[queued.id]);
  const second=await tx(db=>claimSend(db));assert.ok(second);assert.notEqual(second.leaseToken,first.leaseToken);
  assert.equal(await tx(db=>beginHandoff(db,first)),false);
  assert.equal(await tx(db=>beginHandoff(db,second)),true);
  assert.equal(await tx(db=>beginHandoff(db,second)),false);
  assert.equal(await tx(db=>cancelSend(db,customer,queued.id)),false);
  await pool.query("UPDATE webdock_mail.send_job SET lease_until=now()-interval '1 second' WHERE id=$1",[queued.id]);
  assert.equal(await tx(db=>claimSend(db)),null);
  assert.equal((await pool.query('SELECT state FROM webdock_mail.send_job WHERE id=$1',[queued.id])).rows[0].state,'unknown');
  assert.equal(await tx(db=>finishHandoff(db,first,{state:'accepted',providerID:'12'})),false);
  assert.equal(await tx(db=>finishHandoff(db,second,{state:'accepted',providerID:'12'})),true);
  assert.equal(await tx(db=>finishHandoff(db,second,{state:'accepted',providerID:'12'})),true);
  assert.equal(await tx(db=>finishHandoff(db,second,{state:'rejected'})),false);
  const canceled=await tx(db=>enqueueMail(db,{...input,idempotencyKey:'cancel',payloadRef:crypto.randomUUID()}));
  assert.equal(await tx(db=>cancelSend(db,'123',canceled.id)),false);
  assert.equal(await tx(db=>cancelSend(db,customer,canceled.id)),true);
  assert.equal(await tx(db=>claimSend(db)),null);
  const future=await tx(db=>enqueueMail(db,{...input,idempotencyKey:'future',payloadRef:crypto.randomUUID(),notBefore:new Date(Date.now()+60000)}));
  assert.equal(await tx(db=>claimSend(db)),null);assert.equal(await tx(db=>cancelSend(db,customer,future.id)),true);
 }finally{
  await pool.query('DELETE FROM webdock_mail.send_job WHERE customer_id=$1',[customer]);
  const mapping=(await pool.query('DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1 RETURNING organization_id',[customer])).rows[0];
  await pool.query('DELETE FROM webdock_admin.customers WHERE id=$1',[customer]);await pool.query('DELETE FROM webdock_auth.organization WHERE id=$1',[mapping.organization_id]);
 }
});
