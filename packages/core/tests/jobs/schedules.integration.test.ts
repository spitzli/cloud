import test from 'node:test';import assert from 'node:assert/strict';import {randomUUID} from 'node:crypto';import {createPluginJobs} from '../../src/plugin-jobs.ts';
const port=Number(process.env.SPITZLI_TEST_REDIS_PORT);
test('periodic jobs do not evict completed idempotency records from the shared queue',{skip:!port,timeout:10000},async()=>{
 const scope={pluginID:'notes',customerID:'1',projectID:'2',runtimeGeneration:'1',grantRevision:'1'};let ticks=0;
 const service=createPluginJobs({prefix:'retention-'+randomUUID(),connection:{host:'127.0.0.1',port},authority:{authorize:async()=>({scope,jobs:['work','tick']}),canRun:async()=>true},execute:async(_s,name)=>{if(name==='tick')ticks++;return null;}});
 const worker=service.startWorker(),req=new Request('http://localhost');
 try{
  const first=await service.enqueue(req,{name:'work',payload:null,idempotencyKey:'retained-first'});let last=first;
  for(let i=0;i<105;i++)last=await service.enqueue(req,{name:'work',payload:null,idempotencyKey:'retained-'+i});
  for(let i=0;i<100&&(await service.get(req,last.id)).state!=='completed';i++)await new Promise(r=>setTimeout(r,20));
  assert.equal((await service.get(req,first.id)).state,'completed');
  await service.schedule(scope,'tick',60);
  for(let i=0;i<100&&ticks===0;i++)await new Promise(r=>setTimeout(r,20));assert.ok(ticks>0);await new Promise(r=>setTimeout(r,100));
  assert.equal((await service.get(req,first.id)).state,'completed');
 }finally{await worker.close();await service.close();}
});
