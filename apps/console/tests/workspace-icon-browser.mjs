// Local browser fixtures; authorization and PNG validation also have backend tests.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const browser = await chromium.launch({ headless: true, chromiumSandbox: true });
try {
  for (const role of ['owner', 'admin', 'member']) {
    const context = await browser.newContext();
    const page = await context.newPage();
    let icon = null;
    const writes = [];
    const json = (body) => (r) => r.fulfill({ json: body });
    const path = (p) => (url) => url.pathname === p;
    await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
    await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
    await page.route(path('/api/console/tenants'), r => r.fulfill({ json: [{ id: '101', name: 'Acme', status: 'active', role, icon }] }));
    await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Default', status: 'active' }]));
    await page.route(path('/api/console/plugins'), json([]));
    await page.route(path('/api/console/tenant/101'), r => {
      if (r.request().method() === 'POST') {
        assert.notEqual(role, 'member');
        const cmd = r.request().postDataJSON();
        assert.equal(cmd.action, 'icon');
        icon = cmd.icon;
        writes.push(cmd);
        return r.fulfill({ json: { message: 'Change saved.' } });
      }
      return r.fulfill({ json: { tenant: { name: 'Acme', icon, member_role: role }, canManage: role !== 'member', members: [], invitations: [], activity: [] } });
    });
    await page.goto(previewOrigin + '/tenants/101');
    const switcher = page.getByRole('button', { name: 'Switch workspace', exact: true });
    await switcher.waitFor();
    const input = page.getByLabel('Upload workspace icon', { exact: true });
    if (role === 'member') {
      assert.equal(await input.count(), 0);
      assert.equal(writes.length, 0);
    } else {
      await input.waitFor();
      const png = await page.evaluate(() => { const c = document.createElement('canvas'); c.width = c.height = 16; c.getContext('2d').fillRect(0, 0, 16, 16); return c.toDataURL('image/png').split(',')[1]; });
      await input.setInputFiles({ name: 'icon.png', mimeType: 'image/png', buffer: Buffer.from(png, 'base64') });
      await switcher.locator('img').waitFor();
      assert.match(icon, /^data:image\/png;base64,/);
      assert.ok(icon.length < 32768);
      await input.setInputFiles({ name: 'replacement.png', mimeType: 'image/png', buffer: Buffer.from(png, 'base64') });
      await page.getByText('Workspace icon saved.', { exact: true }).waitFor();
      await page.reload();
      await switcher.locator('img').waitFor();
      await page.goto(previewOrigin + '/workspaces/101');
      await page.getByRole('heading', { name: 'Acme', exact: true }).waitFor();
      await switcher.locator('img').waitFor();
      await page.goto(previewOrigin + '/tenants/101');
      await switcher.locator('img').waitFor();
      await page.getByRole('button', { name: 'Remove workspace icon', exact: true }).click();
      await switcher.getByText('A', { exact: true }).waitFor();
      assert.equal(icon, null);
      await input.setInputFiles({ name: 'bad.svg', mimeType: 'image/svg+xml', buffer: Buffer.from('<svg/>') });
      await page.getByRole('alert').filter({ hasText: 'Choose a PNG, JPEG or WebP image up to 2 MiB.' }).waitFor();
      assert.equal(writes.length, 3);
      icon = 'data:image/png;base64,invalid';
      await page.reload();
      await switcher.getByText('A', { exact: true }).waitFor();
    }
    for (const width of [375, 1280]) {
      await page.setViewportSize({ width, height: 900 });
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'workspace settings fit the viewport');
    }
    await context.close();
  }
  console.log('Workspace icon: owner/admin upload/replace, reload/navigation, remove, invalid file/image, responsive settings; member has no write controls');
} finally { await browser.close(); await server.close(); }
