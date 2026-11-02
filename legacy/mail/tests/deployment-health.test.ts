import test from 'node:test';
import assert from 'node:assert/strict';
import {once} from 'node:events';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {createMailAPI} from '../src/api/server.ts';
import {createMailGateway} from '../src/gateway/server.ts';
import type {PrivatePayloadStore} from '../src/sending/payload-store.ts';

test('rendered startup, readiness and liveness probes reach the real servers without credentials',async()=>{
 const image='webdock.local/mail@sha256:'+'a'.repeat(64);
 const resources=JSON.parse(execFileSync('python3',[fileURLToPath(new URL('../deploy/render.py',import.meta.url)),'--api-image',image,'--gateway-image',image],{encoding:'utf8'}));
 const pod=resources.items.find((item:{kind:string})=>item.kind==='Deployment').spec.template.spec;
 const control=async()=>{throw Error('Health must not invoke authenticated control');};
 for(const container of pod.containers){
  const server=container.name==='api'?createMailAPI(control,{} as PrivatePayloadStore):createMailGateway({control,allowedOrigins:['https://studio.example.test']});
  server.listen(0,'127.0.0.1');await once(server,'listening');
  const origin=`http://127.0.0.1:${(server.address() as {port:number}).port}`;
  try{for(const probe of ['startupProbe','readinessProbe','livenessProbe']){
   const response=await fetch(origin+container[probe].httpGet.path);
   assert.equal(response.status,200,`${container.name} ${probe}`);
   assert.deepEqual(await response.json(),{ok:true});
  }}finally{server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));}
 }
});
