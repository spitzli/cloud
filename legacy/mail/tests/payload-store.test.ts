import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,readFile,rm,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {randomBytes} from 'node:crypto';
import {PrivatePayloadStore} from '../src/sending/payload-store.ts';
test('private payloads are encrypted, immutable and bound to both customer and reference',async()=>{
 const directory=await mkdtemp(join(tmpdir(),'webdock-payload-'));const key=randomBytes(32).toString('base64');
 const store=new PrivatePayloadStore(directory,key);
 try{
  const payload=Buffer.from('Private mail body and attachment');
  const saved=await store.put('123',payload);
  assert.equal(saved.bytes,payload.length);
  assert.deepEqual(await store.get('123',saved.ref),payload);
  const disk=await readFile(join(directory,saved.ref));assert.equal(disk.includes(payload),false);
  await assert.rejects(store.get('124',saved.ref));await assert.rejects(store.get('123','../../etc/passwd'));
  const wrongKey=new PrivatePayloadStore(directory,randomBytes(32).toString('base64'));
  await assert.rejects(wrongKey.get('123',saved.ref));
  await assert.rejects(wrongKey.remove(saved.ref));
  assert.deepEqual(await store.get('123',saved.ref),payload);
  disk[disk.length-2]^=1;await writeFile(join(directory,saved.ref),disk);
  await assert.rejects(store.get('123',saved.ref));
  await store.remove(saved.ref);await store.remove(saved.ref);
 }finally{await rm(directory,{recursive:true,force:true});}
});
