// Real Console/Core/native producer acceptance. Only the first create reply is deliberately lost.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { randomUUID } from 'node:crypto';
import { chmod, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { load } from './native-object-storage-fixture.mjs';
import { setup, client } from './native-object-storage-acceptance.mjs';
import { sha256, ROOT } from './native-plugin-fixture.mjs';
import { database } from './native-object-storage-proof.mjs';

const f = await load(process.argv[2]); let browser;
try {
  assert(Date.now() < f.report.deadline); await setup(f); const c = await client(f);
  browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, acceptDownloads: true });
  await context.addCookies(Object.entries(c.state.cookies).filter(([, v]) => v.slice(v.indexOf('=') + 1)).map(([name, value]) => ({ name, value: value.slice(value.indexOf('=') + 1), url: c.state.cookieOrigins[name], httpOnly: true, sameSite: 'Lax' })));
  const page = await context.newPage(); page.setDefaultTimeout(20000);
  const errors = [], creates = [], descriptors = [], binaryRequests = []; let lost = false, resource;
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => {
    assert(!request.url().includes('X-Amz-Signature') && !request.url().startsWith(f.report.proxy.origin), 'browser_provider_request_forbidden');
    if (new URL(request.url()).pathname.startsWith('/api/console/plugins/storage/')) binaryRequests.push({ method: request.method(), size: request.postDataBuffer()?.length ?? 0, headers: Object.keys(request.headers()) });
  });
  await page.route('**/api/console/plugins/invoke', async route => {
    const body = route.request().postDataJSON();
    if (body.action === 'buckets.create') {
      creates.push(body); const response = await route.fetch(); assert.equal(response.status(), 200);
      const value = await response.json(); resource = value.resource.id;
      if (!lost) { lost = true; await route.abort('failed'); return; }
      await route.fulfill({ response }); return;
    }
    if (body.action.startsWith('objects.')) {
      assert(!JSON.stringify(body).includes('binary-browser-sentinel'));
      const response = await route.fetch(); const value = await response.json();
      if (response.status() === 200) {
        for (const key of ['url', 'headers', 'access_key', 'secret_key', 'endpoint', 'bucket']) assert(value[key] === undefined, 'private_producer_value_in_browser');
        if (body.action !== 'objects.observe') descriptors.push(value);
      }
      await route.fulfill({ response }); return;
    }
    await route.continue();
  });
  await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}`);
  await page.getByText('Assign to this project', { exact: true }).click();
  await page.getByRole('list', { name: 'Add a resource' }).getByRole('button').filter({ hasText: 'Manage buckets' }).click();
  const create = page.getByRole('form', { name: 'Manage buckets' });
  await create.getByLabel('Name', { exact: true }).fill('Browser binary acceptance'); await create.getByLabel('Environment', { exact: true }).fill('acceptance');
  await create.getByRole('button', { name: 'Create bucket', exact: true }).click();
  await create.getByRole('button', { name: 'Retry create bucket', exact: true }).waitFor();
  assert.equal(await create.getByLabel('Name', { exact: true }).isDisabled(), true);
  await create.getByRole('button', { name: 'Retry create bucket', exact: true }).click();
  await page.waitForURL('**/resources/object-storage/*'); assert(resource); assert.equal(creates.length, 2); assert.deepEqual(creates[0], creates[1]);
  const count = await database(f, 'platform', false, async db => (await db.query("SELECT count(*)::int AS n FROM webdock_auth.plugin_resource WHERE plugin_id='object-storage'")).rows[0].n); assert.equal(count, 1);
  c.state.resource = resource; await c.save(); delete f.report.nativeResourceCleanup; await f.saveReport();
  const key = 'browser/proof.txt', bytes = Buffer.from('binary-browser-sentinel\n'.repeat(4096)), digest = sha256(bytes);
  await f.write('binary-browser-sentinel.bin', bytes, true);
  await page.getByLabel('File', { exact: true }).setInputFiles({ name: 'browser-proof.txt', mimeType: 'text/plain', buffer: bytes });
  await page.getByLabel('Upload object key', { exact: true }).fill(key);
  await page.getByRole('button', { name: 'Upload object', exact: true }).click();
  await page.getByText(/Upload confirmed\. SHA-256:/).waitFor();
  await page.getByRole('button', { name: 'Inspect object', exact: true }).click();
  await page.getByRole('button', { name: 'Download observed object', exact: true }).waitFor();
  await page.getByRole('button', { name: 'Download observed object', exact: true }).click();
  const save = page.getByRole('link', { name: 'Save verified file', exact: true }); await save.waitFor(); assert((await save.getAttribute('href')).startsWith('blob:'));
  const downloading = page.waitForEvent('download'); await save.click(); const download = await downloading;
  const downloaded = f.path('binary-browser-downloaded.bin'); await download.saveAs(downloaded); await chmod(downloaded, 0o600); assert.equal(sha256(await readFile(downloaded)), digest);
  await page.getByRole('button', { name: 'Hide transfer result', exact: true }).click(); assert.equal(await page.locator('a[download]').count(), 0);
  for (const width of [375, 1280]) {
    await page.setViewportSize({ width, height: 900 }); assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    const path = f.path('binary-console-' + width + '.png'); await page.screenshot({ path, fullPage: true, mask: [page.getByText(f.report.proxy.origin, { exact: false })] }); await chmod(path, 0o600);
  }
  const api = async (path, data) => { const r = await context.request.post(f.report.origins.console + path, { data, headers: { origin: f.report.origins.console } }); return { status: r.status(), value: await r.json() }; };
  const action = async (name, input, projectID = f.report.project) => api('/api/console/plugins/invoke', { pluginID: 'object-storage', projectID, action: name, input, idempotencyKey: randomUUID() });
  assert.equal((await action('objects.observe', { resource_id: resource, key }, f.report.foreignProject)).status, 403);
  // Observe first, then simulate a direct application overwrite of the exact owned null version.
  await page.getByRole('button', { name: 'Inspect object', exact: true }).click(); await page.getByRole('button', { name: 'Download observed object', exact: true }).waitFor();
  const observed = await action('objects.observe', { resource_id: resource, key }); assert.equal(observed.status, 200);
  await f.command('/usr/bin/python3', [join(ROOT, 'plugins/object-storage/fixtures/prepare.py'), '--minio-env', join(ROOT, '.dev-local/minio.env'), '--directory', f.path('binary-minio-observer')]);
  const binding = await database(f, 'metadata', true, async db => JSON.parse((await db.query('SELECT body FROM resources WHERE id=$1', [resource])).rows[0].body).binding);
  await f.command(f.path('binary-minio-observer/mc'), ['--json', '--config-dir', f.path('binary-minio-observer/mc-config'), 'pipe', 'local/' + binding.bucket + '/' + key], { input: 'controlled application replacement\n' });
  await page.getByRole('button', { name: 'Download observed object', exact: true }).click(); await page.getByRole('alert').filter({ hasText: /Data plan unavailable|could not|changed|unavailable/i }).waitFor(); assert.equal(await page.locator('a[download]').count(), 0);
  const stale = await action('objects.download', { resource_id: resource, ...observed.value, expires_at: Math.floor(Date.now() / 1000) + 120 }); assert.notEqual(stale.status, 200);
  await page.getByRole('button', { name: 'Inspect object', exact: true }).click(); await page.getByRole('button', { name: 'Download observed object', exact: true }).waitFor();
  await page.getByLabel('Expected SHA-256 (optional)', { exact: true }).fill('0'.repeat(64));
  await page.getByRole('button', { name: 'Download observed object', exact: true }).click(); await page.getByRole('alert').waitFor(); assert.equal(await page.locator('a[download]').count(), 0);
  await page.getByRole('button', { name: 'Inspect object', exact: true }).click(); await page.getByRole('button', { name: 'Download observed object', exact: true }).waitFor();
  await page.getByText(/Full SHA-256 is unavailable/).waitFor();
  await page.getByRole('button', { name: 'Download observed object', exact: true }).click(); await page.getByRole('link', { name: 'Save length-checked file', exact: true }).waitFor();
  await page.getByRole('button', { name: 'Hide transfer result', exact: true }).click();
  const installed = (await context.request.get(f.report.origins.console + '/api/console/plugins/installed')).ok(); assert(installed);
  const policy = await context.request.get(f.report.origins.console + '/api/console/plugins/object-storage/self-service'); const policyValue = await policy.json();
  const revision = policyValue.revision;
  // Correct supported policy write is PUT, never an invented customer/session row.
  const setPolicy = await context.request.put(f.report.origins.console + '/api/console/plugins/object-storage/self-service/features/manage-buckets', { data: { mode: 'everyone', expectedRevision: revision }, headers: { origin: f.report.origins.console } }); assert.equal(setPolicy.status(), 200);
  assert.equal((await api('/api/tenant-preview/start', { customerID: f.report.customer, mode: 'read_only' })).status, 200);
  const readonlySession = await (await context.request.get(f.report.origins.console + '/api/console/session')).json();
  assert.equal(readonlySession.data.user.preview, true); assert.equal(readonlySession.data.user.acting, false);
  await page.reload(); await page.getByRole('alert').filter({ hasText: 'You do not have access' }).waitFor(); assert.equal(await page.getByRole('button', { name: 'Upload object', exact: true }).count(), 0);
  assert.equal((await action('objects.upload', { resource_id: resource, key: 'denied.txt', exact_bytes: bytes.length, expected_sha256: digest, filename: 'proof.txt', content_type: 'text/plain', expires_at: Math.floor(Date.now() / 1000) + 120 })).status, 403);
  assert.equal((await api('/api/tenant-preview/exit', {})).status, 200);
  const current = await action('objects.observe', { resource_id: resource, key }); assert.equal(current.status, 200);
  const plan = await action('objects.download', { resource_id: resource, ...current.value, expires_at: Math.floor(Date.now() / 1000) + 120 }); assert.equal(plan.status, 200);
  const issued = (await context.cookies()).map(cookie => cookie.name + '=' + cookie.value).join('; ');
  const logout = await context.request.post(f.report.origins.console + '/api/sso/logout', { data: {}, headers: { origin: f.report.origins.console, accept: 'application/json' } }); assert.equal(logout.status(), 200);
  const revoked = await fetch(f.report.origins.console + plan.value.path, { headers: { cookie: issued }, redirect: 'error' }); assert([401, 403].includes(revoked.status)); await revoked.arrayBuffer();
  assert.equal(errors.length, 0); assert(binaryRequests.some(r => r.method === 'PUT' && r.size === bytes.length)); assert(binaryRequests.some(r => r.method === 'GET')); assert(binaryRequests.every(r => !r.headers.some(h => h.startsWith('x-amz-'))));
  await f.write('binary-browser-private-proof.json', { creates, descriptors, binaryRequests, observed: observed.value, staleStatus: stale.status });
  f.report.binaryBrowserProof = { version: f.report.artifacts[0].version, realConsoleCoreNativeProducer: true, realCreateLostResponseRetry: true, identicalCreateInputAndBothIDs: true, exactlyOneResourceCreated: true, uploadBytes: bytes.length, sha256: digest, browserBlobUpload: true, browserVerifiedBlobDownload: true, explicitUnknownChecksumDownload: true, wrongKnownChecksumDenied: true, privateTargetsAbsentFromBrowser: true, wrongTenantDenied: true, readonlyAuthority: 'authenticated-preview-without-project-plugin-entitlement', readonlyPluginViewDenied: true, readonlyUploadDenied: true, projectReaderMembershipQualified: false, revokedOriginalCookieDenied: true, observedNullVersionOverwriteDenied: true, widths: [375, 1280], horizontalOverflow: false, pageErrors: 0 };
  await f.saveReport(); console.log('native_object_storage_binary_browser_passed ' + f.input.runID);
} catch (error) { await f.write('binary-browser-failure-' + Date.now() + '.json', { error: String(error), stack: error.stack }); console.error('native_object_storage_binary_browser_failed_private_diagnostics_retained'); process.exitCode = 1; }
finally { await browser?.close(); }
