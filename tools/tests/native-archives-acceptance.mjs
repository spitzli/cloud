import assert from 'node:assert/strict';
import { createHmac, randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { load } from './native-archives-fixture.mjs';
import { random, sha256 } from './native-plugin-fixture.mjs';
import { runtimeSnapshot } from './native-archives-proof.mjs';

export function totp(uri) {
  let bits = ''; for (const c of new URL(uri).searchParams.get('secret').toUpperCase().replace(/=+$/, '')) bits += 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'.indexOf(c).toString(2).padStart(5, '0');
  const key = Buffer.from(bits.match(/.{8}/g).map(b => parseInt(b, 2))), counter = Buffer.alloc(8); counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const h = createHmac('sha1', key).update(counter).digest(), at = h[h.length - 1] & 15;
  return String((h.readUInt32BE(at) & 0x7fffffff) % 1000000).padStart(6, '0');
}
export async function client(f) {
  const path = f.path('http-private.json'), state = existsSync(path) ? await f.json(path) : { cookies: {} };
  const save = () => f.write('http-private.json', state, existsSync(path));
  async function raw(url, options = {}) {
    assert(Object.values(f.report.origins).includes(new URL(url).origin), 'fixture_external_http_denied');
    const headers = new Headers(options.headers); headers.set('cookie', Object.values(state.cookies).join('; '));
    const response = await fetch(url, { ...options, headers, redirect: 'manual', signal: AbortSignal.timeout(30000) });
    for (const cookie of response.headers.getSetCookie()) { const value = cookie.split(';')[0], name = value.split('=')[0]; state.cookies[name] = value; (state.cookieOrigins ??= {})[name] = new URL(url).origin; }
    await save(); return response;
  }
  async function json(path, method = 'GET', body, origin = f.report.origins.console) {
    const response = await raw(origin + path, { method, headers: { origin, accept: 'application/json', 'content-type': 'application/json' }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
    const bytes = await response.text(); let value; try { value = JSON.parse(bytes); } catch { value = { nonJSON: true }; }
    await f.write('last-http.json', { path, method, status: response.status, value }, existsSync(f.path('last-http.json')));
    assert.equal(response.status, 200, `fixture_http_status_${response.status}`); return value;
  }
  return { state, save, raw, json };
}
export async function setup(f) {
  const c = await client(f), operator = await f.json(f.path('operator-input.json')), auth = (path, body) => c.json('/api/auth' + path, 'POST', body, f.report.origins.account);
  await auth('/sign-in/email', { email: operator.email, password: c.state.password ?? operator.password });
  if (!c.state.password) { const next = 'Owned-' + random() + '!'; await auth('/change-password', { currentPassword: operator.password, newPassword: next, revokeOtherSessions: true }); c.state.password = next; await c.save(); }
  if (!c.state.totpURI) { const result = await auth('/two-factor/enable', { password: c.state.password }); c.state.totpURI = result.totpURI; await c.save(); }
  await auth('/two-factor/verify-totp', { code: totp(c.state.totpURI) });
  let url = f.report.origins.console + '/api/sso/login?returnTo=/projects';
  for (let n = 0; n < 12; n++) {
    const response = await c.raw(url);
    if (response.status >= 300 && response.status < 400) { url = new URL(response.headers.get('location'), url).href; continue; }
    if (response.headers.get('content-type')?.includes('application/json')) { const body = await response.clone().json(); if (body.redirect && typeof body.url === 'string') { url = new URL(body.url, url).href; continue; } }
    assert.equal(response.status, 200, 'fixture_real_sso_failed'); assert.equal(new URL(url).origin, f.report.origins.console); break;
  }
  const session = await c.json('/api/console/session'); assert.equal(session.data?.user.role, 'operator');
  if (!c.state.customer) { c.state.customer = await c.json('/api/console/registry/customers', 'POST', { name: 'Owned archives acceptance' }); await c.save(); }
  if (!c.state.project) { c.state.project = await c.json('/api/console/registry/projects', 'POST', { name: 'Owned archives project', customer: c.state.customer.id }); await c.save(); }
  if (!c.state.foreignCustomer) { c.state.foreignCustomer = await c.json('/api/console/registry/customers', 'POST', { name: 'Owned foreign tenant denial' }); await c.save(); }
  if (!c.state.foreignProject) { c.state.foreignProject = await c.json('/api/console/registry/projects', 'POST', { name: 'Owned foreign archives project', customer: c.state.foreignCustomer.id }); await c.save(); }
  f.report.project = c.state.project.id; f.report.customer = c.state.customer.id; f.report.foreignProject = c.state.foreignProject.id; f.report.foreignCustomer = c.state.foreignCustomer.id;
  f.report.realOperatorSetup = { passwordChange: true, totp: true, pkce: true, registryAPI: true }; await f.saveReport();
}
async function install(f) {
  const c = await client(f), catalog = await c.json('/api/console/plugins/catalog');
  if (!existsSync(f.path('catalog-public.json'))) await f.write('catalog-public.json', catalog);
  const release = catalog.find(x => x.catalogID === 'archives' && x.version === f.report.artifacts[0].version); assert(release?.targets?.length === 1);
  const body = { catalogID: 'archives', version: release.version, reviewSha256: release.targets[0].reviewSha256, targetID: release.targets[0].id, configuration: {}, approvedCapabilities: release.capabilities, idempotencyKey: 'native-install-' + f.input.runID };
  if (existsSync(f.path('install-request.json'))) assert.deepEqual(await f.json(f.path('install-request.json')), body);
  else await f.write('install-request.json', body);
  const receipt = await c.json('/api/console/plugins/install', 'POST', body); await f.write('install-receipt.json', receipt, existsSync(f.path('install-receipt.json')));
  await observeOperation(f, c, receipt.operationID ?? receipt.id);
  const installed = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'archives'); assert(installed?.managed && installed.state === 'enabled' && installed.runtime === 'ready');
  await runtimeSnapshot(f, 'installed'); f.report.signedNativeInstallation = true; await f.saveReport();
}
export async function observeOperation(f, c, id) {
  assert(id, 'operation_id_required');
  return f.wait(async () => { const result = await c.json('/api/console/plugins/operations/' + id); await f.write('operation-' + id + '.json', result, existsSync(f.path('operation-' + id + '.json'))); const path = 'operation-' + id + '-cursor-' + result.cursor + '.json'; if (!existsSync(f.path(path))) await f.write(path, result); assert(!['failed', 'blocked'].includes(result.status), 'native_operation_' + result.status + '_' + result.condition); return result.status === 'succeeded' ? result : false; }, 420000);
}
export async function invoke(c, f, action, input, key = randomUUID(), projectID = f.report.project) {
  return c.json('/api/console/plugins/invoke', 'POST', { pluginID: 'archives', projectID, action, input, idempotencyKey: key });
}
async function completed(f,c,resource){
 return f.wait(async()=>{const status=await invoke(c,f,'archives.status',{resource_id:resource});
  assert(!['failed','cancelled'].includes(status.job?.state),'archive_job_terminal_failure');
  return status.state==='ready'&&status.job?.state==='completed'?status:false;
 },90000);
}
async function legacyConfigurationDenial(f){
 const namespaces=(await f.kubeJSON(['get','namespaces','-l','webdock.dev/plugin=archives'])).items;assert.equal(namespaces.length,1);
 const namespace=namespaces[0].metadata.name,pods=(await f.kubeJSON(['-n',namespace,'get','pods'])).items;
 const pod=pods.find(p=>p.status.containerStatuses?.some(c=>c.name==='runtime'&&c.ready));assert(pod);
 // Private material stays inside this process and the owned pod's anonymous /tmp;
 // the probe exits before transport/backend startup and removes its exact files.
 const config=JSON.parse((await f.kube(['-n',namespace,'exec',pod.metadata.name,'-c','runtime','--','cat','/run/webdock/runtime/runtime.json'])).output);
 config.databaseSources=[{customer:f.report.customer,project:f.report.project,url:'postgresql://fixture:forbidden@127.0.0.1/source'}];
 const script='umask 077; test ! -e /tmp/archives-legacy-probe.json; test ! -e /tmp/archives-legacy-probe.err; trap "rm -f /tmp/archives-legacy-probe.json /tmp/archives-legacy-probe.err" EXIT; cat > /tmp/archives-legacy-probe.json; status=0; /usr/local/bin/webdock-archives /tmp/archives-legacy-probe.json 2>/tmp/archives-legacy-probe.err || status=$?; test "$status" = 1; grep -Fx archives_plugin_start_failed_legacy_database_sources /tmp/archives-legacy-probe.err';
 const probe=await f.kube(['-n',namespace,'exec','-i',pod.metadata.name,'-c','runtime','--','/bin/sh','-ec',script],{input:JSON.stringify(config)});
 assert.equal(probe.output,'archives_plugin_start_failed_legacy_database_sources');
 f.report.legacyConfigurationDenied={actualSignedExecutable:true,fixtureFeatureCompiled:true,beforeTransportOrBackend:true,privateProbeFilesRemoved:true};await f.saveReport();
}
async function flow(f){
 await legacyConfigurationDenial(f);
 const c=await client(f),sources=await invoke(c,f,'archives.sources',{});
 const unbound=await c.raw(f.report.origins.console+'/api/console/plugins/invoke',{method:'POST',headers:{origin:f.report.origins.console,'content-type':'application/json'},body:JSON.stringify({pluginID:'archives',projectID:f.report.project,action:'archives.create',input:{request_id:randomUUID(),kind:'postgres',retention_seconds:3600},idempotencyKey:randomUUID()})});assert([403,409].includes(unbound.status),'unbound_postgres_must_be_denied');await unbound.arrayBuffer();
 f.report.unqualifiedPostgresDenied={actualSignedRuntime:true,evenForOperator:true,noSourceID:true};await f.saveReport();
 assert(sources.sources.some(s=>s.kind==='report'&&s.id===f.report.project),'real_project_report_source_missing');
 c.state.archiveCreate??={request_id:randomUUID(),kind:'report',source_id:f.report.project,retention_seconds:3600};await c.save();
 if(!c.state.resource){const created=await invoke(c,f,'archives.create',c.state.archiveCreate,c.state.archiveCreate.request_id);c.state.resource=created.resource.id;await c.save();}
 let status=await invoke(c,f,'archives.status',{resource_id:c.state.resource});assert.equal(status.kind,'report');
 if(!status.jobID){const queued=await invoke(c,f,'archives.generate',{resource_id:c.state.resource},'generate-'+f.input.runID);c.state.jobID=queued.id;await c.save();}
 status=await completed(f,c,c.state.resource);assert(status.bytes>0&&status.bytes<=33554432&&/^[a-f0-9]{64}$/.test(status.sha256));
 const descriptor=await invoke(c,f,'archives.download',{resource_id:c.state.resource});
 assert.equal(descriptor.expectedSha256,status.sha256);assert.equal(descriptor.exactBytes,status.bytes);assert.equal(descriptor.method,'GET');
 assert(!JSON.stringify(descriptor).includes('X-Amz')&&!JSON.stringify(descriptor).includes(f.report.proxy.origin)&&!JSON.stringify(descriptor).includes(f.report.minio.bucket));
 const response=await c.raw(f.report.origins.console+descriptor.path);assert.equal(response.status,200);const bytes=Buffer.from(await response.arrayBuffer());assert.equal(bytes.length,status.bytes);assert.equal(sha256(bytes),status.sha256);
 const {gunzipSync}=await import('node:zlib');const csv=gunzipSync(bytes).toString();assert(csv.startsWith('section,kind,count\n'));assert(csv.includes('project,'+f.report.project+',1'));assert(csv.includes('resources,archives.archive,1'));
 const foreign=await c.raw(f.report.origins.console+'/api/console/plugins/invoke',{method:'POST',headers:{origin:f.report.origins.console,'content-type':'application/json'},body:JSON.stringify({pluginID:'archives',projectID:f.report.foreignProject,action:'archives.download',input:{resource_id:c.state.resource},idempotencyKey:randomUUID()})});assert([403,404].includes(foreign.status));await foreign.arrayBuffer();
 c.state.beforeDeleteDescriptor=descriptor;await c.save();
 f.report.dataProof={actualNormalSignedImage:true,realCoreDurableJob:true,realCoreObservedReport:true,producerReadonlyMetadata:true,coreBinaryGateway:true,bytes:bytes.length,sha256:status.sha256,foreignTenantDenied:true,publicDescriptorHasNoProviderURL:true,artifactID:c.state.resource,jobID:status.job.id};await runtimeSnapshot(f,'report');await f.saveReport();
}
async function deletion(f){
 const c=await client(f);assert(c.state.resource);
 const fresh=await invoke(c,f,'archives.download',{resource_id:c.state.resource});const before=await c.raw(f.report.origins.console+fresh.path);assert.equal(before.status,200);await before.arrayBuffer();
 c.state.beforeDeleteDescriptor=fresh;await c.save();
 await invoke(c,f,'archives.delete',{resource_id:c.state.resource},'delete-'+f.input.runID);
 const response=await c.raw(f.report.origins.console+c.state.beforeDeleteDescriptor.path);assert([401,403,404,409].includes(response.status));await response.arrayBuffer();
 f.report.deletionProof={explicitCompletedDeletion:true,oldOpaqueDescriptorDenied:true};await f.saveReport();
}
async function expiry(f){
 const c=await client(f),id=randomUUID();const created=await invoke(c,f,'archives.create',{request_id:id,kind:'report',source_id:f.report.project,retention_seconds:15},id);const resource=created.resource.id;
 await invoke(c,f,'archives.generate',{resource_id:resource},'expire-generate-'+f.input.runID);const status=await completed(f,c,resource);const descriptor=await invoke(c,f,'archives.download',{resource_id:resource});
 assert(descriptor.expiresAt<=status.expires_at);const before=await c.raw(f.report.origins.console+descriptor.path);assert.equal(before.status,200);await before.arrayBuffer();
 await new Promise(r=>setTimeout(r,Math.max(0,status.expires_at*1000-Date.now())+100));
 const expired=await c.raw(f.report.origins.console+descriptor.path);assert([401,403,404,409,410].includes(expired.status));await expired.arrayBuffer();
 const result=await c.raw(f.report.origins.console+'/api/console/plugins/invoke',{method:'POST',headers:{origin:f.report.origins.console,'content-type':'application/json'},body:JSON.stringify({pluginID:'archives',projectID:f.report.project,action:'archives.download',input:{resource_id:resource},idempotencyKey:randomUUID()})});assert([403,404,409].includes(result.status));await result.arrayBuffer();
 await invoke(c,f,'archives.delete',{resource_id:resource},'expire-delete-'+f.input.runID);f.report.expiryProof={actualNativeJob:true,boundedAccessSeconds:15,descriptorClippedToExpiry:true,oldDescriptorDenied:true,newDownloadDenied:true,expiredResourceExplicitlyDeleted:true};await f.saveReport();
}
export async function main([stage,runID]){
 const f=await load(runID);assert(Date.now()<f.report.deadline,'owned_fixture_deadline');
 try{if(stage==='setup')await setup(f);else if(stage==='install')await install(f);else if(stage==='flow')await flow(f);else if(stage==='delete')await deletion(f);else if(stage==='expiry')await expiry(f);else throw Error('unsupported_archives_acceptance_stage');
 (f.report.acceptanceStages??=[]).push(stage);await f.saveReport();process.stdout.write('native_archives_'+stage+'_passed '+runID+'\n');
 }catch(error){await f.write('acceptance-failure-'+Date.now()+'.json',{stage,error:String(error),stack:error.stack});throw Error('native_archives_acceptance_failed_private_diagnostics_retained');}
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url))main(process.argv.slice(2)).catch(error=>{process.stderr.write(error.message+'\n');process.exitCode=1;});
