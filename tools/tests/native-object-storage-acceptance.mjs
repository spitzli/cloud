import assert from 'node:assert/strict';
import { createHmac, randomUUID, randomBytes } from 'node:crypto';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { load, reimportArtifacts } from './native-object-storage-fixture.mjs';
import { random, sha256, writePrivate, readPrivate } from './native-plugin-fixture.mjs';
import { ROOT } from './native-plugin-fixture.mjs';
import { join } from 'node:path';
import { metadataSnapshot, runtimeSnapshot, proxyProof } from './native-object-storage-proof.mjs';

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
  if (!c.state.customer) { c.state.customer = await c.json('/api/console/registry/customers', 'POST', { name: 'Owned object storage acceptance' }); await c.save(); }
  if (!c.state.project) { c.state.project = await c.json('/api/console/registry/projects', 'POST', { name: 'Owned storage project', customer: c.state.customer.id }); await c.save(); }
  if (!c.state.foreignCustomer) { c.state.foreignCustomer = await c.json('/api/console/registry/customers', 'POST', { name: 'Owned foreign tenant denial' }); await c.save(); }
  if (!c.state.foreignProject) { c.state.foreignProject = await c.json('/api/console/registry/projects', 'POST', { name: 'Owned foreign storage project', customer: c.state.foreignCustomer.id }); await c.save(); }
  f.report.project = c.state.project.id; f.report.customer = c.state.customer.id; f.report.foreignProject = c.state.foreignProject.id; f.report.foreignCustomer = c.state.foreignCustomer.id;
  f.report.realOperatorSetup = { passwordChange: true, totp: true, pkce: true, registryAPI: true }; await f.saveReport();
}
async function install(f) {
  const c = await client(f), catalog = await c.json('/api/console/plugins/catalog');
  if (!existsSync(f.path('catalog-public.json'))) await f.write('catalog-public.json', catalog);
  const release = catalog.find(x => x.catalogID === 'object-storage' && x.version === f.report.artifacts[0].version); assert(release?.targets?.length === 1);
  const body = { catalogID: 'object-storage', version: release.version, reviewSha256: release.targets[0].reviewSha256, targetID: release.targets[0].id, configuration: {}, approvedCapabilities: release.capabilities, idempotencyKey: 'native-install-' + f.input.runID };
  if (existsSync(f.path('install-request.json'))) assert.deepEqual(await f.json(f.path('install-request.json')), body);
  else await f.write('install-request.json', body);
  const receipt = await c.json('/api/console/plugins/install', 'POST', body); await f.write('install-receipt.json', receipt, existsSync(f.path('install-receipt.json')));
  await observeOperation(f, c, receipt.operationID ?? receipt.id);
  const installed = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'object-storage'); assert(installed?.managed && installed.state === 'enabled' && installed.runtime === 'ready');
  await runtimeSnapshot(f, 'installed'); f.report.signedNativeInstallation = true; await f.saveReport();
}
export async function observeOperation(f, c, id) {
  assert(id, 'operation_id_required');
  return f.wait(async () => { const result = await c.json('/api/console/plugins/operations/' + id); await f.write('operation-' + id + '.json', result, existsSync(f.path('operation-' + id + '.json'))); const path = 'operation-' + id + '-cursor-' + result.cursor + '.json'; if (!existsSync(f.path(path))) await f.write(path, result); assert(!['failed', 'blocked'].includes(result.status), 'native_operation_' + result.status + '_' + result.condition); return result.status === 'succeeded' ? result : false; }, 420000);
}
export async function invoke(c, f, action, input, key = randomUUID(), projectID = f.report.project) {
  return c.json('/api/console/plugins/invoke', 'POST', { pluginID: 'object-storage', projectID, action, input, idempotencyKey: key });
}
async function dataFlow(f) {
  const c = await client(f);
  f.report.proxyProof = await proxyProof(f); await f.saveReport();
  if (!c.state.createRequest) { c.state.createRequest = { request_id: randomUUID(), name: 'Owned native proof', environment: 'acceptance' }; await c.save(); }
  if (!c.state.resource) { const created = await invoke(c, f, 'buckets.create', c.state.createRequest, c.state.createRequest.request_id); c.state.resource = created.resource.id; await c.save(); }
  const resource = c.state.resource;
  let view = await invoke(c, f, 'buckets.view', { resource_id: resource }); assert.equal(view.state, 'active');
  if (!c.state.accessKey) {
    c.state.keyRequest ??= { resource_id: resource, request_id: randomUUID(), revision: view.revision }; await c.save();
    const created = await invoke(c, f, 'credentials.create', c.state.keyRequest, c.state.keyRequest.request_id); c.state.accessKey = created.access_key; await c.save();
  }
  const revealed = await invoke(c, f, 'credentials.reveal', { resource_id: resource, access_key: c.state.accessKey });
  assert(revealed.secret_key && revealed.access_key === c.state.accessKey);
  if (!c.state.credentialSha256) { c.state.credentialSha256 = sha256(JSON.stringify(revealed)); await c.save(); }
  assert.equal(sha256(JSON.stringify(revealed)), c.state.credentialSha256);
  await f.write('credential-private.json', revealed, existsSync(f.path('credential-private.json')));
  if (!existsSync(f.path('sentinel.bin'))) await f.write('sentinel.bin', randomBytes(256 * 1024));
  const bytes = await import('node:fs/promises').then(fs => fs.readFile(f.path('sentinel.bin'))), digest = sha256(bytes), key = 'native-proof/sentinel.bin';
  const input = () => ({ resource_id: resource, key, exact_bytes: bytes.length, expected_sha256: digest, filename: 'sentinel.bin', content_type: 'application/octet-stream', expires_at: Math.floor(Date.now() / 1000) + 60 });
  if (!c.state.uploadReceipt) {
    const descriptor = await invoke(c, f, 'objects.upload', input()); assert(descriptor.path.startsWith('/api/console/plugins/storage/') && descriptor.method === 'PUT');
    assert(!JSON.stringify(descriptor).includes('X-Amz') && !JSON.stringify(descriptor).includes(f.report.proxy.origin));
    const response = await c.raw(f.report.origins.console + descriptor.path, { method: 'PUT', headers: { origin: f.report.origins.console, 'content-type': 'application/octet-stream' }, body: bytes });
    const receipt = await response.json(); await f.write('upload-response.json', { status: response.status, receipt }); assert.equal(response.status, 200);
    assert.equal(receipt.bytes, bytes.length); assert.equal(receipt.sha256, digest); assert(receipt.etag);
    c.state.uploadReceipt = receipt; await c.save();
  }
  if (!existsSync(f.path('minio-observer/backend.json'))) await f.command('/usr/bin/python3', [join(ROOT, 'plugins/object-storage/fixtures/prepare.py'), '--minio-env', join(ROOT, '.dev-local/minio.env'), '--directory', f.path('minio-observer')]);
  view = await invoke(c, f, 'buckets.view', { resource_id: resource });
  const stat = JSON.parse((await f.command(f.path('minio-observer/mc'), ['--json', '--config-dir', f.path('minio-observer/mc-config'), 'stat', 'local/' + view.bucket + '/' + key])).output);
  assert.equal('"' + stat.etag + '"', c.state.uploadReceipt.etag);
  const versions = (await f.command(f.path('minio-observer/mc'), ['--json', '--config-dir', f.path('minio-observer/mc-config'), 'ls', '--versions', 'local/' + view.bucket + '/' + key])).output.split('\n').filter(Boolean).map(line => JSON.parse(line));
  assert.equal(versions.length, 1, 'fixture_exact_owned_version_required');
  const version = versions[0].versionId; assert(typeof version === 'string' && version.length > 0, 'provider_version_observation_required');
  c.state.objectVersion = version; c.state.objectKey = key; await c.save();
  const descriptor = await invoke(c, f, 'objects.download', { ...input(), version_id: version, if_match: c.state.uploadReceipt.etag });
  const response = await c.raw(f.report.origins.console + descriptor.path), downloaded = Buffer.from(await response.arrayBuffer());
  assert.equal(response.status, 200); assert.equal(response.headers.get('cache-control'), 'no-store'); assert.equal(sha256(downloaded), digest); assert(downloaded.equals(bytes));
  const forbidden = await c.raw(f.report.origins.console + '/api/console/plugins/invoke', { method: 'POST', headers: { origin: f.report.origins.console, 'content-type': 'application/json' }, body: JSON.stringify({ pluginID: 'object-storage', projectID: f.report.foreignProject, action: 'objects.download', input: { ...input(), version_id: version, if_match: c.state.uploadReceipt.etag }, idempotencyKey: randomUUID() }) });
  assert.equal(forbidden.status, 403); await forbidden.arrayBuffer();
  const readOnly = await c.json('/api/tenant-preview/start', 'POST', { customerID: f.report.customer, mode: 'read_only' }); assert(readOnly);
  const denied = await c.raw(f.report.origins.console + '/api/console/plugins/invoke', { method: 'POST', headers: { origin: f.report.origins.console, 'content-type': 'application/json' }, body: JSON.stringify({ pluginID: 'object-storage', projectID: f.report.project, action: 'objects.upload', input: input(), idempotencyKey: randomUUID() }) });
  assert.equal(denied.status, 403); await denied.arrayBuffer(); await c.json('/api/tenant-preview/exit', 'POST', {});
  const revokedDescriptor = await invoke(c, f, 'objects.download', { ...input(), version_id: version, if_match: c.state.uploadReceipt.etag });
  const issuedCookies = Object.values(c.state.cookies).join('; ');
  await c.json('/api/sso/logout', 'POST', {});
  const revoked = await fetch(f.report.origins.console + revokedDescriptor.path, { headers: { cookie: issuedCookies }, redirect: 'error', signal: AbortSignal.timeout(10000) }); assert([401, 403].includes(revoked.status)); await revoked.arrayBuffer();
  f.report.dataProof = { resourceID: resource, bytes: bytes.length, sha256: digest, actualCoreProducerUpload: true, actualCoreProducerDownload: true, providerVersionObservedPrivately: true, foreignProjectDenied: true, readOnlyUploadDenied: true, revokedBrowserDenied: true, publicDescriptorHasNoProviderURL: true };
  await f.write('metadata-data.json', await metadataSnapshot(f)); await runtimeSnapshot(f, 'data'); await f.saveReport();
}
async function availability(f, c, desired, label) {
  const installed = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'object-storage'); assert(installed);
  const requests = c.state.availabilityRequests ??= {};
  const body = requests[label] ?? { action: 'state', pluginID: 'object-storage', desired, expectedRevision: installed.revision, idempotencyKey: label + '-' + f.input.runID };
  requests[label] = body; await c.save();
  const receipt = await c.json('/api/console/plugins/installed', 'POST', body);
  await observeOperation(f, c, receipt.id);
  const actual = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'object-storage'); assert.equal(actual.state, desired); return actual;
}
async function retainedRead(f, c, label) {
  const fs = await import('node:fs/promises'), bytes = await fs.readFile(f.path('sentinel.bin'));
  const revealed = await invoke(c, f, 'credentials.reveal', { resource_id: c.state.resource, access_key: c.state.accessKey });
  assert.equal(sha256(JSON.stringify(revealed)), c.state.credentialSha256, 'retained_credential_unseal_changed');
  const descriptor = await invoke(c, f, 'objects.download', { resource_id: c.state.resource, key: c.state.objectKey, exact_bytes: bytes.length, expected_sha256: sha256(bytes), filename: 'sentinel.bin', content_type: 'application/octet-stream', expires_at: Math.floor(Date.now() / 1000) + 60, version_id: c.state.objectVersion, if_match: c.state.uploadReceipt.etag });
  const response = await c.raw(f.report.origins.console + descriptor.path); assert.equal(response.status, 200); assert.equal(sha256(Buffer.from(await response.arrayBuffer())), sha256(bytes));
  const snapshot = await metadataSnapshot(f), original = await f.json(f.path('metadata-data.json')); assert.deepEqual(snapshot, original, 'retained_metadata_or_bucket_key_changed');
  await f.write('metadata-' + label + '.json', snapshot); return runtimeSnapshot(f, label);
}
async function lifecycle(f) {
  await setup(f); const c = await client(f);
  if (!existsSync(f.path('metadata-restart.json'))) {
    const before = await runtimeSnapshot(f, 'before-restart'), active = before.find(r => r.pods.some(p => p.ready)); assert(active && active.pods.length === 1);
    const pod = active.pods[0];
    await f.kube(['delete', '--raw', `/api/v1/namespaces/${active.namespace}/pods/${pod.name}`, '-f', '-'], { input: JSON.stringify({ apiVersion: 'v1', kind: 'DeleteOptions', preconditions: { uid: pod.uid } }) });
    await f.wait(async () => { const pods = (await f.kubeJSON(['-n', active.namespace, 'get', 'pods'])).items; return pods.length === 1 && pods[0].metadata.uid !== pod.uid && pods[0].status.containerStatuses?.every(c => c.ready); }, 120000);
    const restarted = await retainedRead(f, c, 'restart'); assert.deepEqual(restarted[0].objects, active.objects); assert.notEqual(restarted[0].pods[0].uid, pod.uid);
  }
  const current = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'object-storage');
  if (current.desiredState === 'disabled') {
    if (current.state !== 'disabled') await observeOperation(f, c, current.operationID);
  } else await availability(f, c, 'disabled', 'disable-resume-proof');
  await f.write('metadata-disabled.json', await metadataSnapshot(f), existsSync(f.path('metadata-disabled.json')));
  await availability(f, c, 'enabled', 'resume-proof'); await retainedRead(f, c, 'resume');
  await availability(f, c, 'disabled', 'disable-update-proof');
  await reimportArtifacts(f);
  const catalog = await c.json('/api/console/plugins/catalog'), target = catalog.find(r => r.catalogID === 'object-storage' && r.version === f.report.artifacts[1].version); assert(target);
  let installed = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'object-storage');
  const update = await c.json('/api/console/plugins/object-storage/update', 'POST', { version: '0.2.0', reviewSha256: target.targets[0].reviewSha256, expectedRevision: installed.revision, approvedCapabilities: target.capabilities, idempotencyKey: 'native-update-' + f.input.runID });
  await observeOperation(f, c, update.id);
  await availability(f, c, 'enabled', 'enable-update-proof'); const updated = await retainedRead(f, c, 'update');
  const history = await c.json('/api/console/plugins/object-storage/releases'); assert.equal(history.currentGeneration, '2');
  assert(updated.find(r => r.generation === 1).pods.length === 0 && updated.find(r => r.generation === 2).pods[0].ready);
  await availability(f, c, 'disabled', 'disable-rollback-proof');
  await reimportArtifacts(f);
  installed = (await c.json('/api/console/plugins/installed')).find(p => p.id === 'object-storage'); const original = catalog.find(r => r.catalogID === 'object-storage' && r.version === f.report.artifacts[0].version);
  const rollback = await c.json('/api/console/plugins/object-storage/rollback', 'POST', { releaseGeneration: '1', reviewSha256: original.targets[0].reviewSha256, expectedRevision: installed.revision, approvedCapabilities: original.capabilities, idempotencyKey: 'native-rollback-' + f.input.runID });
  await observeOperation(f, c, rollback.id); await availability(f, c, 'enabled', 'enable-rollback-proof'); const rolled = await retainedRead(f, c, 'rollback');
  const final = await c.json('/api/console/plugins/object-storage/releases'); assert.equal(final.currentGeneration, '3');
  assert(rolled.filter(r => r.generation !== 3).every(r => r.pods.length === 0) && rolled.find(r => r.generation === 3).pods[0].ready);
  assert(rolled.every(r => r.storageUrlSha256 === rolled[0].storageUrlSha256 && r.sealingKeySha256 === rolled[0].sealingKeySha256));
  f.report.lifecycleProof = { observedPodRestart: true, exactSixUIDsRetainedOnRestart: true, disableResume: true, update: true, rollback: true, generations: [1, 2, 3], oldGenerationPodsAbsent: true, physicalMetadataAndPasswordRetained: true, sealingKeyRetained: true, bucketAndApplicationKeyRetained: true, originalCredentialUnsealsAcrossGenerations: true, sentinelCoreDownloadsAcrossGenerations: true }; await f.saveReport();
}
async function cleanupResource(f) {
  const c = await client(f), resource = c.state.resource; assert(resource);
  let view = await invoke(c, f, 'buckets.view', { resource_id: resource });
  if (view.state === 'active') view = await invoke(c, f, 'buckets.deactivate', { resource_id: resource, request_id: randomUUID(), revision: view.revision });
  assert.equal(view.state, 'inactive');
  for (let page = 0; page < 10; page++) {
    view = await invoke(c, f, 'buckets.purge', { resource_id: resource, request_id: randomUUID(), revision: view.revision, confirm: resource });
    if (view.purge_complete) break;
  }
  assert.equal(view.purge_complete, true, 'owned_purge_deadline');
  const deleted = await invoke(c, f, 'buckets.delete', { resource_id: resource, request_id: randomUUID(), revision: view.revision, confirm: resource });
  assert.equal(deleted.deleted, true);
  f.report.nativeResourceCleanup = { resourceID: resource, deactivated: true, keysRevoked: true, purged: true, bucketDeleted: true }; await f.saveReport();
}
async function cleanupQualification(f) {
  const c = await client(f);
  c.state.createRequest ??= { request_id: randomUUID(), name: 'Native cleanup regression', environment: 'acceptance' }; await c.save();
  const created = await invoke(c, f, 'buckets.create', c.state.createRequest, c.state.createRequest.request_id); c.state.resource = created.resource.id; await c.save();
  let view = await invoke(c, f, 'buckets.view', { resource_id: c.state.resource });
  const credential = await invoke(c, f, 'credentials.create', { resource_id: c.state.resource, request_id: randomUUID(), revision: view.revision }); c.state.accessKey = credential.access_key; await c.save();
  const before = await runtimeSnapshot(f, 'before-restart'), active = before.find(r => r.pods.some(p => p.ready)); assert(active?.pods.length === 1);
  const original = await metadataSnapshot(f), pod = active.pods[0];
  await f.kube(['delete', '--raw', `/api/v1/namespaces/${active.namespace}/pods/${pod.name}`, '-f', '-'], { input: JSON.stringify({ apiVersion: 'v1', kind: 'DeleteOptions', preconditions: { uid: pod.uid } }) });
  await f.wait(async () => { const p = (await f.kubeJSON(['-n', active.namespace, 'get', 'pods'])).items; return p.length === 1 && p[0].metadata.uid !== pod.uid && p[0].status.containerStatuses?.every(c => c.ready); }, 120000);
  const path = '/tmp/webdock-minio/' + c.state.resource.replaceAll('-', '') + '.policy.json';
  await f.kube(['-n', active.namespace, 'exec', 'deployment/runtime', '--', '/bin/sh', '-ec', 'test ! -e "$1" && test ! -L "$1"', 'sh', path]);
  assert.deepEqual(await metadataSnapshot(f), original);
  const restarted = await runtimeSnapshot(f, 'restart'); assert.deepEqual(restarted[0].objects, active.objects);
  await cleanupResource(f);
  const deleted = await metadataSnapshot(f); assert.equal(deleted.resources[0].state, 'deleted'); assert.equal(deleted.resources[0].credentials.length, 0);
  f.report.cleanupRegressionProof = { actualNativeImage: true, exactSixUIDsRetained: true, actualPodReplacement: true, localPolicyCacheAbsent: true, metadataRetainedBeforeDelete: true, normalCoreDeleteSucceeded: true, privateResourceStateDeleted: true, noSQLRepair: true }; await f.saveReport();
}
export async function main([stage, runID]) {
  const f = await load(runID);
  try {
    assert(Date.now() < f.report.deadline, 'fixture_deadline_expired');
    if (stage === 'setup') await setup(f);
    else if (stage === 'install') await install(f);
    else if (stage === 'data') await dataFlow(f);
    else if (stage === 'lifecycle') await lifecycle(f);
    else if (stage === 'cleanup-resource') await cleanupResource(f);
    else if (stage === 'cleanup-qualification') await cleanupQualification(f);
    else throw Error('unsupported_acceptance_stage');
    (f.report.completedStages ??= []).push(stage); await f.saveReport(); process.stdout.write(`native_object_storage_${stage}_passed ${runID}\n`);
  } catch (error) { await f.write('acceptance-failure-' + Date.now() + '.json', { stage, error: String(error), stack: error.stack }); throw Error('native_object_storage_acceptance_failed_private_diagnostics_retained'); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main(process.argv.slice(2)).catch(error => { process.stderr.write(error.message + '\n'); process.exitCode = 1; });
