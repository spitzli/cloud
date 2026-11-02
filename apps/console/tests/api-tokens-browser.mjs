// Test-only fixtures: an owner creates an API token, sees it once, and revokes it; members see no page.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const state = { tokens: [], posts: [] };
async function open(browser, member) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, workspaceApiOrigin: 'https://api.example.test', authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: member }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/api/console/tenant/101/api-tokens'), (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: state.tokens });
    const body = route.request().postDataJSON();
    state.posts.push(body);
    const token = { id: String(600 + state.tokens.length), name: body.name, canWrite: body.canWrite, createdAt: '2026-10-22T10:00:00Z', expiresAt: '2027-01-20T10:00:00Z', lastUsedAt: null, createdBy: 'Ada' };
    state.tokens.push(token);
    return route.fulfill({ json: { ...token, token: 'wdk_' + 'f'.repeat(64) } });
  });
  await page.route((url) => url.pathname.startsWith('/api/console/tenant/101/api-tokens/'), (route) => {
    state.tokens = state.tokens.filter((t) => !route.request().url().endsWith('/' + t.id));
    return route.fulfill({ json: {} });
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  let session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101');
  await session.page.getByRole('link', { name: 'API tokens' }).click();
  await session.page.getByText('No tokens yet.').waitFor();
  await session.page.getByRole('textbox', { name: 'Name' }).fill('CI deploy');
  await session.page.getByRole('combobox', { name: 'Access' }).selectOption('write');
  await session.page.getByRole('combobox', { name: 'Expires after' }).selectOption('30');
  await session.page.getByRole('button', { name: 'Create token' }).click();
  await session.page.getByText('wdk_' + 'f'.repeat(64)).waitFor();
  assert.equal(await session.page.getByRole('status').locator('code').last().textContent(), 'https://api.example.test/v1/api/projects', 'scripts must use the canonical API host, with a slash before v1');
  assert.deepEqual(state.posts[0], { name: 'CI deploy', canWrite: true, days: 30 });
  await session.page.getByText(/Read and change, by Ada/).waitFor();
  // After a reload the secret is gone for good.
  await session.page.reload();
  await session.page.getByText(/Read and change, by Ada/).waitFor();
  assert.equal(await session.page.getByText('wdk_' + 'f'.repeat(64)).count(), 0);
  await session.page.getByRole('button', { name: 'Revoke' }).click();
  await session.page.getByText('No tokens yet.').waitFor();
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();
  session = await open(browser, 'member');
  await session.page.goto(previewOrigin + '/workspaces/101');
  await session.page.getByRole('link', { name: 'Domains' }).waitFor();
  assert.equal(await session.page.getByRole('link', { name: 'API tokens' }).count(), 0, 'members do not manage tokens');
  errors.push(...session.errors);
  await session.context.close();
  assert.deepEqual(errors, []);
  console.log('API token checks passed: create with access and expiry, secret shown once, revoke, members without the page. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
