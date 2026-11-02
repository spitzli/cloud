import {test} from 'node:test';
import assert from 'node:assert/strict';
import type {RealtimeClient,SealedLiveQuery} from '@neon/realtime/client';
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
 let update!:()=>void;let changed=0,unsubscribed=0,closed=0;
 const subscription={onUpdate:(fn:()=>void)=>{update=fn;return()=>undefined},renew:()=>Promise.resolve(),unsubscribe:()=>{unsubscribed++}};
 const abort=new AbortController();
 const client={subscribe:()=>subscription,close:()=>{closed++}} as unknown as Client;
 await startLiveInvalidation({client,capability:()=>Promise.resolve(sealed()),changed:()=>{changed++},signal:abort.signal});
 update();assert.equal(changed,1);abort.abort();assert.equal(unsubscribed,1);assert.equal(closed,1);
});
await test('temporary capability outages recover while permission denial stops renewal',async()=>{
 let attempts=0,attached=0,closed=0;const abort=new AbortController();
 const client={subscribe:()=>{attached++;return {onUpdate:()=>()=>undefined,renew:()=>Promise.resolve(),unsubscribe:()=>undefined}},close:()=>{closed++}} as unknown as Client;
 await startLiveInvalidation({client,signal:abort.signal,changed:()=>undefined,capability:()=>++attempts===1?Promise.reject(Object.assign(Error('temporary'),{status:503})):Promise.resolve(sealed())});
 await new Promise(resolve=>setTimeout(resolve,1200));assert.equal(attempts,2);assert.equal(attached,1);assert.equal(closed,0);abort.abort();
 const denied=new AbortController();let deniedAttempts=0;
 await startLiveInvalidation({client,signal:denied.signal,changed:()=>undefined,capability:()=>{deniedAttempts++;return Promise.reject(Object.assign(Error('denied'),{status:403}))}});
 await new Promise(resolve=>setTimeout(resolve,1100));assert.equal(deniedAttempts,1);assert.equal(closed,2);denied.abort();
});
