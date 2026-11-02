// Disposable HTTP fixtures exercise operator review/retry and customer boundaries; no provisioning.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const release = {
  catalogID: 'postgres', pluginID: 'postgres', version: '0.1.0', manifestSha256: 'a'.repeat(64),
  image: `registry.example.test/postgres@sha256:${'b'.repeat(64)}`, registryURL: 'https://registry.example.test', signerKeyID: 'owned-fixture',
  capabilities: ['plugin.workloads'], supportedFeatures: ['create-dedicated'],
  targets: [{ id: 'eu', label: 'EU cluster', reviewSha256: 'c'.repeat(64) }],
  configurationSchema: { type: 'object', properties: {}, additionalProperties: false },
};
const browser = await chromium.launch({ headless: true });
async function open(mode = 'operator') {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage(); page.setDefaultTimeout(5000);
  const state = { catalogReads: 0, writes: [], loseReply: true, observations: 0, complete: false };
  const path = (value) => (url) => url.pathname === value;
  const json = (value) => (route) => route.fulfill({ json: value });
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Fixture operator', email: 'operator@example.test', role: mode === 'operator' ? 'operator' : 'user', preview: mode === 'acting', acting: mode === 'acting', emailVerified: true }, session: { id: 'fixture-session', userId: '2', expiresAt: '2099-01-01T00:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json(mode === 'operator' ? [] : [{ id: '101', name: 'Fixture workspace', status: 'active', role: 'owner' }]));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/api/console/plugins/installed'), json([]));
  await page.route(path('/api/console/tenant-preview/current'), json({ customerID: '101', customerName: 'Fixture workspace', status: 'active', mode: 'owner', readOnly: false, expiresAt: '2099-01-01T00:00:00Z' }));
  await page.route(path('/api/console/plugins/catalog'), (route) => { state.catalogReads++; return route.fulfill({ json: [release] }); });
  await page.route(path('/api/console/plugins/install'), (route) => {
    state.writes.push(route.request().postDataJSON());
    if (state.loseReply) { state.loseReply = false; return route.abort('failed'); }
    return route.fulfill({ json: { id: '1001', status: 'pending' } });
  });
  await page.route(path('/api/console/plugins/operations/1001'), (route) => {
    const current = state.observations++;
    return route.fulfill({ json: { id: '1001', pluginID: 'postgres', kind: 'install', status: state.complete ? 'succeeded' : 'running', phase: state.complete ? 'complete' : 'verifying_runtime', generation: '1', cursor: String(current + 1), condition: null } });
  });
  return { page, context, state };
}
try {
  const { page, context, state } = await open();
  await page.goto(previewOrigin + '/extensions');
  await page.getByRole('heading', { name: 'Available extensions', exact: true }).waitFor();
  await page.getByRole('link', { name: 'Review postgres 0.1.0', exact: true }).click();
  await page.getByRole('heading', { name: 'Install postgres', exact: true }).waitFor();
  await page.getByText('owned-fixture', { exact: true }).waitFor();
  await page.getByText(release.image, { exact: true }).waitFor();
  const install = page.getByRole('button', { name: 'Install extension', exact: true });
  assert.equal(await install.isDisabled(), true);
  await page.getByRole('checkbox', { name: 'I reviewed this release, its permissions and the selected target.' }).check();
  await install.click();
  const retry = page.getByRole('button', { name: 'Retry same request', exact: true });
  await retry.waitFor();
  assert.equal(await page.getByRole('combobox', { name: 'Installation target' }).isDisabled(), true);
  await retry.click();
  await page.waitForURL('**/extensions/operations/1001');
  assert.equal(state.writes.length, 2);
  assert.deepEqual(state.writes[0], state.writes[1], 'lost acceptance reply retains the exact request identity');
  assert.deepEqual({ ...state.writes[0], idempotencyKey: 'checked' }, { catalogID: 'postgres', version: '0.1.0', reviewSha256: 'c'.repeat(64), targetID: 'eu', configuration: {}, approvedCapabilities: ['plugin.workloads'], idempotencyKey: 'checked' });
  await page.getByText('Verifying runtime identity', { exact: true }).waitFor();
  state.complete = true;
  await page.getByText('succeeded', { exact: true }).waitFor();
  await page.getByRole('link', { name: 'Configure self-service', exact: true }).waitFor();
  for (const width of [375, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
  }
  await context.close();
  const recovery = await open();
  await recovery.page.route((url) => url.pathname === '/api/console/plugins/installed', (route) => route.fulfill({ json: [{
    id: 'postgres', kind: 'external', revision: '2', desiredState: 'disabled', state: 'disabled',
    version: release.version, imageDigest: release.image, runtime: 'failed', blockers: [], dependencies: [],
    operationID: '1001', installRecovery: true,
  }] }));
  await recovery.page.goto(previewOrigin + '/extensions');
  await recovery.page.getByRole('link', { name: 'View operation', exact: true }).waitFor();
  assert.equal(await recovery.page.getByRole('button', { name: 'Change availability', exact: true }).count(), 0, 'unpromoted installations recover through their install operation');
  await recovery.context.close();
  for (const mode of ['customer', 'acting']) {
    const fixture = await open(mode);
    await fixture.page.goto(previewOrigin + '/extensions/install/postgres/0.1.0');
    await fixture.page.getByRole('heading', { name: 'Operator access required', exact: true }).waitFor();
    assert.equal(fixture.state.catalogReads, 0);
    assert.equal(fixture.state.writes.length, 0);
    await fixture.context.close();
  }
  console.log('Plugin installation UI passed: exact signed review, approval, uncertain replay, observed completion and customer/acting denial. HTTP fixtures only.');
} finally { await browser.close(); await server.close(); }
