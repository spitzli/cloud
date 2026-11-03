// Disposable HTTP fixtures exercise the real UI; no customer data, mail or provider I/O.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { mkdir, readFile } from 'node:fs/promises';
import { server, previewOrigin } from './preview-server.ts';
import { fromRepo } from './repo.mjs';

const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const failures = [];
const path = (value) => (url) => url.pathname === value;
const future = '2099-01-01T00:00:00Z';
async function open(role = 'owner', surface = 'console', signedIn = true) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  page.setDefaultTimeout(4000);
  const state = { failChange: false, calls: [], operatorCalls: [], members: [
    { id: '601', name: 'Ada Owner', email: 'ada@example.test', role: 'owner' },
    { id: '602', name: 'Grace Admin', email: 'grace@example.test', role: 'admin' },
  ] };
  const user = { id: '2', name: 'Ada Owner', email: 'ada@example.test', role: role === 'operator' ? 'operator' : 'user', emailVerified: true, twoFactorEnabled: true };
  const session = { user, session: { id: 'session-ui', userId: user.id, expiresAt: future } };
  const json = (data) => (route) => route.fulfill({ json: data });
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [], ...(surface === 'account' ? { surface, consoleOrigin: 'https://console.example.test' } : {}) }));
  await page.route(path('/v1/auth/session'), json({ data: signedIn ? session : null }));
  await page.route(path('/api/auth/get-session'), json(signedIn ? session : null));
  await page.route(path('/api/auth/list-sessions'), json([{ id: 'session-ui', current: true, createdAt: new Date().toISOString(), expiresAt: future }]));
  await page.route(path('/api/auth/passkey/list-user-passkeys'), json([]));
  const tenants = [{ id: '101', name: 'Acme Labs', status: 'active', role: role === 'operator' ? null : role }, { id: '102', name: 'Other workspace', status: 'active', role: 'member' }];
  await page.route(path('/api/console/tenants'), json(tenants));
  await page.route(path('/api/tenants'), json(tenants));
  await page.route(path('/api/invitations'), json([]));
  await page.route(path('/api/invitation/9'), json({ organizationName: 'Acme Labs', email: 'ada@example.test', role: 'member', status: 'pending', customerID: '101' }));
  await page.route(path('/api/console/plugins'), json([]));
  await page.route(path('/api/console/plugins/installed'), json([]));
  await page.route((url) => url.pathname.startsWith('/api/') && /\/self-service$/.test(url.pathname), json([]));
  const meters = [{ id: 'projects', label: 'Projects', unit: 'count' }, { id: 'memory', label: 'Memory', unit: 'MiB' }, { id: 'cpu', label: 'CPU', unit: 'millicores' }, { id: 'storage', label: 'Storage', unit: 'MiB' }];
  const plan = { id: '301', name: 'Starter', description: 'Resources for this workspace.', limits: { projects: 5, memory: 2048, cpu: 1000, storage: 10240 }, isDefault: true, customers: 2 };
  await page.route(path('/api/console/plans'), json({ plans: [plan], meters }));
  await page.route(path('/api/console/clusters'), json([{ id: '401', name: 'EU cluster', country: 'DE', region: 'eu-central', verified: true, offered: true, dedicatedTo: null, projects: 2 }]));
  await page.route((url) => url.pathname.startsWith('/api/') && /\/usage$/.test(url.pathname), json({ plan: { ...plan, assigned: true }, overrides: {}, limits: plan.limits, allocated: { projects: 1, memory: 256, cpu: 50, storage: 1024 }, meters, units: [{ plugin: 'hosting', unit: 'web-1', project: { id: '201', name: 'Shop' }, resource: { id: 'app-1', name: 'Public website', offering: 'app' }, allocation: { memory: 256, cpu: 50, storage: 0 }, usage: { memory: 40, cpu: 3 }, observedAt: new Date().toISOString() }], history: [0, 1, 2].map((i) => ({ hour: new Date(Date.now() - i * 3600000).toISOString(), memory: 40 + i, cpu: 3 + i })).reverse() }));
  await page.route((url) => url.pathname.startsWith('/api/') && /\/domains$/.test(url.pathname), json([]));
  await page.route((url) => url.pathname.startsWith('/api/') && /\/api-tokens$/.test(url.pathname), json([]));
  await page.route((url) => url.pathname.startsWith('/api/') && /\/placement$/.test(url.pathname), json({ cluster: null, options: [], movable: true }));
  await page.route((url) => url.pathname.startsWith('/api/') && /^\/api\/console\/tenant\/\d+\/projects$/.test(url.pathname), json([{ id: '201', name: 'Shop', status: 'active' }]));
  await page.route(path('/api/console/project/201'), json({ id: '201', name: 'Shop', role, customer: { id: '101', name: 'Acme Labs' } }));
  await page.route(path('/api/console/projects/201/plugins'), json([]));
  await page.route((url) => url.pathname.startsWith('/api/') && /\/notifications$/.test(url.pathname), json([]));
  await page.route(path('/api/console/tenant/101'), async (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: { canManage: role !== 'member', operator: role === 'operator', tenant: { member_role: role }, members: state.members, invitations: [], activity: [] } });
    const command = route.request().postDataJSON();
    state.calls.push(command);
    if (state.failChange) return route.fulfill({ status: 400, json: { error: { message: 'Invitation unavailable. Try again.' } } });
    if (command.action === 'remove') state.members = state.members.filter((m) => m.id !== command.member);
    return route.fulfill({ json: { message: 'Change saved.', ...(command.action === 'invite' ? { link: previewOrigin + '/invitation?id=9', delivery: 'queued' } : {}) } });
  });
  if (role !== 'operator') await page.route((url) => url.pathname.startsWith('/v1/operator/'), (route) => {
    state.operatorCalls.push(route.request().url());
    return route.fulfill({ status: 403, json: { error: { message: 'Operator only.' } } });
  });
  return { page, context, state };
}
async function check(name, run) {
  if (process.env.WEBDOCK_UX_CASE && !name.toLowerCase().includes(process.env.WEBDOCK_UX_CASE.toLowerCase())) return;
  try { await run(); console.log(`PASS ${name}`); }
  catch (error) { failures.push(name); console.error(`FAIL ${name}: ${error.message}`); }
}
async function using(role, surface, run, signedIn = true) {
  const fixture = await open(role, surface, signedIn);
  try { await run(fixture); } finally { await fixture.context.close(); }
}
try {
  await check('Account opens the selected workspace for every member role', async () => {
    for (const role of ['owner', 'admin', 'member']) await using(role, 'account', async ({ page }) => {
      await page.goto(previewOrigin + '/workspaces');
      const workspace = page.getByRole('list', { name: 'Your workspaces', exact: true }).getByRole('listitem').filter({ hasText: 'Acme Labs' });
      await workspace.getByRole('link', { name: 'Open in Console' }).waitFor();
      assert.equal(await workspace.getByRole('link').getAttribute('href'), 'https://console.example.test/api/sso/login?returnTo=%2Fworkspaces%2F101');
    });
  });
  await check('Customer palette filters and switches authorized workspaces', () => using('member', 'console', async ({ page, state }) => {
    await page.goto(previewOrigin + '/workspaces/101');
    await page.getByRole('heading', { name: 'Acme Labs', exact: true }).waitFor();
    await page.keyboard.press('Control+k');
    const dialog = page.getByRole('dialog', { name: 'Go to workspace' });
    await dialog.waitFor();
    assert.equal(await dialog.getByRole('textbox').evaluate((el) => el === document.activeElement), true, 'workspace search receives keyboard focus when opened');
    await page.keyboard.type('Other');
    await dialog.getByRole('button', { name: /Other workspace/ }).click();
    await page.waitForURL(/\/workspaces\/102$/);
    assert.deepEqual(state.operatorCalls, []);
  }));
  await check('Failed invitation keeps all entered fields', () => using('owner', 'console', async ({ page, state }) => {
    state.failChange = true;
    await page.goto(previewOrigin + '/tenants/101');
    const form = page.getByRole('form', { name: 'Invite a person' });
    await form.getByRole('textbox', { name: 'Email address' }).fill('new@example.test');
    await form.getByRole('textbox', { name: 'Full name' }).fill('New colleague');
    await form.getByRole('combobox', { name: 'Role', exact: true }).selectOption('member');
    await form.getByRole('button', { name: 'Send invitation' }).click();
    await page.getByRole('alert').waitFor();
    assert.equal(await form.getByRole('textbox', { name: 'Email address' }).inputValue(), 'new@example.test');
    assert.equal(await form.getByRole('textbox', { name: 'Full name' }).inputValue(), 'New colleague');
    assert.equal(await form.getByRole('combobox').inputValue(), 'member');
    state.failChange = false;
    await form.getByRole('button', { name: 'Send invitation' }).click();
    await page.getByRole('textbox', { name: 'Invitation link' }).waitFor();
    assert.equal(await form.getByRole('textbox', { name: 'Email address' }).inputValue(), '', 'a successful invitation clears the form');
  }));
  await check('Removing a member requires confirmation and supports cancel', () => using('owner', 'console', async ({ page, state }) => {
    await page.goto(previewOrigin + '/tenants/101');
    const row = page.getByRole('region', { name: 'Members', exact: true }).getByRole('listitem').filter({ hasText: 'Grace Admin' });
    await row.getByRole('button', { name: 'Remove', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Remove Grace Admin?' });
    await dialog.waitFor();
    assert.deepEqual(state.calls, []);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    assert.deepEqual(state.calls, []);
    await row.getByRole('button', { name: 'Remove', exact: true }).click();
    await dialog.getByRole('button', { name: 'Remove member', exact: true }).click();
    await row.waitFor({ state: 'detached' });
    assert.deepEqual(state.calls, [{ action: 'remove', member: '602' }]);
  }));
  await check('Denied clipboard access offers manual copy without an unhandled error', () => using('owner', 'console', async ({ page }) => {
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await page.addInitScript(() => { Object.defineProperty(navigator, 'clipboard', { value: { writeText: () => Promise.reject(new Error('Clipboard refused')) } }); });
    await page.goto(previewOrigin + '/tenants/101');
    const form = page.getByRole('form', { name: 'Invite a person' });
    await form.getByRole('textbox', { name: 'Email address' }).fill('new@example.test');
    await form.getByRole('textbox', { name: 'Full name' }).fill('New colleague');
    await form.getByRole('button', { name: 'Send invitation' }).click();
    await page.getByRole('button', { name: 'Copy link' }).click();
    await page.getByText('Automatic copying is unavailable. Select the link and copy it manually.').waitFor();
    assert.equal(await page.getByRole('textbox', { name: 'Invitation link' }).evaluate((el) => el === document.activeElement && el.selectionEnd === el.value.length), true);
    assert.deepEqual(errors, []);
  }));
  await check('A verification challenge can return to sign-in', () => using('member', 'account', async ({ page }) => {
    await page.route(path('/api/auth/sign-in/email'), (route) => route.fulfill({ json: { twoFactorRedirect: true } }));
    await page.goto(previewOrigin + '/login');
    await page.getByRole('textbox', { name: 'Email', exact: true }).fill('ada@example.test');
    await page.getByLabel('Password', { exact: true }).fill('disposable-ui-password');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.getByRole('heading', { name: 'Verify your identity' }).waitFor();
    await page.getByRole('button', { name: 'Back to sign in', exact: true }).click();
    await page.getByRole('heading', { name: 'Sign in to Spitzli Cloud' }).waitFor();
  }, false));
  await check('Clicking modal padding preserves it and short screens fit the dialog', () => using('operator', 'console', async ({ page }) => {
    await page.goto(previewOrigin + '/projects');
    await page.getByRole('heading', { name: 'Projects', exact: true }).waitFor();
    await page.keyboard.press('Control+k');
    const dialog = page.getByRole('dialog', { name: 'Go to workspace' });
    await dialog.waitFor();
    const box = await dialog.boundingBox();
    assert.ok(box);
    await page.mouse.click(box.x + 5, box.y + 5);
    assert.equal(await dialog.isVisible(), true);
    for (const height of [320, 568, 812]) {
      await page.setViewportSize({ width: 375, height });
      const bounds = await dialog.boundingBox();
      assert.ok(bounds && bounds.y >= 0 && bounds.y + bounds.height <= height, `dialog fits ${height}px height`);
    }
    await page.keyboard.press('Escape');
    await dialog.waitFor({ state: 'detached' });
    await page.getByRole('button', { name: 'Open navigation' }).click();
    const navigation = page.getByRole('dialog', { name: 'Navigation', exact: true });
    await navigation.waitFor();
    const bounds = await navigation.boundingBox();
    assert.ok(bounds && bounds.y >= 0 && bounds.y + bounds.height <= 812);
  }));
  await check('Mobile inputs stay readable at 320/375/414px', () => using('owner', 'console', async ({ page }) => {
    await page.goto(previewOrigin + '/tenants/101');
    const email = page.getByRole('textbox', { name: 'Email address', exact: true });
    await email.waitFor();
    for (const width of [320, 375, 414]) {
      await page.setViewportSize({ width, height: 812 });
      assert.ok(await email.evaluate((el) => Number.parseFloat(getComputedStyle(el).fontSize)) >= 16, 'mobile input text must not trigger Safari focus zoom');
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    }
  }));
  await check('Members and administrators only see permitted member controls', async () => {
    for (const role of ['member', 'admin', 'owner', 'operator']) await using(role, 'console', async ({ page, state }) => {
      await page.goto(previewOrigin + '/tenants/101');
      await page.getByRole('region', { name: 'Members', exact: true }).waitFor();
      assert.equal(await page.getByRole('form', { name: 'Invite a person' }).count(), role === 'member' ? 0 : 1);
      assert.equal(await page.getByRole('combobox', { name: 'Role of Grace Admin' }).count(), ['owner', 'operator'].includes(role) ? 1 : 0);
      assert.equal(await page.getByRole('button', { name: 'Remove', exact: true }).count(), ['owner', 'operator'].includes(role) ? 2 : 0);
      assert.deepEqual(state.calls, []);
    });
  });
  for (const [url, button] of [['/plans', 'New plan'], ['/workspaces/101/api-tokens', 'Create token']]) {
    await check(`Restricted deep link ${url} shows no editing controls`, () => using('member', 'console', async ({ page }) => {
      await page.goto(previewOrigin + url);
      await page.getByRole('main').waitFor();
      await page.getByRole('heading', { name: /access required/i }).waitFor();
      assert.equal(await page.getByRole('button', { name: button, exact: true }).count(), 0);
    }));
  }
  await check('Pending sign-in prevents conflicting mode and passkey actions', () => using('member', 'account', async ({ page }) => {
    let release;
    const pending = new Promise((resolve) => { release = resolve; });
    await page.route(path('/api/auth/sign-in/email'), async (route) => { await pending; await route.fulfill({ json: { twoFactorRedirect: true } }); });
    await page.goto(previewOrigin + '/login');
    await page.getByRole('textbox', { name: 'Email', exact: true }).fill('ada@example.test');
    await page.getByLabel('Password', { exact: true }).fill('disposable-ui-password');
    const requested = page.waitForRequest((r) => new URL(r.url()).pathname === '/api/auth/sign-in/email');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await requested;
    await page.getByRole('button', { name: 'Checking…', exact: true }).waitFor();
    try {
      assert.equal(await page.getByRole('button', { name: 'Forgot password?' }).isDisabled(), true);
      assert.equal(await page.getByRole('button', { name: 'Sign in with a passkey' }).isDisabled(), true);
      assert.equal(await page.getByRole('textbox', { name: 'Email', exact: true }).isDisabled(), true);
    } finally { release(); }
    await page.getByRole('heading', { name: 'Verify your identity' }).waitFor();
  }, false));
  await check('Customer create menus stay inside the mobile viewport', () => using('owner', 'console', async ({ page }) => {
    const manifest = JSON.parse(await readFile(fromRepo('plugins/hosting/manifest.json'), 'utf8'));
    await page.route(path('/api/console/plugins'), json => json.fulfill({ json: [{ state: 'enabled', runtime: 'ready', manifest, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: ['create-apps'], resources: [] }] }));
    await page.setViewportSize({ width: 320, height: 812 });
    for (const [url, button, label] of [
      ['/workspaces/101/projects/201', 'Add', 'Add a resource'],
      ['/workspaces/101/products/hosting', 'Create', 'Create'],
    ]) {
      await page.goto(previewOrigin + url);
      await page.locator('summary').filter({ hasText: new RegExp('^' + button + '$') }).click();
      const menu = page.getByRole('list', { name: label, exact: true });
      await menu.waitFor();
      const box = await menu.boundingBox();
      assert.ok(box && box.x >= 0 && box.x + box.width <= 320, url + ' create menu must fit the viewport');
      await page.keyboard.press('Tab');
      assert.equal(await menu.getByRole('button').first().evaluate(el => el === document.activeElement), true);
    }
  }));
  await check('Read-only resource preview keeps reads and withholds writes for native and generic plugins', () => using('operator', 'console', async ({ page }) => {
    const previewSession = { user: { id: '2', name: 'Preview operator', email: 'operator@example.test', role: 'operator', emailVerified: true, twoFactorEnabled: true, preview: true, acting: false, customerScope: '101' }, session: { id: 'read-only-fixture', userId: '2', expiresAt: future } };
    await page.route(path('/v1/auth/session'), route => route.fulfill({ json: { data: previewSession } }));
    await page.route(path('/api/auth/get-session'), route => route.fulfill({ json: previewSession }));
    await page.route(path('/api/console/tenant-preview/current'), route => route.fulfill({ json: { customerID: '101', customerName: 'Acme Labs', status: 'active', mode: 'read_only', readOnly: true, expiresAt: future } }));
    // Membership can still say owner; the active preview must independently remain read-only.
    await page.route(path('/api/console/project/201'), route => route.fulfill({ json: { id: '201', name: 'Shop', role: 'owner', customer: { id: '101', name: 'Acme Labs' } } }));
    const hosting = JSON.parse(await readFile(fromRepo('plugins/hosting/manifest.json'), 'utf8'));
    const generic = { name: 'generic-fixture', offerings: [{ id: 'resource', label: 'Resource' }], selfService: { features: [] }, actions: [{ name: 'safe.read', intent: 'read', audience: 'customer', resource: 'use' }, { name: 'unsafe.write', intent: 'write', audience: 'customer', resource: 'use' }], areas: { operator: { pages: [] }, selfService: { pages: [{ id: 'resource', label: 'Resource', action: 'safe.read', resource: true }] } } };
    const view = (manifest, resource) => ({ state: 'enabled', runtime: 'ready', manifest, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: [], resources: [resource] });
    await page.route(path('/api/console/plugins'), route => route.fulfill({ json: [view(hosting, { id: 'app-preview', offering: 'app', name: 'Website', createdVia: 'self-service' }), view(generic, { id: 'generic-preview', offering: 'resource', name: 'Generic resource', createdVia: 'self-service' })] }));
    await page.route(url => url.pathname.endsWith('/logs'), route => route.fulfill({ json: [] }));
    const actions = [];
    await page.route(path('/api/console/plugins/invoke'), route => {
      const action = route.request().postDataJSON().action; actions.push(action);
      if (action === 'apps.view') return route.fulfill({ json: { name: 'Website', status: 'running', image: 'nginx:stable-alpine', port: 80, memoryMb: 128, url: null, env: [], error: null, createdAt: '2026-10-22 10:00:00+00', host: 'website.example.test', domains: [] } });
      if (action === 'safe.read') return route.fulfill({ json: { version: 2, title: 'Generic resource', blocks: [{ kind: 'actions', actions: [{ id: 'safe.read', label: 'Refresh' }, { id: 'unsafe.write', label: 'Change settings' }] }] } });
      return route.fulfill({ status: 403, json: { error: { message: 'Read-only fixture' } } });
    });
    await page.goto(previewOrigin + '/workspaces/101/projects/201/resources/hosting/app-preview');
    await page.getByRole('heading', { name: 'Website', exact: true }).waitFor();
    await page.getByText('Running', { exact: true }).first().waitFor();
    const stop = page.getByRole('button', { name: 'Stop', exact: true });
    assert.ok(await stop.count() === 0 || await stop.isDisabled(), 'native writes must be unavailable in read-only preview');
    await page.goto(previewOrigin + '/workspaces/101/projects/201/resources/generic-fixture/generic-preview');
    await page.getByRole('button', { name: 'Change settings', exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: 'Change settings', exact: true }).isDisabled(), true);
    await page.getByRole('button', { name: 'Refresh', exact: true }).click();
    assert.ok(!actions.includes('unsafe.write') && !actions.includes('apps.power'));
  }));
  await check('Desktop/mobile and light/dark role matrix stays usable', async () => {
    const shots = process.env.WEBDOCK_UX_SCREENSHOTS === 'true';
    if (shots) await mkdir(fromRepo('.runtime-test/ui-ux/screens/'), { recursive: true });
    const matrix = [
      ['operator', 'console', ['/projects', '/customers', '/customers/new', '/customers/101', '/tenants/101', '/plans', '/clusters', '/extensions', '/audit-events']],
      ['owner', 'console', ['/workspaces/101', '/workspaces/101/projects/201', '/tenants/101', '/workspaces/101/usage', '/workspaces/101/domains', '/workspaces/101/api-tokens']],
      ['admin', 'console', ['/tenants/101']],
      ['member', 'console', ['/workspaces/101', '/tenants/101']],
      ['member', 'account', ['/account', '/workspaces', '/invitation?id=9']],
      ['member', 'account', ['/login', '/reset-password?token=' + 'a'.repeat(64)], false],
    ];
    for (const [role, surface, routes, signedIn = true] of matrix) await using(role, surface, async ({ page }) => {
      await page.emulateMedia({ reducedMotion: 'reduce' });
      for (const url of routes) {
        const shotName = new URL(url, previewOrigin).pathname.replaceAll('/', '_');
        await page.goto(previewOrigin + url);
        await page.getByRole('heading', { level: 1 }).first().waitFor().catch((error) => { throw new Error(`${role} ${surface} ${url}: ${error.message}`); });
        await page.waitForFunction(() => ![...document.querySelectorAll('[role="status"]')].some((el) => /^Loading/.test(el.textContent)));
        assert.equal(await page.getByRole('alert').count(), 0, `${role} ${url} must not contain an unexpected error`);
        for (const width of [320, 375, 414, 768, 1280]) {
          await page.setViewportSize({ width, height: 812 });
          assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${role} ${url} overflows at ${width}px`);
          if (shots && [375, 1280].includes(width)) await page.screenshot({ path: fromRepo(`.runtime-test/ui-ux/screens/${surface}-${role}-${shotName}-${width}.png`).pathname, fullPage: true });
        }
        await page.evaluate(() => { document.documentElement.dataset.theme = 'dark'; });
        assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).colorScheme), 'dark');
        for (const width of [320, 375, 414, 768, 1280]) {
          await page.setViewportSize({ width, height: 812 });
          assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `${role} ${url} dark overflows at ${width}px`);
          if (shots && [375, 1280].includes(width)) await page.screenshot({ path: fromRepo(`.runtime-test/ui-ux/screens/${surface}-${role}-${shotName}-dark-${width}.png`).pathname, fullPage: true });
        }
        await page.evaluate(() => { document.documentElement.dataset.theme = 'light'; });
      }
    }, signedIn);
  });
  await check('Operator breadcrumbs match Plans and Clusters', () => using('operator', 'console', async ({ page }) => {
    for (const [url, label] of [['/plans', 'Plans'], ['/clusters', 'Clusters']]) {
      await page.goto(previewOrigin + url);
      await page.getByRole('heading', { name: label, exact: true }).waitFor();
      assert.ok((await page.getByRole('navigation', { name: 'Breadcrumb', exact: true }).innerText()).endsWith(label));
    }
  }));
  await check('Account menu supports keyboard focus, arrows, Home, End and Escape', () => using('member', 'console', async ({ page }) => {
    await page.goto(previewOrigin + '/tenants/101');
    const trigger = page.getByRole('button', { name: /^Account menu for/ });
    await trigger.click();
    const menu = page.getByRole('menu', { name: 'Account', exact: true });
    await menu.waitFor();
    const items = menu.getByRole('menuitem');
    await page.waitForFunction(() => document.querySelector('[popover]:popover-open [role="menuitem"]') === document.activeElement);
    assert.equal(await items.first().evaluate((el) => el === document.activeElement), true, 'opening a menu focuses its first action');
    await page.keyboard.press('End');
    assert.equal(await items.last().evaluate((el) => el === document.activeElement), true);
    await page.keyboard.press('ArrowDown');
    assert.equal(await items.first().evaluate((el) => el === document.activeElement), true);
    await page.keyboard.press('ArrowUp');
    assert.equal(await items.last().evaluate((el) => el === document.activeElement), true);
    await page.keyboard.press('Home');
    assert.equal(await items.first().evaluate((el) => el === document.activeElement), true);
    await page.keyboard.press('Escape');
    await menu.waitFor({ state: 'hidden' });
    assert.equal(await trigger.evaluate((el) => el === document.activeElement), true);
  }));
  await check('Long member names keep removal dialogs inside the viewport', () => using('owner', 'console', async ({ page, state }) => {
    const name = 'LongMemberName'.repeat(10);
    state.members[1].name = name;
    await page.goto(previewOrigin + '/tenants/101');
    await page.setViewportSize({ width: 320, height: 568 });
    await page.getByRole('region', { name: 'Members', exact: true }).getByRole('listitem').filter({ hasText: name }).getByRole('button', { name: 'Remove', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: `Remove ${name}?` });
    await dialog.waitFor();
    assert.equal(await dialog.evaluate((el) => el.scrollWidth > el.clientWidth), false, 'long dialog headings must wrap instead of pushing controls out of view');
    const box = await dialog.boundingBox();
    assert.ok(box && box.y >= 0 && box.y + box.height <= 568);
  }));
  await check('A refused member removal keeps the member and a recoverable dialog', () => using('owner', 'console', async ({ page, state }) => {
    state.failChange = true;
    await page.goto(previewOrigin + '/tenants/101');
    const row = page.getByRole('region', { name: 'Members', exact: true }).getByRole('listitem').filter({ hasText: 'Grace Admin' });
    await row.getByRole('button', { name: 'Remove', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Remove Grace Admin?' });
    await dialog.getByRole('button', { name: 'Remove member', exact: true }).click();
    await dialog.getByRole('alert').waitFor();
    assert.equal(await row.count(), 1);
    assert.equal(await dialog.getByRole('button', { name: 'Remove member', exact: true }).isEnabled(), true);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    assert.equal(await row.count(), 1);
  }));
  await check('Review: removal confirmation resets when history changes the customer', () => using('operator', 'console', async ({ page, state }) => {
    const errors = [];
    page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
    const records = [
      { id: '101', name: 'First customer', status: 'active', customerType: 'company' },
      { id: '102', name: 'Second customer', status: 'active', customerType: 'company' },
    ];
    await page.route((url) => url.pathname.startsWith('/v1/operator/registry/customers'), (route) => {
      const id = new URL(route.request().url()).pathname.split('/')[5];
      return route.fulfill({ json: id ? records.find((record) => record.id === id) : { docs: records, page: 1, totalPages: 1, totalDocs: 2, hasNextPage: false, hasPrevPage: false } });
    });
    await page.route(path('/v1/operator/registry/projects'), (route) => route.fulfill({ json: { docs: [], page: 1, totalPages: 1, totalDocs: 0, hasNextPage: false, hasPrevPage: false } }));
    await page.route(path('/api/console/tenant/102'), (route) => route.fulfill({ json: { canManage: true, operator: true, members: [{ id: '603', name: 'Second Owner', email: 'second@example.test', role: 'owner' }], invitations: [] } }));
    const switchTo = async (name, id) => {
      await page.keyboard.press('Control+k');
      const palette = page.getByRole('dialog', { name: 'Go to workspace' });
      await palette.getByRole('textbox').fill(name);
      await palette.getByRole('button', { name: `${name} Customer`, exact: true }).click();
      await page.waitForURL(`**/customers/${id}`);
      await page.getByRole('heading', { name, exact: true }).waitFor();
    };
    await page.goto(previewOrigin + '/customers/102');
    await page.getByRole('region', { name: 'Members', exact: true }).waitFor();
    await switchTo('First customer', '101');
    await page.getByRole('region', { name: 'Members', exact: true }).waitFor();
    await switchTo('Second customer', '102');
    await page.getByRole('region', { name: 'Members', exact: true }).getByRole('button', { name: 'Remove', exact: true }).click();
    const confirmation = page.getByRole('dialog', { name: 'Remove Second Owner?' });
    await confirmation.waitFor();
    await page.goBack();
    await page.getByRole('heading', { name: 'First customer', exact: true }).waitFor();
    await confirmation.waitFor({ state: 'detached' });
    assert.deepEqual(state.calls, [], 'history navigation must not submit a stale membership');
    assert.deepEqual(errors, [], 'customer switching must retain unique React identities');
  }));
  await check('Review: a new removal confirmation clears the previous member error', () => using('owner', 'console', async ({ page, state }) => {
    state.failChange = true;
    await page.goto(previewOrigin + '/tenants/101');
    const members = page.getByRole('region', { name: 'Members', exact: true });
    await members.getByRole('listitem').filter({ hasText: 'Ada Owner' }).getByRole('button', { name: 'Remove', exact: true }).click();
    const rejected = page.getByRole('dialog', { name: 'Remove Ada Owner?' });
    await rejected.getByRole('button', { name: 'Remove member', exact: true }).click();
    await rejected.getByRole('alert').waitFor();
    await rejected.getByRole('button', { name: 'Cancel', exact: true }).click();
    await members.getByRole('listitem').filter({ hasText: 'Grace Admin' }).getByRole('button', { name: 'Remove', exact: true }).click();
    const next = page.getByRole('dialog', { name: 'Remove Grace Admin?' });
    await next.waitFor();
    assert.equal(await next.getByRole('alert').count(), 0, 'a previous member rejection does not apply to this confirmation');
    assert.deepEqual(state.calls, [{ action: 'remove', member: '601' }]);
  }));
  assert.deepEqual(failures, [], 'UX regressions must all pass');
} finally { await browser.close(); await server.close(); }
