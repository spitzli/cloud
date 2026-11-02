import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,writeFileSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {ownedCleanup} from './core-browser-cleanup.mjs';
test('Next env loader cannot load a sentinel even with forceReload',()=>{
 const dir=mkdtempSync(join(tmpdir(),'webdock-no-env-'));
 try{writeFileSync(join(dir,'.env.local'),'WEBDOCK_FIXTURE_SENTINEL=unexpected\n');
 const r=spawnSync(process.execPath,['--preload',resolve(import.meta.dirname,'core-browser-no-env.cjs'),'-e',`const assert=require('node:assert/strict'),e=require('@next/env');const r=e.loadEnvConfig(process.argv[1],true,console,true);assert.equal(process.env.WEBDOCK_FIXTURE_SENTINEL,undefined);assert.deepEqual(r.loadedEnvFiles,[]);e.processEnv([{path:'.env.local',contents:'WEBDOCK_FIXTURE_SENTINEL=unexpected'}],process.argv[1],console,true);assert.equal(process.env.WEBDOCK_FIXTURE_SENTINEL,undefined);`,dir],{env:{PATH:process.env.PATH},encoding:'utf8'});
 assert.equal(r.status,0,r.stderr);
 }finally{rmSync(dir,{recursive:true,force:true});}
});
test('cleanup continues after failure and removes only matching owned container once',async()=>{
 const calls=[];const cleanup=ownedCleanup({name:'owned',closeServers:async()=>{calls.push('servers');throw Error('close');},closePool:async()=>calls.push('pool'),stopChildren:async()=>calls.push('children'),inspectLabel:async()=>'owned',removeContainer:async()=>calls.push('remove')});
 await assert.rejects(cleanup(),AggregateError);await assert.rejects(cleanup(),AggregateError);assert.deepEqual(calls,['servers','pool','children','remove']);
});
test('cleanup refuses mismatched container label',async()=>{
 let removed=false;const noop=async()=>{};await assert.rejects(ownedCleanup({name:'owned',closeServers:noop,closePool:noop,stopChildren:noop,inspectLabel:async()=>'other',removeContainer:async()=>{removed=true;}})(),AggregateError);assert.equal(removed,false);
});
