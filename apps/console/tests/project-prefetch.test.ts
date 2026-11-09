import { test } from 'node:test';
import assert from 'node:assert/strict';
import { QueryClient } from '@tanstack/react-query';
import { ApiClient,type Session } from '../src/api';
import { projectQuery,prefetchProjectSummary } from '../src/project-query';
import { retirePrivateCache } from '../src/session-cache';
const account=(id:string,extra:Partial<Session['user']>={}):Session=>({user:{id,name:id,email:`${id}@example.test`,role:'user',emailVerified:true,...extra},session:{id:`s-${id}`,userId:id,expiresAt:'2027-01-01T00:00:00Z'}});
const api=(fetcher:typeof fetch)=>new ApiClient({apiOrigin:'http://localhost',authOrigin:'http://localhost',enabledFeatures:[]},fetcher);
void test('project prefetch is scoped to current confirmed authority, deduplicated and consumed by the normal query',async()=>{
  const client=new QueryClient(),a=account('a'),b=account('b'),callsRef={n:0},http=api(()=>{callsRef.n++;return Promise.resolve(Response.json({id:'201',name:'Project',customer:{id:'101',name:'Alpha'},role:'owner'}))});client.setQueryData(['session'],{data:a});
  await Promise.all([prefetchProjectSummary(client,http,a,'201'),prefetchProjectSummary(client,http,a,'201')]);await client.query(projectQuery(http,a,'201'));assert.equal(callsRef.n,1);assert.notDeepEqual(projectQuery(http,a,'201').queryKey,projectQuery(http,b,'201').queryKey);const other=api(()=>Promise.resolve(Response.json({})));other.config={...http.config,workspaceApiOrigin:'https://another.example.test'};assert.notDeepEqual(projectQuery(http,a,'201').queryKey,projectQuery(other,a,'201').queryKey);
  client.setQueryData(['session'],{data:b});await prefetchProjectSummary(client,http,a,'202');assert.equal(callsRef.n,1);assert.equal(client.getQueryData(projectQuery(http,b,'201').queryKey),undefined);client.clear();
});
void test('anonymous, setup, preview and unverified authority never starts speculative private reads',async()=>{
  let calls=0;const client=new QueryClient(),http=api(()=>{calls++;return Promise.reject(new Error('must not fetch'))});
  for(const user of [account('a',{preview:true}),account('a',{mustChangePassword:true}),account('a',{emailVerified:false}),account('a',{role:'operator',twoFactorEnabled:false})]){client.setQueryData(['session'],{data:user});await prefetchProjectSummary(client,http,user,'201');}
  const expired=account('expired');expired.session.expiresAt='2000-01-01T00:00:00Z';client.setQueryData(['session'],{data:expired});await prefetchProjectSummary(client,http,expired,'201');
  client.setQueryData(['session'],{data:null});await prefetchProjectSummary(client,http,account('a'),'201');assert.equal(calls,0);client.clear();
});
void test('retiring authority aborts an in-flight prefetched summary and cannot fill a new actor cache',async()=>{
  let aborted=false;const client=new QueryClient(),a=account('a'),http=api((_input,options)=>new Promise((_resolve,reject)=>{options?.signal?.addEventListener('abort',()=>{aborted=true;reject(new DOMException('Aborted','AbortError'))},{once:true})}));client.setQueryData(['session'],{data:a});const work=prefetchProjectSummary(client,http,a,'201');await Promise.resolve();await retirePrivateCache(client);await work;assert.equal(aborted,true);assert.equal(client.getQueryData(projectQuery(http,a,'201').queryKey),undefined);client.clear();
});

void test('invalidated or failed session revalidation cannot schedule speculative reads from old data',async()=>{
  const client=new QueryClient(),a=account('a');let calls=0;const http=api(()=>{calls++;return Promise.reject(new Error('must not fetch'))});client.setQueryData(['session'],{data:a});await client.invalidateQueries({queryKey:['session'],refetchType:'none'});await prefetchProjectSummary(client,http,a,'201');assert.equal(calls,0);client.setQueryData(['session'],{data:a});client.getQueryCache().find({queryKey:['session']})?.setState({status:'error',error:new Error('unavailable')});await prefetchProjectSummary(client,http,a,'201');assert.equal(calls,0);client.clear();
});
