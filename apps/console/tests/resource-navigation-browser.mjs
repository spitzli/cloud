// Owned HTTP fixtures cover product recovery, scoped creation and workspace-safe resources.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import manifest from '@plugins/echo-demo/manifest.json' with { type: 'json' };
const projects = [{ id: '201', name: 'Read-only project', status: 'active' }, { id: '202', name: 'Writable project', status: 'active' }];
async function open(browser, { failed = false, blocked = false, projectFailed = false, preview = false } = {}) {
  const context = await browser.newContext(); const page = await context.newPage(); page.setDefaultTimeout(12000);
  const errors = [], calls = []; const reads = {};
  page.on('pageerror', e => errors.push(e.message));
  page.on('console', message => { if (message.type() === 'error' && /same key|Each child in a list/.test(message.text())) errors.push(message.text()); });
  const resources = { '201': [{ id: 'note-1', name: 'Existing note', offering: 'note', createdVia: 'operator' }], '202': [{ id: 'note-1', name: 'Other note', offering: 'note', createdVia: 'operator' }] };
  await page.route(url => url.pathname === '/console-config.json', route => route.fulfill({ json: { apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] } }));
  await page.route(url => url.pathname === '/v1/auth/session', route => route.fulfill({ json: { data: { user: { id: '2', name: 'QA owner', role: 'user', preview, acting: false, email: 'qa@example.test', emailVerified: true }, session: { id: 'qa-navigation', userId: '2', expiresAt: '2027-01-01T00:00:00Z' } } } }));
  await page.route(url => url.pathname.startsWith('/api/console/'), route => {
    const url = new URL(route.request().url()); const json = data => route.fulfill({ json: data });
    calls.push({ path: url.pathname, project: url.searchParams.get('projectID'), body: route.request().method() === 'POST' ? route.request().postDataJSON() : null });
    if (url.pathname === '/api/console/tenants') return json([{ id: '101', name: 'Alpha', role: 'owner' }, { id: '102', name: 'Beta', role: 'owner' }]);
    if (/\/tenant\/101\/projects$/.test(url.pathname)) return json(projects);
    if (/\/tenant\/102\/projects$/.test(url.pathname)) return json([]);
    if (/\/project\/(201|202)$/.test(url.pathname)) return projectFailed ? route.fulfill({ status: 503, json: { error: { message: 'Project temporarily unavailable' } } }) : json({ ...projects.find(p => url.pathname.endsWith(p.id)), customer: { id: '101', name: 'Alpha' }, role: 'owner' });
    if (url.pathname === '/api/console/plugins') {
      if (failed) return route.fulfill({ status: 500, json: { error: { message: 'Discovery unavailable' } } });
      const id = url.searchParams.get('projectID');
      return json([{ state: 'enabled', runtime: 'ready', manifest, generation: 1, manifestSha256: 'a'.repeat(64), features: id === '202' || blocked ? ['create-notes'] : [], blockedCreateActions: id === '201' && blocked ? ['notes.create'] : [], resources: resources[id] ?? [] }]);
    }
    if (url.pathname === '/api/console/plugins/invoke') {
      const body = route.request().postDataJSON();
      if (body.action === 'notes.create') {
        assert.equal(body.projectID, '202', 'only the eligible project can receive creation');
        const resource = { id: 'created-note', name: body.input.name, offering: 'note', createdVia: 'self-service' };resources['202'].push(resource);return json({ resource });
      }
      reads[body.projectID] = (reads[body.projectID] ?? 0) + 1;
      return json({ version: 2, title: 'Note', blocks: [{ kind: 'text', text: `Content ${body.projectID}:${reads[body.projectID]}` }, { kind: 'actions', actions: [{ id: 'notes.view', label: 'Refresh note' }] }] });
    }
    return json([]);
  });
  return { page, context, calls, errors, recover: () => { failed = false; } };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
try {
  for (const blocked of [false, true]) {
    const s = await open(browser, { blocked }); await s.page.goto(previewOrigin + '/workspaces/101/products/echo-demo');
    await s.page.getByText('Create', { exact: true }).click();
    await s.page.getByRole('button', { name: new RegExp('^' + manifest.offerings[0].label) }).click();
    await s.page.getByRole('group', { name: 'Project destination' }).getByText('Writable project', { exact: true }).waitFor();
    assert.equal(await s.page.getByRole('combobox', { name: 'Project', exact: true }).count(), 0, 'single eligible destination is shown directly');
    await s.page.getByRole('textbox', { name: 'Note name' }).fill('Created note');await s.page.getByRole('button', { name: 'Create', exact: true }).click();
    await s.page.getByRole('heading', { name: 'Created note', exact: true }).waitFor();await s.page.getByRole('link', { name: 'Writable project', exact: true }).waitFor();assert.equal(s.calls.filter(c => c.body?.action === 'notes.create').length, 1);assert.deepEqual(s.errors, []);await s.context.close();
  }
  let s = await open(browser, { failed: true });await s.page.goto(previewOrigin + '/workspaces/101/products/echo-demo');
  await s.page.getByRole('button', { name: 'Try again' }).waitFor();s.recover();await s.page.getByRole('button', { name: 'Try again' }).click();
  await s.page.getByText('Existing note', { exact: true }).waitFor();assert.deepEqual(s.errors, []);await s.context.close();
  for (const path of ['/workspaces/102/projects/201', '/workspaces/102/projects/201/resources/echo-demo/note-1']) {
    s = await open(browser);await s.page.goto(previewOrigin + path);await s.page.getByText('This project is not available in this workspace.', { exact: true }).waitFor();
    assert.equal(s.calls.filter(c => c.path === '/api/console/plugins/invoke').length, 0);assert.deepEqual(s.errors, []);await s.context.close();
  }
  s = await open(browser, { projectFailed: true });await s.page.goto(previewOrigin + '/workspaces/101/projects/201/resources/echo-demo/note-1');
  await s.page.getByRole('button', { name: 'Try again' }).waitFor();assert.equal(s.calls.filter(c => c.path === '/api/console/plugins/invoke').length, 0);await s.context.close();
  for (const path of ['/workspaces/101/products/echo-demo', '/workspaces/101/projects/202']) {
    s = await open(browser, { preview: true });await s.page.goto(previewOrigin + path);
    await s.page.getByRole('heading', { name: path.includes('/products/') ? 'Notes' : 'Writable project', exact: true }).waitFor();
    assert.equal(await s.page.getByText('Create', { exact: true }).count(), 0);assert.equal(await s.page.getByText('Add', { exact: true }).count(), 0);assert.deepEqual(s.errors, []);await s.context.close();
  }
  s = await open(browser);await s.page.goto(previewOrigin + '/workspaces/101/projects/201/resources/echo-demo/note-1');
  await s.page.getByText('Content 201:1', { exact: true }).waitFor();await s.page.getByRole('button', { name: 'Refresh note' }).click();await s.page.getByText('Content 201:2', { exact: true }).waitFor();
  await s.page.evaluate(() => { history.pushState({}, '', '/workspaces/101/projects/202/resources/echo-demo/note-1');window.dispatchEvent(new PopStateEvent('popstate')); });
  await s.page.getByRole('heading', { name: 'Other note', exact: true }).waitFor();await s.page.getByText('Content 202:1', { exact: true }).waitFor();
  assert.equal(await s.page.getByText('Content 201:2', { exact: true }).count(), 0, 'resource state does not cross projects with reused IDs');assert.deepEqual(s.errors, []);await s.context.close();
  console.log('Product retry, per-project create eligibility, blocked actions and workspace/resource authority states PASS. Owned HTTP fixtures only.');
} finally { await browser.close(); await server.close(); }
