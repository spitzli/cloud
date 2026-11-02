// Test-only fixtures: the hosting plugin's own React page inside a customer project.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import manifest from '@plugins/hosting/manifest.json' with { type: 'json' };
const appID = '00000000-0000-4000-8000-0000000000a1';
const state = { status: 'running', env: ['GREETING'], memory: 128, domains: [], calls: [], deleted: false };
async function open(browser, member, features) {
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
  await page.route(path('/api/console/plugins'), (route) => route.fulfill({ json: [{ state: 'enabled', runtime: 'ready', manifest, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features, resources: state.deleted ? [] : [{ id: appID, offering: 'app', name: 'Website', createdVia: 'self-service' }] }] }));
  await page.route(path(`/api/console/projects/201/plugins/hosting/resources/${appID}/logs`), (route) => {
    const search = new URL(route.request().url()).searchParams.get('search') ?? '';
    const lines = [{ id: '2', at: '2026-10-22T10:00:02Z', unit: 'app-1', line: 'ERROR cache miss storm' }, { id: '1', at: '2026-10-22T10:00:01Z', unit: 'app-1', line: 'GET /stored 200' }];
    return route.fulfill({ json: lines.filter((l) => l.line.toLowerCase().includes(search.toLowerCase())) });
  });
  await page.route(path('/api/console/plugins/invoke'), (route) => {
    const body = route.request().postDataJSON();
    state.calls.push(body);
    const ok = (value = {}) => route.fulfill({ json: value });
    switch (body.action) {
      case 'apps.view': return ok({ name: 'Website', status: state.status, image: 'nginx:stable-alpine', port: 80, memoryMb: state.memory, url: state.status === 'running' ? 'http://website-0123abcd.apps.localhost:55480' : null, env: state.env, error: null, createdAt: '2026-10-22 10:00:00+00', host: 'website-0123abcd.apps.localhost', domains: state.domains });
      case 'apps.logs': return ok({ lines: ['2026-10-22T10:00:00Z GET / 200'] });
      case 'apps.power': state.status = body.input.action === 'stop' ? 'stopped' : 'running'; return ok();
      case 'apps.env': state.env = body.input.env.split('\n').map((line) => line.split('=')[0]); return ok({ status: 'deploying' });
      case 'apps.resize': state.memory = body.input.memory_mb; return ok({ status: 'deploying' });
      case 'apps.domains':
        if (body.input.domains.includes('unverified')) return route.fulfill({ status: 409, json: { error: { code: 'conflict', message: 'Verify the domain of shop.unverified.test before using it.' } } });
        state.domains = body.input.domains.split('\n').filter(Boolean);
        return ok({ status: 'deploying' });
      case 'apps.delete': state.deleted = true; return ok();
      default: return route.fulfill({ status: 404, json: {} });
    }
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // 1. Running app: URL, logs, stop and start.
  let session = await open(browser, 'owner', ['create-apps', 'resize-apps']);
  const page = () => session.page;
  await page().goto(previewOrigin + `/workspaces/101/projects/201/resources/hosting/${appID}`);
  await page().getByRole('link', { name: 'http://website-0123abcd.apps.localhost:55480' }).waitFor();
  await page().getByText('GET / 200').waitFor();
  await page().getByRole('button', { name: 'Stop' }).click();
  await page().getByRole('button', { name: 'Start' }).waitFor();
  await page().getByRole('button', { name: 'Start' }).click();
  await page().getByRole('button', { name: 'Restart' }).waitFor();

  // 1b. Owners look back through the stored log history and search it.
  const history = page().getByRole('region', { name: 'Log history' });
  await history.getByText('GET /stored 200').waitFor();
  await history.getByRole('textbox', { name: 'Search logs' }).fill('error');
  await history.getByRole('button', { name: 'Search' }).click();
  await history.getByText('ERROR cache miss storm').waitFor();
  await page().waitForFunction(() => !document.querySelector('[aria-label="Log history"]')?.textContent?.includes('GET /stored 200'));

  // 2. Variables are replaced as a whole; values never come back.
  await page().getByRole('button', { name: 'Replace variables' }).click();
  await page().getByRole('textbox', { name: 'Variables, one KEY=value per line' }).fill('GREETING=hi\nAPI_TOKEN=s3cret');
  await page().getByRole('button', { name: 'Save and redeploy' }).click();
  await page().getByText('API_TOKEN', { exact: true }).waitFor();
  assert.equal(await page().getByText('s3cret').count(), 0);

  // 2b. Own domains: Core's refusal reaches the customer in its own words; a verified one is listed.
  await page().getByRole('button', { name: 'Change domains' }).click();
  await page().getByRole('textbox', { name: 'Own domains, one per line' }).fill('shop.unverified.test');
  await page().getByRole('button', { name: 'Save and redeploy' }).click();
  await page().getByText('Verify the domain of shop.unverified.test before using it.').waitFor();
  await page().getByRole('textbox', { name: 'Own domains, one per line' }).fill('shop.example.test');
  await page().getByRole('button', { name: 'Save and redeploy' }).click();
  await page().getByRole('list', { name: 'Domains' }).getByText('shop.example.test').waitFor();

  // 3. Memory and delete.
  await page().getByRole('combobox', { name: 'Memory' }).selectOption('512');
  await page().waitForFunction(() => document.querySelector('select')?.value === '512');
  await page().getByRole('textbox', { name: 'Type Website to confirm' }).fill('Website');
  await page().getByRole('button', { name: 'Delete app' }).click();
  await page().getByRole('heading', { name: 'Apps', exact: true }).waitFor();
  assert.equal(state.calls.at(-1).action, 'apps.delete');
  errors.push(...session.errors);
  await session.context.close();

  // 4. Without the resize feature the memory is read-only; members only read.
  state.deleted = false;
  session = await open(browser, 'owner', ['create-apps']);
  await page().goto(previewOrigin + `/workspaces/101/projects/201/resources/hosting/${appID}`);
  await page().getByText('512 MB').waitFor();
  assert.equal(await page().getByRole('combobox', { name: 'Memory' }).count(), 0);
  errors.push(...session.errors);
  await session.context.close();
  session = await open(browser, 'member', []);
  await page().goto(previewOrigin + `/workspaces/101/projects/201/resources/hosting/${appID}`);
  await page().getByRole('region', { name: 'Status' }).waitFor();
  // Logs often carry configuration values, so only owners and admins read them.
  assert.equal(await page().getByRole('region', { name: 'Logs' }).count(), 0);
  assert.equal(await page().getByRole('region', { name: 'Log history' }).count(), 0);
  for (const name of ['Stop', 'Deploy', 'Replace variables', 'Change domains', 'Delete app']) assert.equal(await page().getByRole('button', { name }).count(), 0, name);
  await page().setViewportSize({ width: 390, height: 844 });
  assert.ok(await page().evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();

  assert.deepEqual(errors, []);
  console.log('Hosting plugin UI checks passed: URL and logs, power, variables never shown, own domains with the refusal from Core, searchable log history, memory gated by feature, delete, member read-only. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
