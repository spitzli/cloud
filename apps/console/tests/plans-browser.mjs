// Test-only fixtures: an operator defines a plan and puts a customer on it; the customer reads usage.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const meters = [
  { id: 'projects', label: 'Projects', unit: 'count' },
  { id: 'memory', label: 'Memory', unit: 'MiB' },
  { id: 'cpu', label: 'CPU', unit: 'millicores' },
  { id: 'storage', label: 'Storage', unit: 'MiB' },
  { id: 'hosting.app', label: 'App', unit: 'count', plugin: 'hosting' },
];
const state = { plans: [], assignment: { planId: null, overrides: {} }, writes: [] };
const people = {
  operator: { id: '1', name: 'Acceptance operator', role: 'operator' },
  owner: { id: '2', name: 'Ada Owner', role: 'user', member: 'owner' },
};
function usage() {
  const hour = Math.floor(Date.now() / 3600_000) * 3600_000;
  const plan = state.plans.find((p) => p.id === state.assignment.planId) ?? state.plans.find((p) => p.isDefault);
  const limits = Object.fromEntries(Object.entries({ ...(plan?.limits ?? {}), ...state.assignment.overrides }).filter(([, v]) => v !== null));
  return {
    plan: plan ? { id: plan.id, name: plan.name, description: plan.description, assigned: state.assignment.planId !== null } : null,
    overrides: state.assignment.overrides,
    limits,
    allocated: { projects: 2, memory: 768, cpu: 150, storage: 2048, 'hosting.app': 2 },
    meters,
    units: [
      { plugin: 'hosting', unit: 'app-1', project: { id: '201', name: 'Demo project' }, resource: { id: 'a1', name: 'Shop frontend', offering: 'app' }, allocation: { memory: 256, cpu: 64, storage: 0 }, usage: { cpu: 3, memory: 41 }, observedAt: '2026-10-22T10:00:00Z' },
      { plugin: 'postgres', unit: 'pg-1', project: { id: '201', name: 'Demo project' }, resource: { id: 'p1', name: 'Orders', offering: 'dedicated' }, allocation: { memory: 512, cpu: 86, storage: 2048 }, usage: null, observedAt: null },
      { plugin: 'fixture', unit: 'workspace-service', project: null, resource: null, allocation: {}, usage: null, observedAt: null },
    ],
    history: [0, 1, 2, 4, 5].map((h) => ({ hour: new Date(hour - (5 - h) * 3600_000).toISOString(), cpu: 20 + h * 10, memory: 300 + h * 40 })),
  };
}
async function open(browser, label, locale = 'en') {
  const who = people[label];
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: typeof body === 'function' ? body() : body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [], localePreference: locale }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: who.id, name: who.name, email: `${label}@example.test`, role: who.role, emailVerified: true }, session: { id: `s-${label}`, userId: who.id, expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: who.member ?? null }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/v1/operator/registry/customers/101'), json({ id: '101', name: 'Demo customer', status: 'active' }));
  await page.route(path('/api/console/customers/101/self-service'), json([]));
  await page.route(path('/api/console/customers/101/usage'), json(usage));
  await page.route(path('/api/console/plans'), (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: { plans: state.plans.map((p) => ({ ...p, customers: state.assignment.planId === p.id ? 1 : 0 })), meters } });
    const body = route.request().postDataJSON();
    state.writes.push(['POST', body]);
    const id = String(301 + state.plans.length);
    if (body.isDefault) state.plans.forEach((p) => { p.isDefault = false; });
    state.plans.push({ id, ...body, updatedAt: '2026-10-22T10:00:00Z' });
    return route.fulfill({ json: { id } });
  });
  await page.route(path('/api/console/customers/101/plan'), (route) => {
    const body = route.request().postDataJSON();
    state.writes.push(['PUT', body]);
    state.assignment = body;
    return route.fulfill({ json: {} });
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // 1. The operator creates the default plan: 1 GiB memory, three projects, two apps.
  let session = await open(browser, 'operator');
  await session.page.goto(previewOrigin + '/plans');
  await session.page.getByText('No plans yet').waitFor();
  await session.page.getByRole('button', { name: 'New plan' }).click();
  await session.page.getByRole('textbox', { name: 'Name' }).fill('Starter');
  await session.page.getByRole('textbox', { name: 'Memory' }).fill('1024');
  await session.page.getByRole('textbox', { name: 'Projects' }).fill('3');
  await session.page.getByRole('textbox', { name: 'App (hosting)' }).fill('2');
  await session.page.getByRole('checkbox', { name: /Default plan/ }).check();
  await session.page.getByRole('button', { name: 'Save plan' }).click();
  await session.page.getByText('Projects 3, Memory 1 GiB, App 2').waitFor();
  assert.deepEqual(state.writes[0], ['POST', { name: 'Starter', description: '', isDefault: true, limits: { memory: 1024, projects: 3, 'hosting.app': 2 } }]);

  // 2. On the customer page the operator keeps Starter but gives this customer 2 GiB of memory.
  await session.page.goto(previewOrigin + '/customers/101');
  await session.page.getByRole('heading', { name: 'Plan and limits' }).waitFor();
  await session.page.getByRole('combobox', { name: 'Plan', exact: true }).selectOption({ label: 'Starter' });
  await session.page.getByRole('combobox', { name: 'Memory limit' }).selectOption('custom');
  await session.page.getByRole('textbox', { name: 'Memory custom limit (MiB)' }).fill('2048');
  await session.page.getByRole('combobox', { name: 'CPU limit' }).selectOption('unlimited');
  await session.page.getByRole('button', { name: 'Save plan' }).click();
  for (let i = 0; i < 50 && state.writes.length < 2; i++) await session.page.waitForTimeout(100);
  assert.deepEqual(state.writes[1], ['PUT', { planId: '301', overrides: { memory: 2048, cpu: null } }]);
  errors.push(...session.errors);
  await session.context.close();

  // 3. The owner sees the plan, what counts against it, each service and the last hours.
  session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/usage');
  await session.page.getByRole('heading', { name: 'Starter' }).waitFor();
  await session.page.getByText('768 MiB of 2 GiB').waitFor();
  await session.page.getByText('2 of 3').waitFor();
  await session.page.getByText('2 of 2').waitFor();
  await session.page.getByRole('cell', { name: /Shop frontend/ }).waitFor();
  const workspaceService = session.page.getByRole('row').filter({ has: session.page.getByText('workspace-service', { exact: true }) });
  await workspaceService.getByRole('cell', { name: 'Workspace', exact: true }).waitFor();
  await session.page.getByText('Now 500 MiB, peak 500 MiB').waitFor();
  assert.equal(await session.page.locator('svg[aria-label="Memory used over the last 7 days"] path').count(), 2, 'a gap in the samples splits the line');
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();

  // No telemetry is not evidence that a workspace has no running services.
  session = await open(browser, 'owner');
  await session.page.route(url => url.pathname === '/api/console/customers/101/usage', route => route.fulfill({ json: {
    plan: null, overrides: {}, limits: {}, allocated: { projects: 1 }, meters, units: [], history: [],
  } }));
  await session.page.goto(previewOrigin + '/workspaces/101/usage');
  await session.page.getByText('No workspace plan is assigned. Service-specific limits may still apply.').waitFor({ timeout: 1500 });
  await session.page.getByText('No service usage has been reported yet.').waitFor();
  assert.doesNotMatch(await session.page.locator('main').innerText(), /Nothing is limited|No services are running/);
  errors.push(...session.errors);
  await session.context.close();

  session = await open(browser, 'owner', 'de');
  await session.page.route(url => url.pathname === '/api/console/customers/101/usage', route => route.fulfill({ json: {
    plan: null, overrides: {}, limits: {}, allocated: {}, meters, units: [], history: [],
  } }));
  await session.page.goto(previewOrigin + '/workspaces/101/usage');
  await session.page.getByText('Diesem Arbeitsbereich ist kein Tarif zugewiesen. Für einzelne Dienste können weiterhin Limits gelten.').waitFor();
  await session.page.getByText('Es wurden noch keine Nutzungsdaten der Dienste gemeldet.').waitFor();
  await session.page.getByText('Es wurden noch keine reservierten Ressourcen gemeldet.').waitFor();
  errors.push(...session.errors);
  await session.context.close();

  assert.deepEqual(errors, []);
  console.log('Plans checks passed: operator plan, customer assignment with overrides, customer usage with services and history. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
