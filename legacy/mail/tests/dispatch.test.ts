import test from 'node:test';
import assert from 'node:assert/strict';
import {dispatchOne} from '../src/sending/dispatch.ts';
import {RelayAPIFailure} from '../src/sending/relay-api.ts';
const job={id:'123',customerID:'456',sourceID:'key:789',payloadRef:crypto.randomUUID(),payloadBytes:100,requestHash:'a'.repeat(64),payloadFormat:'request' as const,sourceContext:{},route:{kind:'direct'},envelope:{},leaseToken:crypto.randomUUID()};
const message={sender:{email:'a@example.test'},to:['b@example.test'],subject:'Test',raw:Buffer.from('Subject: Test\r\n\r\nHello')};
const authorization={kind:'direct' as const,account:{region:'eu' as const,consumerKey:'test',consumerSecret:'test'}};
test('dispatch persists the handoff before exactly one transport attempt and records uncertainty',async()=>{
 const order:string[]=[];
 const control={claim:async()=>job,begin:async()=>{order.push('begin');return authorization;},finish:async(_job:unknown,result:unknown)=>{order.push('finish');assert.deepEqual(result,{state:'unknown'});return true;}};
 const worked=await dispatchOne(control,async()=>{order.push('load');return message;},{relay:async()=>{order.push('send');throw new RelayAPIFailure(true);}});
 assert.equal(worked,true);assert.deepEqual(order,['load','begin','send','finish']);
});
test('revoked authorization or a lost begin response never reaches the network',async()=>{
 for(const begin of [async()=>null,async()=>{throw Error('lost reply');}]){
  let calls=0;await dispatchOne({claim:async()=>job,begin,finish:async()=>{throw Error('must not finish');}},async()=>message,{relay:async()=>{calls++;return {providerID:'1'};}}).catch(()=>{});assert.equal(calls,0);
 }
});
test('accepted and definite rejection receipts are distinct; lost completion never retries sending',async()=>{
 for(const reject of [false,true]){
  let calls=0;
  await assert.rejects(dispatchOne({claim:async()=>job,begin:async()=>authorization,finish:async(_job,result)=>{assert.equal(result.state,reject?'rejected':'accepted');throw Error('receipt database unavailable');}},async()=>message,{relay:async()=>{calls++;if(reject)throw new RelayAPIFailure(false);return {providerID:'1'};}}));
  assert.equal(calls,1);
 }
});

test('unreadable private payload is held for review before credentials or network handoff',async()=>{
 let rejected=0,begun=0;
 await assert.rejects(dispatchOne({claim:async()=>job,prepareFailed:async()=>{rejected++;return true;},begin:async()=>{begun++;return authorization;},finish:async()=>{throw Error('Must not finish a handoff');}},async()=>{throw Error('corrupt private payload');}));
 assert.equal(rejected,1);assert.equal(begun,0);
});
