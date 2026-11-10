import assert from 'node:assert/strict';
import { cp, mkdtemp, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { coreRuntimeProductPath } from '../release/build-core-console.mjs';
const root=resolve(fileURLToPath(new URL('../..',import.meta.url)));
const source=resolve(process.argv[2] || join(root,'.core-build/console/.next/standalone'));
const fixture=await mkdtemp(join(tmpdir(),'webdock-core-console-artifact-'));
let child;
try {
  await cp(source,fixture,{recursive:true});
  async function files(directory) {
    const result=[];
    for(const entry of await readdir(directory,{withFileTypes:true})) {
      const file=join(directory,entry.name);
      if(entry.isDirectory())result.push(...await files(file));else result.push(file);
    }
    return result;
  }
  const paths=await files(fixture);
  assert.equal(paths.some(coreRuntimeProductPath),false,'Standalone must not contain product packages');
  const server=paths.find(path=>path.endsWith('/server.js')&&!path.includes('/node_modules/'));
  assert.ok(server);
  const port=await new Promise(resolvePort=>{const socket=createServer();socket.listen(0,'127.0.0.1',()=>{const port=socket.address().port;socket.close(()=>resolvePort(port));});});
  let logs='';
  child=spawn(process.execPath,[server],{cwd:fixture,env:{...process.env,PORT:String(port),HOSTNAME:'127.0.0.1',NEXT_TELEMETRY_DISABLED:'1',SPITZLI_AUTH_PROVIDER:JSON.stringify({provider:'better-auth'}),SPITZLI_API_ORIGIN:'http://127.0.0.1:1'},stdio:['ignore','pipe','pipe']});
  child.stdout.on('data',data=>logs+=data);child.stderr.on('data',data=>logs+=data);
  const origin=`http://127.0.0.1:${port}`;
  let response;
  for(let attempt=0;attempt<100;attempt++) {
    try{response=await fetch(origin+'/icon.svg');if(response.ok)break;}catch{}
    await new Promise(done=>setTimeout(done,100));
  }
  assert.equal(response?.status,200,logs);
  const login=await fetch(origin+'/login',{redirect:'manual'});
  // Account pages retain the real API dependency; this isolated fixture supplies no API.
  assert.equal(login.status,500,logs);
  assert.match(logs,/Studio could not reach account services/);
  console.log(JSON.stringify({standalone:relative(root,source),isolated:true,productFiles:0,icon:response.status,accountAPIUnavailable:login.status}));
} finally {
  if(child && child.exitCode===null){child.kill('SIGTERM');await new Promise(done=>child.once('exit',done));}
  await rm(fixture,{recursive:true,force:true});
}
