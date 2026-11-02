// Test-only fixtures: the workspace overview shows what needs attention and links to the resource.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const notices = [
  { id: '1', kind: 'crashing', plugin: 'hosting', unit: 'app-1', project: { id: '201', name: 'Demo project' }, resource: { id: 'a1', name: 'Shop frontend' }, openedAt: '2026-10-22T10:00:00Z', resolvedAt: null },
  { id: '2', kind: 'image', plugin: 'hosting', unit: 'app-2', project: { id: '201', name: 'Demo project' }, resource: { id: 'a2', name: 'Old worker' }, openedAt: '2026-10-21T10:00:00Z', resolvedAt: '2026-10-21T11:00:00Z' },
  { id: '3', kind: 'refused', plugin: 'fixture', unit: 'workspace-service', project: null, resource: null, openedAt: '2026-10-22T10:00:00Z', resolvedAt: null },
];
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: 'member' }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/api/console/customers/101/notifications'), json(notices));
  await page.goto(previewOrigin + '/workspaces/101');
  const region = page.getByRole('region', { name: 'Needs attention' });
  await region.getByText(/keeps stopping right after it starts/).waitFor();
  await region.getByText(/workspace-service.*in this workspace.*was refused/).waitFor();
  assert.equal(await region.getByRole('link', { name: 'workspace-service' }).count(), 0);
  assert.equal(await region.getByText('Old worker').count(), 0, 'resolved notices are not shown as open');
  assert.equal(await region.getByRole('link', { name: 'Shop frontend' }).getAttribute('href'), '/workspaces/101/projects/201/resources/hosting/a1');
  assert.deepEqual(errors, []);
  console.log('Notice checks passed: open notices on the overview with a link to the resource, resolved ones hidden. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
