// Test-only fixtures: a customer adds a domain, publishes its TXT record and verifies it.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const state = { domains: [], published: false };
const view = (d) => ({ ...d, record: { type: 'TXT', name: `_webdock-challenge.${d.domain}`, value: 'a'.repeat(32) } });
async function open(browser, member) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: typeof body === 'function' ? body() : body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: member }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/api/console/tenant/101/domains'), (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: state.domains.map(view) });
    const domain = route.request().postDataJSON().domain.trim().toLowerCase();
    const added = { id: '501', domain, verified: false, verifiedAt: null, checkedAt: null, usedBy: [] };
    state.domains.push(added);
    return route.fulfill({ json: view(added) });
  });
  await page.route((url) => url.pathname.startsWith('/api/console/tenant/101/domains/'), (route) => {
    const domain = state.domains[0];
    if (route.request().method() === 'DELETE') {
      state.domains = [];
      return route.fulfill({ json: {} });
    }
    domain.checkedAt = '2026-10-22T10:00:00Z';
    if (state.published) Object.assign(domain, { verified: true, verifiedAt: '2026-10-22T10:00:00Z' });
    return route.fulfill({ json: view(domain) });
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  let session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/domains');
  await session.page.getByText('No custom domains yet.', { exact: false }).waitFor();
  await session.page.getByRole('textbox', { name: 'Domain' }).fill('Shop.Example.test');
  await session.page.getByRole('button', { name: 'Add domain' }).click();
  await session.page.getByText('_webdock-challenge.shop.example.test').waitFor();
  // Before the record is published the check says so; afterwards the domain is verified.
  await session.page.getByRole('button', { name: 'Check record' }).click();
  await session.page.getByText('The record is not there yet.').waitFor();
  state.published = true;
  await session.page.getByRole('button', { name: 'Check record' }).click();
  await session.page.getByText('Verified', { exact: true }).waitFor();
  assert.equal(await session.page.getByText('_webdock-challenge.shop.example.test').count(), 0, 'a verified domain no longer shows its record');
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  state.domains[0].usedBy = [{ plugin: 'mail', unit: 'engine-c101', project: null, resource: null }];
  await session.page.reload();
  await session.page.getByText('Used by engine-c101 (Workspace)', { exact: true }).waitFor();
  assert.equal(await session.page.getByRole('button', { name: 'Remove', exact: true }).isDisabled(), true, 'customer-wide units keep their domains reserved');
  errors.push(...session.errors);
  await session.context.close();
  // Members read, but neither add nor remove.
  session = await open(browser, 'member');
  await session.page.goto(previewOrigin + '/workspaces/101/domains');
  await session.page.getByText('shop.example.test').waitFor();
  for (const name of ['Add domain', 'Remove', 'Check record']) assert.equal(await session.page.getByRole('button', { name }).count(), 0, name);
  errors.push(...session.errors);
  await session.context.close();
  assert.deepEqual(errors, []);
  console.log('Domain checks passed: add, record shown, check before and after publishing, member read-only. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
