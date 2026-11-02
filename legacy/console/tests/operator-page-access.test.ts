import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import ts from 'typescript';
import {StudioError,AccessError} from '../src/lib/studio-errors';
function guard(user:unknown={collection:'users',role:'operator',name:'Fixture'},error?:Error){
 const client={};let calls=0;
 const loaded={exports:{} as {requireRegistryOperator:()=>Promise<{payload:unknown;user:unknown}>}};
 const imports:Record<string,unknown>={'server-only':{},react:{cache:(fn:unknown)=>fn},'next/navigation':{redirect:(path:string)=>{throw Error('redirect:'+path);}},'./studio-client':{studioCall:async()=>{calls++;if(error)throw error;return user;}},'./registry-client':{registryClient:client},'./studio-errors':{StudioError,AccessError}};
 const code=ts.transpileModule(readFileSync(new URL('../src/lib/registry-page.ts',import.meta.url),'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
 runInNewContext(code,{exports:loaded.exports,require:(name:string)=>{assert.ok(Object.hasOwn(imports,name),name);return imports[name];}});
 return{run:loaded.exports.requireRegistryOperator,client,calls:()=>calls};
}
test('operator pages use only the verified API mapping and HTTP registry client',async()=>{
 const g=guard(),value=await g.run();assert.equal(value.payload,g.client);assert.equal(g.calls(),1);
 for(const user of [null,{collection:'users',role:'reader'},{collection:'other',role:'operator'}])await assert.rejects(guard(user).run(),/redirect:\/tenants/);
});
test('only a missing local session restarts SSO; upstream authentication failures stay visible',async()=>{
 await assert.rejects(guard(null,new AccessError('Sign in',401)).run(),/redirect:\/api\/sso\/login/);
 await assert.rejects(guard(null,new StudioError('Forbidden',403)).run(),/redirect:\/tenants/);
 for(const status of [401,503]){const error=new StudioError('Upstream unavailable',status);await assert.rejects(guard(null,error).run(),value=>value===error);}
});
