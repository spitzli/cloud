// Test-only fixtures: an operator offers a cluster, an owner moves an empty project onto it.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const state = { offered: false, cluster: null, movable: true, puts: [] };
const fra = { id: '301', name: 'Frankfurt 1', region: 'eu-central', country: 'DE' };
async function open(browser, who) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  const operator = who === 'operator';
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada', email: 'ada@example.test', role: operator ? 'operator' : 'user', emailVerified: true }, session: { id: 's', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: operator ? null : 'owner' }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/project/201'), json({ id: '201', name: 'Demo project', customer: { id: '101', name: 'Demo customer' }, role: operator ? 'operator' : 'owner' }));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/api/console/clusters'), (route) => route.fulfill({ json: [{ ...fra, verified: true, offered: state.offered, dedicatedTo: null, projects: state.cluster ? 1 : 0 }] }));
  await page.route(path('/api/console/clusters/301'), (route) => { state.offered = route.request().postDataJSON().offered; return route.fulfill({ json: {} }); });
  const placement = () => ({ cluster: state.cluster, options: state.offered ? [{ ...fra, dedicated: false }] : [], movable: state.movable });
  await page.route(path('/api/console/projects/201/placement'), (route) => {
    if (route.request().method() === 'PUT') { state.puts.push(route.request().postDataJSON()); state.cluster = route.request().postDataJSON().clusterId; }
    return route.fulfill({ json: placement() });
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // Without an offered cluster there is nothing to choose, so the project shows no placement.
  let session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByRole('heading', { name: 'Demo project' }).waitFor();
  await session.page.waitForTimeout(500);
  assert.equal(await session.page.getByRole('region', { name: 'Runs on' }).count(), 0);
  errors.push(...session.errors);
  await session.context.close();
  session = await open(browser, 'operator');
  await session.page.goto(previewOrigin + '/clusters');
  await session.page.getByRole('checkbox', { name: 'Offered' }).click();
  await session.page.waitForFunction(() => document.querySelector('input[type=checkbox]')?.checked === true);
  assert.equal(state.offered, true);
  errors.push(...session.errors);
  await session.context.close();
  session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByRole('combobox', { name: 'Cluster' }).selectOption({ label: 'Frankfurt 1 (eu-central, DE)' });
  await session.page.waitForFunction(() => document.querySelector('[aria-label="Runs on"] select')?.value === '301');
  assert.deepEqual(state.puts, [{ clusterId: '301' }]);
  // Once services run, the choice is shown but fixed.
  state.movable = false;
  await session.page.reload();
  await session.page.getByText('Frankfurt 1 (eu-central, DE)').waitFor();
  assert.equal(await session.page.getByRole('combobox', { name: 'Cluster' }).count(), 0);
  errors.push(...session.errors);
  await session.context.close();
  assert.deepEqual(errors, []);
  console.log('Placement checks passed: hidden without a choice, operator offers, owner chooses while empty, fixed once services run. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
