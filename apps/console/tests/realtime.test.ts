import {test} from 'node:test';
import assert from 'node:assert/strict';
import type {RealtimeClient,SealedLiveQuery,RawLiveQueryUpdate} from '@neon/realtime/client';
import {startLiveInvalidation} from '../src/realtime';
type Client=Pick<RealtimeClient,'subscribe'|'close'>;
const sealed=():SealedLiveQuery<unknown>=>({capability:'opaque',queryFingerprint:'fixed',expiresAt:Date.now()+60000});
await test('late capabilities cannot attach after logout or leak into persistent storage',async()=>{
 let resolve!:(value:SealedLiveQuery<unknown>)=>void;let attached=0,closed=0;
 const query=new Promise<SealedLiveQuery<unknown>>(r=>{resolve=r});const abort=new AbortController();
 const running=startLiveInvalidation({client:{subscribe:()=>{attached++;throw Error('must not subscribe')},close:()=>{closed++}},capability:()=>query,changed:()=>undefined,signal:abort.signal});
 abort.abort();resolve(sealed());await running;
 assert.equal(attached,0);assert.equal(closed,1);
});
await test('revision events refresh authorized API data and unsubscribe on cleanup',async()=>{
 let update!:(value:RawLiveQueryUpdate<unknown>)=>void;let changed=0,unsubscribed=0,closed=0;
 const subscription={onUpdate:(fn:(value:RawLiveQueryUpdate<unknown>)=>void)=>{update=fn;return()=>undefined},renew:()=>Promise.resolve(),unsubscribe:()=>{unsubscribed++}};
 const abort=new AbortController();
 const client={subscribe:()=>subscription,close:()=>{closed++}} as unknown as Client;
 await startLiveInvalidation({client,capability:()=>Promise.resolve(sealed()),changed:()=>{changed++},signal:abort.signal});
 update({type:"rows",rows:[]});assert.equal(changed,1);abort.abort();assert.equal(unsubscribed,1);assert.equal(closed,1);
});
await test('temporary capability outages recover while permission denial stops renewal',async()=>{
 let attempts=0,attached=0,closed=0;const abort=new AbortController();
 const client={subscribe:()=>{attached++;return {onUpdate:()=>()=>undefined,renew:()=>Promise.resolve(),unsubscribe:()=>undefined}},close:()=>{closed++}} as unknown as Client;
 await startLiveInvalidation({client,signal:abort.signal,changed:()=>undefined,capability:()=>++attempts===1?Promise.reject(Object.assign(Error('temporary'),{status:503})):Promise.resolve(sealed())});
 await new Promise(resolve=>setTimeout(resolve,1200));assert.equal(attempts,2);assert.equal(attached,1);assert.equal(closed,0);abort.abort();
 const denied=new AbortController();let deniedAttempts=0,deniedChanges=0;
 await startLiveInvalidation({client,signal:denied.signal,changed:()=>{deniedChanges++},capability:()=>{deniedAttempts++;return Promise.reject(Object.assign(Error('denied'),{status:403}))}});
 await new Promise(resolve=>setTimeout(resolve,1100));assert.equal(deniedAttempts,1);assert.equal(deniedChanges,1);assert.equal(closed,2);denied.abort();
});

await test('empty transaction progress does not restart API reads, while real changes still refresh',async()=>{
 let update!:(value:RawLiveQueryUpdate<unknown>)=>void;let changed=0;
 const subscription={onUpdate:(fn:typeof update)=>{update=fn;return()=>undefined},renew:()=>Promise.resolve(),unsubscribe:()=>undefined};
 const client={subscribe:()=>subscription,close:()=>undefined} as unknown as Client;
 const abort=new AbortController();
 await startLiveInvalidation({client,capability:()=>Promise.resolve(sealed()),changed:()=>{changed++},signal:abort.signal});
 try {
  update({type:'changes',changes:[],batch:{txids:['1']}});
  assert.equal(changed,0,'A transaction with no query changes must not cancel or refetch API data');
  update({type:'rows',rows:[]});assert.equal(changed,1,'A full replacement remains authoritative');
  update({type:'changes',changes:[{type:'upsert',rowId:'counter',row:{revision:'2'}}],batch:{txids:['2']}});assert.equal(changed,2);
  update({type:'changes',changes:[{type:'remove',rowId:'counter'}],batch:{txids:['3']}});assert.equal(changed,3);
 } finally {abort.abort()}
 update({type:"rows",rows:[]});assert.equal(changed,3,"Stopped subscriptions cannot invalidate a later session");
});
