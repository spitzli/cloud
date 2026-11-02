import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {generateKeyPairSync,randomUUID,sign} from 'node:crypto';
import {request} from 'node:https';
import {once} from 'node:events';
import {createActionServer} from '../templates/actions/src/runtime.ts';

test('actions runtime requires TLS, bearer identity and action-bound signed invocations', async()=>{
 const dir=await mkdtemp(join(tmpdir(),'webdock-runtime-'));
 let server;
 try {
  // ECDSA P-256 like the certificates the platform issues to plugin runtimes.
  const generated=spawnSync('openssl',['req','-x509','-newkey','ec','-pkeyopt','ec_paramgen_curve:P-256','-noenc','-days','1','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost','-keyout',join(dir,'key.pem'),'-out',join(dir,'cert.pem')],{encoding:'utf8'});
  assert.equal(generated.status,0,generated.stderr);
  const cert=await readFile(join(dir,'cert.pem'));
  const key=await readFile(join(dir,'key.pem'));
  const keys=generateKeyPairSync('ed25519');
  const publicKey=keys.publicKey.export({format:'pem',type:'spki'}).toString();
  const runtimeToken='runtime-secret-'.repeat(4);
  server=createActionServer({pluginID:'echo-demo',version:'0.1.0',generation:1,hostURL:'https://api.example.test',runtimeToken,publicKey,cert,key});
  server.listen(0,'127.0.0.1');await once(server,'listening');
  const token=(changes={})=>{
   const now=Math.floor(Date.now()/1000);
   const body=Buffer.from(JSON.stringify({v:1,id:randomUUID(),pluginID:'echo-demo',generation:1,projectID:'42',customerID:'101',action:'demo.echo',iat:now,exp:now+60,...changes})).toString('base64url');
   return body+'.'+sign(null,Buffer.from(body),keys.privateKey).toString('base64url');
  };
  const call=(path,{auth=true,invocation,body,method='GET'}={})=>new Promise((resolve,reject)=>{
   const req=request({hostname:'127.0.0.1',servername:'localhost',port:server.address().port,path,method,ca:cert,headers:{...(auth?{authorization:`Bearer ${runtimeToken}`}:{}) ,...(invocation?{'x-plugin-invocation':invocation}:{})}},res=>{
    const chunks=[];res.on('data',chunk=>chunks.push(chunk));res.on('end',()=>resolve({status:res.statusCode,body:JSON.parse(Buffer.concat(chunks).toString())}));
   });req.on('error',reject);req.end(body===undefined?undefined:JSON.stringify(body));
  });
  assert.equal((await call('/healthz',{auth:false})).status,401);
  assert.deepEqual(await call('/healthz'),{status:200,body:{pluginID:'echo-demo',version:'0.1.0',generation:1,protocolVersion:1}});
  for(const invocation of [undefined,token({action:'another.action'}),token({pluginID:'other-plugin'}),token({generation:2}),token({exp:0}),token()+'x']) {
   assert.equal((await call('/actions/demo.echo',{method:'POST',invocation,body:{input:{message:'hello'}}})).status,403);
  }
  assert.deepEqual(await call('/actions/demo.echo',{method:'POST',invocation:token(),body:{input:{message:'hello'}}}),{status:200,body:{message:'hello'}});
  assert.equal((await call('/actions/demo.echo',{method:'POST',invocation:token(),body:{input:{message:42}}})).status,400);
  assert.equal((await call('/actions/demo.echo',{method:'POST',invocation:token(),body:{input:{message:'x'.repeat(33000)}}})).status,413);
  assert.equal((await call('/actions/unknown',{method:'POST',invocation:token()})).status,404);
 } finally {
  if(server) await new Promise(resolve=>server.close(resolve));
  await rm(dir,{recursive:true,force:true});
 }
});
