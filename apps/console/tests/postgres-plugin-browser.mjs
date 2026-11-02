// Test-only fixtures: the PostgreSQL plugin's own React page inside a customer project.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import manifest from '@plugins/postgres/manifest.json' with { type: 'json' };
const id = '00000000-0000-4000-8000-000000000001';
const state = { firstView: 0, calls: [], deleted: false };
async function open(browser, member) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada Owner', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's-ada', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: member }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/project/201'), json({ id: '201', name: 'Demo project', customer: { id: '101', name: 'Demo customer' }, role: member }));
  await page.route(path('/api/console/plugins'), (route) => route.fulfill({ json: [{ state: 'enabled', runtime: 'ready', manifest, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: member === 'owner' ? ['create-dedicated'] : [], resources: state.deleted ? [] : [{ id, offering: 'dedicated', name: 'Analytics', createdVia: 'self-service' }] }] }));
  await page.route(path('/api/console/plugins/invoke'), (route) => {
    const body = route.request().postDataJSON();
    state.calls.push(body.action);
    assert.equal(body.input.resource_id, id);
    if (body.action === 'databases.view') {
      state.firstView ||= Date.now();
      const ready = Date.now() - state.firstView > 1500;
      return route.fulfill({ json: { id, name: 'Analytics', offering: 'dedicated', state: ready ? 'ready' : 'creating', host: '127.0.0.1', port: ready ? 49152 : null, database: 'app', user: 'wd_u_abc', version: '18', server: null, error: null, createdAt: '2026-10-22 09:00:00+00', sslMode: 'disable' } });
    }
    if (body.action === 'databases.credentials') return route.fulfill({ json: { value: 'f00dfeed', uri: 'postgresql://wd_u_abc:f00dfeed@127.0.0.1:49152/app' } });
    if (body.action === 'databases.rotate') return route.fulfill({ json: {} });
    if (body.action === 'instances.delete') { state.deleted = true; return route.fulfill({ json: {} }); }
    if (state.deleted) return route.fulfill({ status: 403, json: { error: 'permission_denied' } });
    return route.fulfill({ status: 404, json: {} });
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // 1. A new instance is still starting; the page updates by itself once it is ready.
  let session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByRole('link', { name: 'Analytics' }).click();
  await session.page.getByText('Creating', { exact: true }).waitFor();
  await session.page.getByText('Ready', { exact: true }).waitFor({ timeout: 10000 });
  await session.page.getByText('Dedicated PostgreSQL 18').waitFor();
  assert.equal(await session.page.getByText('f00dfeed').count(), 0, 'the password stays hidden until asked for');
  await session.page.getByRole('button', { name: 'Show password' }).click();
  await session.page.getByText('postgresql://wd_u_abc:f00dfeed@127.0.0.1:49152/app').waitFor();
  await session.page.getByRole('button', { name: 'Rotate password' }).click();
  await session.page.getByRole('button', { name: 'Rotated' }).waitFor();

  // 2. Deleting asks for the name, then returns to the project.
  await session.page.getByRole('button', { name: 'Delete', exact: true }).click();
  const confirm = session.page.getByRole('button', { name: 'Delete permanently' });
  assert.ok(await confirm.isDisabled());
  await session.page.getByRole('textbox', { name: 'Type Analytics to confirm' }).fill('Analytics');
  await confirm.click();
  await session.page.getByRole('heading', { name: 'Databases', exact: true }).waitFor();
  await session.page.waitForTimeout(500);
  assert.equal(state.calls.at(-1), 'instances.delete', 'nothing reads the deleted instance again');
  errors.push(...session.errors);
  await session.context.close();

  // 3. Members see where to connect, but neither the password nor the controls.
  state.deleted = false;
  session = await open(browser, 'member');
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/postgres/${id}`);
  await session.page.getByText('Host', { exact: true }).waitFor();
  assert.equal(await session.page.getByRole('region', { name: 'Manage' }).count(), 0);
  assert.equal(await session.page.getByRole('button', { name: 'Show password' }).count(), 0, 'members never see the password');
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();

  assert.deepEqual(errors, []);
  console.log('PostgreSQL plugin UI checks passed: creating to ready, hidden password, rotate, confirmed delete, member read-only. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
