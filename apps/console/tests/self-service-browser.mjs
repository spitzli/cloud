// Test-only fixtures: an operator chooses who may create plugin resources, customers use the project page.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import manifest from '@plugins/echo-demo/manifest.json' with { type: 'json' };
const state = { revision: 0, mode: 'assigned', customers: [], resources: [], puts: [] };
const people = {
  operator: { id: '1', name: 'Acceptance operator', role: 'operator' },
  owner: { id: '2', name: 'Ada Owner', role: 'user', member: 'owner' },
  member: { id: '3', name: 'Mo Member', role: 'user', member: 'member' },
};
function usable(who) {
  if (who.role === 'operator') return ['create-notes'];
  if (who.member !== 'owner') return [];
  return state.mode === 'everyone' || (state.mode === 'selected' && state.customers.includes('101')) ? ['create-notes'] : [];
}
async function open(browser, label, secretCase = false) {
  const viewManifest = structuredClone(manifest);
  if (secretCase) viewManifest.actions.find(a => a.name === "notes.create").input.properties.api_key = { type: "string", maxLength: 2048 };
  const who = people[label];
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: who.id, name: who.name, email: `${label}@example.test`, role: who.role, emailVerified: true }, session: { id: `s-${label}`, userId: who.id, expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: who.member ?? null }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/project/201'), json({ id: '201', name: 'Demo project', customer: { id: '101', name: 'Demo customer' }, role: who.role === 'operator' ? 'operator' : who.member }));
  await page.route(path('/api/console/plugins'), (route) => route.fulfill({ json: [{ state: 'enabled', runtime: 'ready', manifest: viewManifest, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: usable(who), resources: state.resources }] }));
  await page.route(path('/api/console/plugins/invoke'), (route) => {
    const body = route.request().postDataJSON();
    if (body.action === 'notes.create') {
      if (secretCase) { assert.equal(body.input.api_key, 'owned-test-key'); return route.fulfill({ status: 503, json: { error: { message: 'Owned failure' } } }); }
      if (!usable(who).length) return route.fulfill({ status: 403, json: { error: 'permission_denied' } });
      const id = `0000000${state.resources.length + 1}-0000-4000-8000-000000000000`;
      state.resources.push({ id, offering: 'note', name: body.input.name, createdVia: who.role === 'operator' ? 'operator' : 'self-service' });
      return route.fulfill({ json: { resource: { id, name: body.input.name } } });
    }
    if (body.action === 'notes.view') return route.fulfill({ json: { version: 2, title: 'Note', blocks: [{ kind: 'status', label: 'State', state: 'healthy', text: 'Ready' }, { kind: 'reveal', label: 'Access code', action: 'notes.secret' }] } });
    if (body.action === 'notes.secret') return route.fulfill({ json: { value: `echo:${body.input.resource_id}` } });
    if (body.action === 'demo.operator') return route.fulfill({ json: { version: 2, title: 'Echo demo', blocks: [] } });
    return route.fulfill({ status: 404, json: {} });
  });
  await page.route(path('/api/console/plugins/echo-demo/self-service'), (route) => route.fulfill({ json: { revision: String(state.revision), features: [{ id: 'create-notes', label: 'Create notes', description: 'Add and remove demo notes in a project', offering: 'note', mode: state.mode, customers: state.customers.map((id) => ({ id, name: 'Demo customer' })) }] } }));
  await page.route((url) => url.pathname.startsWith('/api/console/plugins/echo-demo/self-service/features/'), (route) => {
    const body = route.request().postDataJSON();
    assert.equal(body.expectedRevision, String(state.revision), 'writes carry the revision they were based on');
    state.puts.push(new URL(route.request().url()).pathname);
    const customer = /customers\/(\d+)$/.exec(route.request().url())?.[1];
    if (customer) state.customers = body.allowed ? [...state.customers, customer] : state.customers.filter((c) => c !== customer);
    else state.mode = body.mode;
    state.revision += 1;
    return route.fulfill({ json: { revision: String(state.revision) } });
  });
  await page.route(path('/v1/operator/registry/customers/101'), json({ id: '101', name: 'Demo customer', status: 'active' }));
  await page.route(path('/api/console/customers/101/self-service'), (route) => route.fulfill({ json: [{ plugin: 'echo-demo', pluginRevision: String(state.revision), feature: 'create-notes', label: 'Create notes', mode: state.mode, allowed: state.customers.includes('101') }] }));
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // 1. The operator lets selected customers create notes, then allows Demo on its customer page.
  let session = await open(browser, 'operator');
  await session.page.goto(previewOrigin + '/extensions/echo-demo');
  const mode = session.page.getByRole('combobox', { name: 'Who can do this themselves' });
  assert.equal(await mode.inputValue(), 'assigned', 'nothing is self-service by default');
  await mode.selectOption('selected');
  await session.page.getByText(/No customers yet\./).waitFor();
  assert.equal(await session.page.getByRole('combobox', { name: 'Add a customer' }).count(), 0, 'customers are chosen on the customer page');
  await session.page.goto(previewOrigin + '/customers/101');
  await session.page.getByRole('checkbox', { name: 'Create notes' }).click();
  await session.page.waitForFunction(() => document.querySelector('[aria-label="Self-service"] input[type=checkbox]')?.checked === true);
  await session.page.goto(previewOrigin + '/extensions/echo-demo');
  await session.page.getByText(/Allowed for Demo customer\./).waitFor();
  errors.push(...session.errors);
  await session.context.close();

  // 2. The Demo owner adds a note from the project page and reveals its code.
  session = await open(browser, 'owner');
  // A single project remains visible in the overview.
  await session.page.goto(previewOrigin + '/workspaces/101');
  await session.page.getByRole('article', { name: 'Demo project' }).waitFor();
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByRole('heading', { name: 'Demo project' }).waitFor();
  await session.page.getByText('Add', { exact: true }).click();
  await session.page.getByRole('button', { name: 'Create notes' }).click();
  await session.page.getByRole('textbox', { name: 'Note name' }).fill('Shopping');
  await session.page.getByRole('button', { name: 'Create', exact: true }).click();
  await session.page.getByRole('heading', { name: 'Shopping' }).waitFor();
  assert.equal(await session.page.getByText(/^echo:/).count(), 0, 'the code stays hidden until asked for');
  await session.page.getByRole('button', { name: 'Show access code' }).click();
  await session.page.getByText(`echo:${state.resources[0].id}`).waitFor();
  errors.push(...session.errors);
  await session.context.close();

  // 3. The operator takes creation back.
  session = await open(browser, 'operator');
  await session.page.goto(previewOrigin + '/extensions/echo-demo');
  await session.page.getByRole('combobox', { name: 'Who can do this themselves' }).selectOption('assigned');
  await session.page.waitForFunction(() => document.querySelector('select')?.value === 'assigned');
  errors.push(...session.errors);
  await session.context.close();

  // 4. The owner keeps the note but can no longer add one.
  session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByRole('link', { name: 'Shopping' }).waitFor();
  assert.equal(await session.page.getByText('Add', { exact: true }).count(), 0);
  await session.page.getByRole('link', { name: 'Shopping' }).click();
  await session.page.getByRole('button', { name: 'Show access code' }).waitFor();
  errors.push(...session.errors);
  await session.context.close();

  // 5. Members see the project's resources and never an Add menu.
  state.mode = 'everyone';
  session = await open(browser, 'member');
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByRole('link', { name: 'Shopping' }).waitFor();
  assert.equal(await session.page.getByText('Add', { exact: true }).count(), 0);
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();

  // API credentials never appear as plain text or remain in a failed create form.
  session = await open(browser, 'owner', true);
  session.page.setDefaultTimeout(5000);
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByText('Add', { exact: true }).click();
  await session.page.getByRole('button', { name: 'Create notes' }).click();
  const credential = session.page.getByLabel('Api key', { exact: true });
  assert.equal(await credential.getAttribute('type'), 'password');
  assert.equal(await credential.getAttribute('autocomplete'), 'new-password');
  await session.page.getByRole('textbox', { name: 'Note name', exact: true }).fill('Retained draft');
  await credential.fill('owned-test-key');
  await session.page.getByRole('button', { name: 'Create', exact: true }).click();
  await session.page.getByRole('alert').waitFor();
  assert.equal(await credential.inputValue(), '');
  assert.equal(await session.page.getByRole('textbox', { name: 'Note name', exact: true }).inputValue(), 'Retained draft');
  errors.push(...session.errors);
  await session.context.close();

  assert.equal(state.puts.length, 3);
  assert.deepEqual(errors, []);
  console.log('Self-service checks passed: operator modes, allow list on the customer page, customer create and reveal, assigned mode keeps resources, member read-only. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
