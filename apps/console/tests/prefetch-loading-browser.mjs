// Slow lazy chunks must preserve the existing shell and Suspense loading state.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';

async function navigate(page, navigation) {
  if (navigation === 'direct') {
    await page.goto(previewOrigin + '/workspaces/101/connections', { waitUntil: 'domcontentloaded' });
    return;
  }
  await page.goto(previewOrigin + '/workspaces/101');
  const link = page.getByRole('link', { name: 'Connections', exact: true }).first();
  await link.waitFor();
  // A touch/programmatic click can navigate without a prior hover/focus preload.
  await link.evaluate(element => element.click());
}

const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
try {
  for (const navigation of ['direct', 'in-app']) {
    const context = await browser.newContext();
    const page = await context.newPage();
    page.setDefaultTimeout(6000);
    let release;
    const chunkGate = new Promise(resolve => { release = resolve; });
    let sessionRequests = 0;
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route(url => /\/Connections(?:-[^/]+\.js|\.tsx)$/.test(url.pathname), async route => {
      await chunkGate;
      await route.continue();
    });
    await page.route(url => url.pathname === '/console-config.json', route => route.fulfill({ json: { apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] } }));
    await page.route(url => url.pathname === '/v1/auth/session', route => {
      sessionRequests++;
      return route.fulfill({ json: { data: { user: { id: '2', name: 'Owner', email: 'qa@example.test', role: 'user', emailVerified: true }, session: { id: 'slow-chunk-session', userId: '2', expiresAt: '2027-01-01T00:00:00Z' } } } });
    });
    await page.route(url => url.pathname.startsWith('/api/console/'), route => {
      const path = new URL(route.request().url()).pathname;
      const fixtures = {
        '/api/console/tenants': [{ id: '101', name: 'Alpha', role: 'owner' }],
        '/api/console/byok/neon': { data: { canManage: true, connection: null } },
      };
      return route.fulfill({ json: fixtures[path] ?? [] });
    });
    try {
      const chunk = page.waitForRequest(request => /\/Connections(?:-[^/]+\.js|\.tsx)$/.test(new URL(request.url()).pathname));
      await navigate(page, navigation);
      await chunk;
      await page.getByText('Loading connections…', { exact: true }).waitFor({ state: 'visible' });
      assert.ok(sessionRequests > 0, navigation + ': loading the chunk must not block session bootstrap');
      assert.ok(await page.locator('main').isVisible(), navigation + ': shell remains visible');
      release();
      await page.getByRole('heading', { name: 'Connections', exact: true }).waitFor();
      assert.deepEqual(errors, []);
      console.log('Slow chunk ' + navigation + ' PASS: visible shell/loading state before Connections JavaScript arrives.');
    } finally {
      release();
      await context.close();
    }
  }
} finally {
  await browser.close();
  await server.close();
}
