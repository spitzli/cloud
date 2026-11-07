// Disposable HTTP fixtures: workspace overview uses project data, never provider invocations.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import neon from '@plugins/neon/manifest.json' with { type: 'json' };
const projects = [{ id: '201', name: 'Shop', status: 'active' }, { id: '202', name: 'Admin Panel', status: 'active' }];
async function open(browser, { role = 'owner', count = 2, unavailable = false, preview = false, acting = false, pluginsUnavailable = false, hosting = false, nativeUnavailable = false, alert = false } = {}) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 960 } });
  const page = await context.newPage(); const errors = []; const calls = [];
  page.on('pageerror', e => errors.push(e.message));
  await page.route(url => url.pathname.startsWith('/api/console/'), route => {
    const url = new URL(route.request().url()); calls.push({ path: url.pathname, method: route.request().method() });
    const send = body => route.fulfill({ json: body });
    if (url.pathname === '/api/console/tenants') return send([{ id: '101', name: 'Acme', status: 'active', role }]);
    if (url.pathname === '/api/console/tenant/101/projects') return send(projects.slice(0, count));
    if (url.pathname === '/api/console/plugins' && pluginsUnavailable) return route.fulfill({ status: 503, json: { error: { message: 'Plugin discovery unavailable' } } });
    if (url.pathname === '/api/console/plugins') return send([{ state: 'enabled', runtime: 'ready', manifest: neon, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: [], resources: [{ id: 'db-' + url.searchParams.get('projectID'), offering: neon.offerings[0].id, name: 'project-db', createdVia: 'operator' }] }]);
    if (url.pathname === '/api/console/hosting') return nativeUnavailable ? route.fulfill({ status: 503, json: { error: { message: 'Hosting unavailable' } } }) : send({ effective: { apps: 4, cpuMillicores: 800, memoryBytes: 512000000, volumeBytes: null, ephemeralBytes: 1000000000, replicasPerApp: 3, concurrentDeployments: 2 }, allocated: { apps: 2, cpuMillicores: 300, memoryBytes: 256000000, volumeBytes: 0, ephemeralBytes: 64000000, replicasPerApp: 2, concurrentDeployments: 0 }, reserved: { apps: 1, cpuMillicores: 200, memoryBytes: 64000000, volumeBytes: 0, ephemeralBytes: 32000000, replicasPerApp: 3, concurrentDeployments: 1 } });
    if (url.pathname.endsWith('/usage')) return unavailable ? route.fulfill({ status: 503, json: { error: { message: 'Usage unavailable' } } }) : send({ plan: { id: '301', name: 'Starter', description: '', assigned: true }, overrides: {}, limits: { projects: 3, cpu: 500, memory: 512 }, allocated: { projects: 2, cpu: 250, memory: 256 }, meters: [{ id: 'projects', label: 'Projects', unit: 'count' }, { id: 'cpu', label: 'CPU', unit: 'millicores' }, { id: 'memory', label: 'Memory', unit: 'MiB' }], units: [], history: [] });
    if (url.pathname.endsWith('/notifications')) return unavailable ? route.fulfill({ status: 503, json: { error: { message: 'Alerts unavailable' } } }) : send(alert ? [{ id: 'notice-1', kind: 'crashing', plugin: 'hosting', unit: 'app-qa', project: projects[0], resource: { id: 'app-qa', name: 'service-' + 'x'.repeat(180) }, openedAt: '2026-11-07T10:00:00Z', resolvedAt: null }] : []);
    return send([]);
  });
  await page.route(url => url.pathname === '/console-config.json', route => route.fulfill({ json: { apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: hosting ? ['hosting'] : [] } }));
  await page.route(url => url.pathname === '/v1/auth/session', route => route.fulfill({ json: { data: { user: { id: '2', name: 'Ada', role: 'user', preview, acting, email: 'ada@example.test', emailVerified: true }, session: { id: 'home-qa', userId: '2', expiresAt: '2027-01-01T00:00:00Z' } } } }));
  await page.goto(previewOrigin + '/workspaces/101');
  await page.getByRole('heading', { name: 'Acme', exact: true }).waitFor();
  return { context, page, errors, calls };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
try {
  const artifacts = '.runtime-test/workspace-home-20261107'; await mkdir(artifacts, { recursive: true });
  let s = await open(browser);
  const main = s.page.getByRole('main');
  await main.getByRole('article', { name: 'Shop' }).waitFor();
  assert.equal(await main.getByRole('list', { name: 'Products', exact: true }).count(), 0, 'workspace home is project-first, not a provider catalog');
  await main.getByRole('article', { name: 'Admin Panel' }).waitFor();
  assert.match(await main.getByRole('article', { name: 'Shop' }).getByRole('link', { name: 'Shop', exact: true }).getAttribute('href'), /\/projects\/201$/);
  await main.getByRole('region', { name: 'Plan and usage' }).getByText('Starter', { exact: true }).waitFor();
  await main.getByText('No open alerts.', { exact: true }).waitFor();
  const usageReads = s.calls.filter(c => c.path.endsWith('/usage')).length;
  const alertReads = s.calls.filter(c => c.path.endsWith('/notifications')).length;
  await main.getByRole('button', { name: 'Refresh workspace' }).click();
  await s.page.waitForFunction(() => !document.querySelector('[aria-label="Refresh workspace"]')?.disabled);
  assert.equal(s.calls.filter(c => c.path.endsWith('/usage')).length, usageReads + 1, 'usage refreshes only on explicit request');
  assert.equal(s.calls.filter(c => c.path.endsWith('/notifications')).length, alertReads + 1, 'alerts can be refreshed explicitly');
  const search = main.getByRole('searchbox', { name: 'Search projects' });
  await search.fill('admin'); assert.equal(await main.getByRole('article').count(), 1); await main.getByRole('article', { name: 'Admin Panel' }).waitFor();
  await search.fill('missing'); await main.getByText('No matching projects', { exact: true }).waitFor(); await search.fill('');
  const viewports = ['light', 'dark'].flatMap(theme => [1440, 390, 320].map(width => ({ theme, width })));
  for (const { theme, width } of viewports) {
    await s.page.evaluate(theme => document.documentElement.dataset.theme = theme, theme); await s.page.setViewportSize({ width, height: 960 });
    assert(await s.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${theme}/${width} fits viewport`);
    if ((theme === 'dark' && width === 1440) || (theme === 'light' && width === 390)) await main.locator('section').first().screenshot({ path: `${artifacts}/${theme}-${width}.png`, animations: 'disabled' });
  }
  assert(!s.calls.some(c => c.method !== 'GET' || /invoke/.test(c.path)), 'overview never creates resources or invokes Neon');
  assert.deepEqual(s.errors, []); await s.context.close();
  for (const count of [0, 1]) {
    s = await open(browser, { count });
    if (count === 0) await s.page.getByText('No projects yet', { exact: true }).waitFor();
    else await s.page.getByRole('article', { name: 'Shop' }).waitFor();
    await s.page.getByRole('button', { name: 'New project', exact: true }).click(); await s.page.getByRole('textbox', { name: 'Project name' }).waitFor();
    assert.deepEqual(s.errors, []); await s.context.close();
  }
  s = await open(browser, { role: 'member', unavailable: true });
  await s.page.getByText('Usage is unavailable.', { exact: true }).waitFor(); await s.page.getByText('Alerts are unavailable.', { exact: true }).waitFor();
  assert.equal(await s.page.getByRole('button', { name: 'New project', exact: true }).count(), 0);
  assert.equal(await s.page.getByText('No open alerts.', { exact: true }).count(), 0, 'errors are not a healthy empty state');
  await s.page.getByRole('article', { name: 'Shop' }).waitFor(); assert.deepEqual(s.errors, []); await s.context.close();
  s = await open(browser, { preview: true, pluginsUnavailable: true });
  await s.page.getByText('Resource details unavailable.', { exact: true }).first().waitFor();
  assert.equal(await s.page.getByRole('button', { name: 'New project', exact: true }).count(), 0, 'read-only previews cannot create projects');
  assert.equal(await s.page.getByText('No resources yet', { exact: true }).count(), 0, 'unknown discovery does not imply zero resources');
  assert.deepEqual(s.errors, []); await s.context.close();
  s = await open(browser, { hosting: true, unavailable: true });
  await s.page.getByText('Usage is unavailable.', { exact: true }).waitFor();
  await s.page.getByLabel('Hosting reservations', { exact: true }).getByText('3 of 4', { exact: true }).waitFor();
  assert.deepEqual(s.errors, []); await s.context.close();
  s = await open(browser, { hosting: true, nativeUnavailable: true });
  await s.page.getByText('Hosting usage is unavailable.', { exact: true }).waitFor();
  assert.equal(await s.page.getByLabel('Hosting reservations', { exact: true }).count(), 0);
  assert.deepEqual(s.errors, []); await s.context.close();
  s = await open(browser, { alert: true });
  const notice = s.page.getByRole('region', { name: 'Needs attention', exact: true });
  await notice.getByRole('link', { name: /^service-/ }).waitFor();
  assert.match(await notice.getByRole('link').getAttribute('href'), /\/projects\/201\/resources\/hosting\/app-qa$/);
  await s.page.setViewportSize({ width: 320, height: 960 });
  assert(await s.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'long alert names fit narrow screens');
  assert.deepEqual(s.errors, []); await s.context.close();
  console.log('Workspace home: project cards/search, no provider catalog, real usage/alerts, zero/one projects, member access, failures and responsive themes PASS. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
