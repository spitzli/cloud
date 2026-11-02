import test from 'node:test';
import assert from 'node:assert/strict';
import {once} from 'node:events';
import {createMailAPI} from '../src/api/server.ts';
import {hookMessageDigest} from '@webdock/mail-core/submission-proof';
import type {PrivatePayloadStore} from '../src/sending/payload-store.ts';
test('private SMTP hook forwards only metadata, strips accepted proof and fails closed',async()=>{
 const token='wd_hook_123_'+'t'.repeat(43),proof='456.'+'p'.repeat(43);
 const message={context:{stage:'data',sasl:{login:'wd-submit@mail-123.example.test'},queue:{id:'abc'}},envelope:{from:{address:'a@example.test'},to:[{address:'b@example.test'}]},message:{headers:[['From','a@example.test'],['X-Webdock-Submission',proof]],contents:'Private body',size:120}};
 let calls=0,unavailable=false;
 const digest=hookMessageDigest(message.message);
 const api=createMailAPI(async command=>{if(command.action==='send.policy.auth')return {ok:true};calls++;if(unavailable)throw Error('Controller unavailable');const metadata=command.submission as {digest:string};assert.equal(JSON.stringify(command).includes('Private body'),false);assert.equal(command.action,'send.policy');return {accept:metadata.digest===digest};},{} as PrivatePayloadStore);
 api.listen(0,'127.0.0.1');await once(api,'listening');const origin=`http://127.0.0.1:${(api.address() as {port:number}).port}`;
 const send=(value:unknown,authorization='Bearer '+token)=>fetch(origin+'/internal/smtp/123',{method:'POST',headers:{Authorization:authorization,'Content-Type':'application/json'},body:JSON.stringify(value)});
 try{
  const accepted=await (await send(message)).json() as {action:string;modifications:unknown[]};assert.equal(accepted.action,'accept');assert.deepEqual(accepted.modifications,[{type:'deleteHeader',index:1,name:'X-Webdock-Submission'}]);
  assert.equal(((await (await send({...message,message:{...message.message,contents:'Changed body'}})).json()) as {action:string}).action,'reject');
  const before=calls;
  assert.equal((await send(message,'Bearer wd_hook_124_'+'t'.repeat(43))).status,401);
  assert.equal(((await (await send({...message,message:{...message.message,headers:[...message.message.headers,['X-Webdock-Submission',proof]]}})).json()) as {action:string}).action,'reject');
  assert.equal(calls,before);
  unavailable=true;assert.equal((await send(message)).status,503);
 }finally{api.closeAllConnections();await new Promise<void>(resolve=>api.close(()=>resolve()));}
});
