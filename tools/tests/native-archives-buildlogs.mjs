// Archives-owned qualification, run only after the coordinated schema/compiler release.
import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {gunzipSync} from 'node:zlib';
import {load} from './native-archives-fixture.mjs';
import {client,invoke} from './native-archives-acceptance.mjs';
import {ROOT,sha256} from './native-plugin-fixture.mjs';

export async function sourceOperation(f,stage,buildId){
 const c=await client(f);const env=await f.json(f.path('core-environment.json'));const platform=await f.json(f.path('platform.json'));
 assert(f.report.realOperatorSetup?.registryAPI&&f.report.schema,'actual bootstrapped Native project/operator required');
 const executable=f.path(f.report.buildlogHelper?.path??'binaries/native-buildlog-source');
 if(!f.report.buildlogHelper){f.report.buildlogHelper=await f.json(f.path('buildlog-helper-pin.json'));assert.equal(f.report.buildlogHelper.schemaThrough,53);await f.saveReport();}
 assert.equal(sha256(await readFile(executable)),f.report.buildlogHelper.sha256);
 const admin=new URL(platform.url);admin.searchParams.set('sslmode','verify-full');admin.searchParams.set('sslrootcert',join(platform.directory,'ca.crt'));
 const input={runId:f.input.runID,attempt:f.report.buildlogHelper?.attempt??f.report.buildlogHelper?.version??1,database:env.WEBDOCK_SESSION_DATABASE_URL,authDatabase:env.WEBDOCK_DATABASE_URL,workerDatabase:env.WEBDOCK_WORKER_DATABASE_URL,adminDatabase:admin.href,rlsSecret:env.DATABASE_RLS_SECRET,authSecret:env.WEBDOCK_AUTH_SECRET,cookie:Object.values(c.state.cookies).join('; '),customer:f.report.customer,project:f.report.project,cluster:f.report.clusterID,vmRoot:join(ROOT,'.runtime-test/build-vm'),scratch:f.directory,buildId:buildId??null,connectionId:f.report.buildlog?.connectionID??null};
 const name='buildlog-source-'+stage+'-'+Date.now()+'.json';await f.write(name,input);
 const result=await f.command(executable,[stage,f.path(name)],{timeout:480000});
 const evidence=JSON.parse(result.output);
 if(stage==='setup'){assert(evidence.sources.succeeded.actualCoreCompletion&&evidence.sources.failed.actualCoreCompletion);assert(evidence.sources.succeeded.actualVMOutput&&evidence.sources.failed.actualVMOutput);f.report.buildlog=evidence;}
 else (f.report.buildlogTransitions??=[]).push({stage,...evidence});
 await f.saveReport();return evidence;
}
async function completed(f,c,resource){return f.wait(async()=>{const status=await invoke(c,f,'archives.status',{resource_id:resource});assert(!['failed','cancelled'].includes(status.job?.state));return status.state==='ready'&&status.job?.state==='completed'?status:false;},90000);}
async function get(c,f,descriptor,expected=200){const response=await c.raw(f.report.origins.console+descriptor.path);assert.equal(response.status,expected);return Buffer.from(await response.arrayBuffer());}
export async function flow(f){
 const c=await client(f);assert(f.report.buildlog?.sources.failed.actualVMOutput);
 const choices=await invoke(c,f,'archives.sources',{});c.state.buildlog??={};
 for(const outcome of ['succeeded','failed']){
  const source=f.report.buildlog.sources[outcome];assert(choices.sources.some(s=>s.kind==='buildlog'&&s.id===source.buildID));
  const id=randomUUID();await invoke(c,f,'archives.create',{request_id:id,kind:'buildlog',source_id:source.buildID,retention_seconds:3600},id);
  await invoke(c,f,'archives.generate',{resource_id:id},'generate-'+id);const status=await completed(f,c,id);assert.equal(status.kind,'buildlog');assert.equal(status.source.id,source.buildID);assert(status.bytes>0&&status.bytes<=65536);
  const descriptor=await invoke(c,f,'archives.download',{resource_id:id});assert.equal(descriptor.attachment.filename,'build.log.gz');assert.equal(descriptor.attachment.contentType,'application/gzip');
  const gzip=await get(c,f,descriptor);assert.equal(sha256(gzip),status.sha256);const bytes=gunzipSync(gzip);assert.equal(bytes.length,source.bytes);assert.equal(sha256(bytes),source.sha256);assert(bytes.length<=24000);
  c.state.buildlog[outcome]={resource:id,descriptor,sha256:status.sha256,bytes:status.bytes,sourceID:source.buildID};await c.save();
 }
 // The exact same image/generation restarts; durable private receipts remain readable.
 const namespaces=(await f.kubeJSON(['get','namespaces','-l','webdock.dev/plugin=archives'])).items;assert.equal(namespaces.length,1);const ns=namespaces[0].metadata.name;const pods=(await f.kubeJSON(['-n',ns,'get','pods'])).items;const prior=pods.find(p=>p.status.containerStatuses?.some(c=>c.name==='runtime'&&c.ready));assert(prior);
 await f.kube(['-n',ns,'delete','pod',prior.metadata.name,'--wait=false']);
 await f.wait(async()=>{const p=(await f.kubeJSON(['-n',ns,'get','pods'])).items;return p.some(p=>p.metadata.uid!==prior.metadata.uid&&p.status.containerStatuses?.some(c=>c.name==='runtime'&&c.ready));},90000);
 for(const output of Object.values(c.state.buildlog)){const bytes=await f.wait(async()=>{const response=await c.raw(f.report.origins.console+output.descriptor.path);if(response.status!==200){await response.arrayBuffer();return false;}return Buffer.from(await response.arrayBuffer());},90000);assert.equal(sha256(bytes),output.sha256);}
 f.report.buildlogNativeProof={actualCoreCompletionToNativeJobS3Gateway:true,succeededAndFailed:true,sourceHashesMatchActualStoredBytes:true,knownLiteralMaskedByActualPipeline:true,noPublicLogPayload:true,producerRestartRetainsReceipts:true,originalDescriptorWorksAfterSameGenerationRestart:true};await f.saveReport();
}
export async function expiry(f){
 const c=await client(f),id=randomUUID();await invoke(c,f,'archives.create',{request_id:id,kind:'buildlog',source_id:f.report.buildlog.sources.succeeded.buildID,retention_seconds:15},id);await invoke(c,f,'archives.generate',{resource_id:id},'generate-'+id);const status=await completed(f,c,id);const d=await invoke(c,f,'archives.download',{resource_id:id});await get(c,f,d);
 await new Promise(r=>setTimeout(r,Math.max(0,status.expires_at*1000-Date.now())+100));const response=await c.raw(f.report.origins.console+d.path);assert([401,403,404,409,410].includes(response.status));await response.arrayBuffer();
 const denied=await c.raw(f.report.origins.console+'/api/console/plugins/invoke',{method:'POST',headers:{origin:f.report.origins.console,'content-type':'application/json'},body:JSON.stringify({pluginID:'archives',projectID:f.report.project,action:'archives.download',input:{resource_id:id},idempotencyKey:randomUUID()})});assert([403,404,409].includes(denied.status));await denied.arrayBuffer();await invoke(c,f,'archives.delete',{resource_id:id});
 f.report.buildlogExpiryProof={actualJob:true,seconds:15,oldAndNewDownloadsDenied:true,expiredExactDeletion:true};await f.saveReport();
}
export async function corrupt(f){
 const c=await client(f),output=c.state.buildlog.failed;output.descriptor=await invoke(c,f,'archives.download',{resource_id:output.resource});await c.save();await get(c,f,output.descriptor);
 await sourceOperation(f,'corrupt-log',f.report.buildlog.sources.failed.buildID);
 const response=await c.raw(f.report.origins.console+output.descriptor.path);assert([401,403,404,409].includes(response.status));await response.arrayBuffer();
 await invoke(c,f,'archives.delete',{resource_id:output.resource});f.report.buildlogCorruptionProof={exactStoredLogHashChanged:true,oldOpaqueDescriptorDenied:true,authorizedExactDeletionAfterSourceChange:true,noRepairPerformed:true};await f.saveReport();
}
export async function main([stage,runID]){
 const f=await load(runID);assert(Date.now()<f.report.deadline);
 try{if(stage==='setup')await sourceOperation(f,'setup');else if(stage==='flow')await flow(f);else if(stage==='expiry')await expiry(f);else if(stage==='corrupt')await corrupt(f);else throw Error('unsupported_buildlog_stage');console.log('native_archives_buildlog_'+stage+'_passed '+runID);}
 catch(error){await f.write('buildlog-failure-'+Date.now()+'.json',{stage,error:String(error),stack:error.stack});throw Error('native_archives_buildlog_failed_private_diagnostics_retained');}
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url))main(process.argv.slice(2)).catch(error=>{process.stderr.write(error.message+'\n');process.exitCode=1;});
