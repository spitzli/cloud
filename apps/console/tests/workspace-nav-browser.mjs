// Test-only fixtures: a customer workspace navigated by product from the sidebar.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import postgres from '@plugins/postgres/manifest.json' with { type: 'json' };
import mail from '@plugins/mail/manifest.json' with { type: 'json' };
const projects = [{ id: '201', name: 'Shop', status: 'active' }, { id: '202', name: 'Blog', status: 'active' }];
const resources = { 201: [{ id: '00000000-0000-4000-8000-000000000001', offering: 'shared', name: 'orders', createdVia: 'self-service' }], 202: [{ id: '00000000-0000-4000-8000-000000000002', offering: 'dedicated', name: 'analytics', createdVia: 'operator' }] };
async function open(browser, role, workspaceProjects = projects) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's-ada', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Acme', status: 'active', role }]));
  const created = [];
  await page.route(path('/api/console/tenant/101/projects'), (route) => {
    if (route.request().method() === 'POST') {
      created.push(route.request().postDataJSON().name);
      workspaceProjects.push({ id: '209', name: 'Default', status: 'active' });
      return route.fulfill({ json: { id: '209' } });
    }
    return route.fulfill({ json: workspaceProjects });
  });
  const features = role === 'member' ? [] : ['create-shared', 'create-dedicated'];
  await page.route(path('/api/console/plugins'), (route) => {
    const project = new URL(route.request().url()).searchParams.get('projectID');
    const view = (m, f, r) => ({ state: 'enabled', runtime: 'ready', manifest: m, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: f, resources: r });
    return route.fulfill({ json: [view(postgres, features, resources[project] ?? []), view(mail, [], [])] });
  });
  return { page, context, errors, created };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // 1. A customer with one workspace starts inside it; the sidebar lists its products.
  let session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/');
  await session.page.waitForURL(/\/workspaces\/101$/);
  const sidebar = session.page.getByRole('link', { name: /^Databases/ }).first();
  await sidebar.waitFor();
  assert.match(await sidebar.innerText(), /2/, 'the entry counts the workspace resources');
  assert.equal(await session.page.getByRole('link', { name: /^Mail/ }).count(), 0, 'products without access or resources stay hidden');
  assert.equal(await session.page.getByRole('link', { name: 'Extensions' }).count(), 0, 'operator pages are not offered to customers');

  // 2. The product page lists everything across projects and creates into a chosen project.
  await sidebar.click();
  await session.page.getByRole('heading', { name: 'Databases', exact: true }).waitFor();
  for (const text of ['orders', 'analytics', 'Shop', 'Blog', 'added by your provider']) await session.page.getByText(text, { exact: true }).waitFor();
  // Every plugin explains its offerings: below the list, in the create menu and above the form.
  await session.page.getByText('Which one do I need?').click();
  await session.page.getByText('Own database and login, invisible to other customers').waitFor();
  await session.page.getByText('Create', { exact: true }).click();
  await session.page.getByRole('button', { name: /^Shared database A database of your own/ }).click();
  await session.page.getByRole('paragraph').filter({ hasText: /^A database of your own on a PostgreSQL server/ }).first().waitFor();
  await session.page.getByRole('combobox', { name: 'Project' }).selectOption('202');
  await session.page.getByRole('textbox', { name: 'Name' }).waitFor();
  errors.push(...session.errors);
  await session.context.close();

  // 3. Members see the same resources but can neither create nor manage members.
  session = await open(browser, 'member');
  await session.page.goto(previewOrigin + '/workspaces/101/products/postgres');
  await session.page.getByText('orders', { exact: true }).waitFor();
  assert.equal(await session.page.getByText('Create', { exact: true }).count(), 0);
  assert.equal(await session.page.getByRole('link', { name: 'Members' }).count(), 0);
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();

  // 4. A workspace holds the Default project the server created with it; with one project the
  // project level stays out of sight and the console creates nothing.
  const single = [{ id: '209', name: 'Default', status: 'active' }];
  session = await open(browser, 'owner', single);
  await session.page.goto(previewOrigin + '/workspaces/101');
  await session.page.getByText('Everything lives in one project for now.').waitFor();
  assert.deepEqual(session.created, [], 'the console creates no project');
  await session.page.getByRole('button', { name: 'Add a project' }).waitFor();
  await session.page.goto(previewOrigin + '/workspaces/101/products/postgres');
  await session.page.getByRole('heading', { name: 'Databases', exact: true }).waitFor();
  assert.equal(await session.page.getByRole('columnheader', { name: 'Project' }).count(), 0);
  errors.push(...session.errors);
  await session.context.close();

  assert.deepEqual(errors, []);
  console.log('Workspace navigation checks passed: single workspace landing, product sidebar with counts, cross-project product page, offering help, create into a chosen project, member read-only, single-project view without console-created projects. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
