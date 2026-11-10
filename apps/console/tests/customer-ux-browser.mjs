// Real customer routes against a loopback HTTP mock, without page.route API replacements.
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { fromRepo } from './repo.mjs';
process.env.SPITZLI_CUSTOMER_FIXTURE = '1';
const { server, previewOrigin } = await import('./preview-server.ts');
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, locale: 'en-GB' });
const page = await context.newPage();
page.setDefaultTimeout(5000);
const errors = [], externalRequests = [], checks = [], failures = [];
page.on('pageerror', error => errors.push(error.message));
await context.route('**/*', route => {
  const url = new URL(route.request().url());
  if (url.origin === previewOrigin || ['data:', 'blob:'].includes(url.protocol)) return route.continue();
  externalRequests.push(url.origin);
  return route.abort();
});
const output = fromRepo('.runtime-test/customer-ux/');
await mkdir(output, { recursive: true });
async function check(name, run) {
  if (process.env.SPITZLI_CUSTOMER_UX_CASE && !name.includes(process.env.SPITZLI_CUSTOMER_UX_CASE)) return;
  try { await run(); checks.push({ name, passed: true }); console.log(`PASS ${name}`); }
  catch (error) { await page.screenshot({ path: new URL('failure-' + checks.length + '.png', output).pathname, fullPage: true }); await writeFile(new URL('failure-' + checks.length + '.txt', output), await page.locator('body').innerText()); checks.push({ name, passed: false, error: error.message }); failures.push(name); console.error(`FAIL ${name}: ${error.message}`); }
}
async function capture(path, width, theme) {
  if (![320, 1280].includes(width) || !['/workspaces/101', projectPath, dbPath, '/workspaces/101/databases'].includes(path)) return;
  await page.screenshot({ path: new URL(`${theme}-${width}-${path.split('/').slice(2).join('-')}.png`, output).pathname, fullPage: true });
}
async function goto(path, heading) {
  await page.goto(previewOrigin + path);
  await page.getByRole('heading', { name: heading, exact: true }).waitFor().catch(error => { throw new Error(path + ': ' + error.message); });
}
const projectPath = '/workspaces/101/projects/202';
const dbPath = '/workspaces/101/products/postgres';
try {
  await check('customer page matrix', async () => {
    const routes = [
      ['/tenants', 'Your workspaces'], ['/workspaces/101', 'Nordlicht GmbH'],
      ['/workspaces/101/projects/201', 'Onlineshop'], [projectPath, 'Kundenportal'],
      ['/workspaces/101/projects/203', 'Website und Veranstaltungen für Geschäftskunden'],
      ['/workspaces/101/databases', 'Databases'], [dbPath, 'Databases'],
      ['/workspaces/101/products/mail', 'Mail'],
      ['/workspaces/101/projects/201/resources/postgres/00000000-0000-4000-8000-000000000001', 'production'],
      ['/workspaces/101/projects/201/resources/mail/00000000-0000-4000-8000-0000000000d1', 'shop.example.test'],
      ['/workspaces/101/domains', 'Domains'], ['/workspaces/101/connections', 'Connections'],
      ['/workspaces/101/usage', 'Plan and usage'], ['/workspaces/101/api-tokens', 'API tokens'],
      ['/tenants/101', 'Nordlicht GmbH'], ['/account', 'Account'],
      ['/workspaces/102', 'Studio Linden'], ['/workspaces/102/projects/301', 'Kampagne'],
    ];
    for (const theme of ['light', 'dark']) {
      await page.goto(previewOrigin);
      await page.evaluate(value => localStorage.setItem('webdock.theme.v1', value), theme);
      for (const width of [320, 375, 414, 768, 1280]) {
        await page.setViewportSize({ width, height: 900 });
        for (const [path, heading] of routes) {
          console.log(`PAGE ${theme} ${width} ${path}`);
          await goto(path, heading);
          await page.waitForFunction(() => [...document.querySelectorAll('main [role="status"]')].every(element => !/^(Loading|Opening|Checking)/.test(element.textContent.trim())));
          const alerts = await page.locator('main [role="alert"]').allTextContents();
          assert.deepEqual(alerts, [], `${path}, ${width}px: no unexpected failure panel`);
          assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${path}, ${width}px: no horizontal overflow`);
          const actualTheme = await page.locator('html').getAttribute('data-theme');
          assert.equal(actualTheme, theme, `${path}: ${theme} theme`);
          await capture(path, width, theme);
        }
      }
    }
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await check('project search and workspace switching', async () => {
    await goto('/workspaces/101', 'Nordlicht GmbH');
    await page.getByRole('button', { name: 'Find a workspace or project', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Go to workspace' });
    const search = dialog.getByRole('textbox', { name: 'Search workspaces and projects' });
    await search.fill('kundenportal');
    await dialog.getByRole('button', { name: /Kundenportal/ }).click();
    await page.waitForURL(new RegExp(projectPath + '$'));
    await page.keyboard.press('Control+k');
    await search.fill('Studio');
    await dialog.getByRole('button', { name: 'Studio Linden', exact: true }).click();
    await page.waitForURL(/\/workspaces\/102$/);
    await page.keyboard.press('Control+k');
    await search.fill('Onlineshop');
    assert.equal(await dialog.getByRole('button', { name: /Onlineshop/ }).count(), 0, 'project search stays in current workspace');
    await page.keyboard.press('Escape');
    await page.keyboard.press('Control+k');
    await search.fill('Kampagne');
    await dialog.getByRole('button', { name: /Kampagne/ }).click();
    await page.waitForURL(/\/workspaces\/102\/projects\/301$/);
  });
  await check('project breadcrumb names actual project', async () => {
    await goto(projectPath, 'Kundenportal');
    await page.getByRole('navigation', { name: 'Breadcrumb' }).getByText('Kundenportal', { exact: true }).waitFor();
    await goto('/workspaces/101/projects/201/resources/postgres/00000000-0000-4000-8000-000000000001', 'production');
    await page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('link', { name: 'Onlineshop', exact: true }).click();
    await page.waitForURL(/\/workspaces\/101\/projects\/201$/);
  });
  await check('product filter and correct creation destination', async () => {
    await goto(dbPath, 'Databases');
    const filter = page.getByRole('combobox', { name: 'Filter by project', exact: true });
    await filter.selectOption('202');
    const rows = page.getByRole('row').filter({ has: page.getByRole('link', { name: 'production', exact: true }) });
    assert.equal(await rows.count(), 1);
    await rows.getByRole('link', { name: 'Kundenportal', exact: true }).waitFor();
    await page.getByText('Create', { exact: true }).click();
    await page.getByRole('button', { name: /^Shared database A database of your own/ }).click();
    assert.equal(await page.getByRole('combobox', { name: 'Project', exact: true }).inputValue(), '202');
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await filter.selectOption('203');
    await page.getByRole('heading', { name: 'No matching resources', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
    assert.equal(await rows.count(), 2);
    await page.getByRole('searchbox', { name: 'Search resources', exact: true }).fill('does-not-exist');
    await page.getByRole('heading', { name: 'No matching resources', exact: true }).waitFor();
  });
  await check('resource names fit mobile and project links remain clickable', async () => {
    for (const width of [320, 1280]) {
      await page.setViewportSize({ width, height: 900 });
      await goto(dbPath, 'Databases');
      const table = page.getByRole('table');
      const project = table.getByRole('link', { name: 'Kundenportal', exact: true });
      const resource = table.getByRole('link', { name: 'production', exact: true }).first();
      await resource.waitFor();
      assert.equal(await resource.evaluate(element => {
        const range = document.createRange(); range.selectNodeContents(element);
        return range.getClientRects().length;
      }), 1, 'resource names stay on one readable line');
      await project.click();
      await page.waitForURL(new RegExp(projectPath + '$'));
    }
  });
  await check('failed inventory stays local to one project', async () => {
    await page.request.post(previewOrigin + '/__customer-fixture', { data: { failedProjects: ['202'] } });
    try {
      await goto('/workspaces/101', 'Nordlicht GmbH');
      await page.getByRole('article', { name: 'Onlineshop', exact: true }).getByRole('list', { name: 'Resources in Onlineshop', exact: true }).waitFor();
      await page.getByRole('article', { name: 'Kundenportal', exact: true }).getByText('Resource details unavailable.', { exact: true }).waitFor();
      await page.getByRole('article', { name: 'Website und Veranstaltungen für Geschäftskunden', exact: true }).getByText('No extension resources linked.', { exact: true }).waitFor();
    } finally { await page.request.post(previewOrigin + '/__customer-fixture', { data: { failedProjects: [] } }); }
  });
  await check('customer roles and recovery', async () => {
    await page.request.post(previewOrigin + '/__customer-fixture', { data: { failCreate: true } });
    await goto('/workspaces/102', 'Studio Linden');
    assert.equal(await page.getByRole('button', { name: 'New project', exact: true }).count(), 0);
    assert.equal(await page.getByRole('link', { name: 'Members', exact: true }).count(), 0);
    await goto('/workspaces/102/api-tokens', 'Workspace administrator access required');
    await goto('/workspaces/101', 'Nordlicht GmbH');
    await page.getByRole('button', { name: 'New project', exact: true }).click();
    await page.getByRole('textbox', { name: 'Project name', exact: true }).fill('Cannot be created');
    await page.getByRole('button', { name: 'Create project', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: 'Fixture project quota reached.' }).waitFor();
    assert.equal(await page.getByRole('textbox', { name: 'Project name', exact: true }).inputValue(), 'Cannot be created');
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.request.post(previewOrigin + '/__customer-fixture', { data: { failCreate: false } });
    await page.getByRole('button', { name: 'New project', exact: true }).click();
    assert.deepEqual(await page.getByRole('alert').allTextContents(), [], 'reopening the form clears the previous error');
    await page.getByRole('textbox', { name: 'Project name', exact: true }).fill('Neue Website');
    await page.getByRole('button', { name: 'Create project', exact: true }).click();
    await page.getByRole('heading', { name: 'Neue Website', exact: true }).waitFor();
    assert.match(page.url(), /\/workspaces\/101\/projects\/9[0-9]+$/);
    await page.setViewportSize({ width: 320, height: 812 });
    await page.getByRole('button', { name: 'Open navigation', exact: true }).click();
    await page.getByRole('dialog', { name: 'Navigation', exact: true }).getByRole('link', { name: 'Domains', exact: true }).click();
    await page.getByRole('heading', { name: 'Domains', exact: true }).waitFor();
    assert.equal(await page.getByRole('dialog', { name: 'Navigation', exact: true }).count(), 0);
    const state = await (await page.request.get(previewOrigin + '/__customer-fixture')).json();
    assert.deepEqual(state.calls.filter(c => c.path.startsWith('/v1/operator/')), [], 'customer never requests operator data');
    assert.deepEqual(errors, []);
    assert.deepEqual(externalRequests, [], 'no provider, database or external network traffic');
  });
  await check('foreign project URL keeps the workspace boundary', async () => {
    await goto('/workspaces/101/projects/301', 'Project unavailable');
    assert.equal(await page.getByRole('heading', { name: 'Kampagne', exact: true }).count(), 0);
    await page.getByRole('link', { name: 'Open workspace', exact: true }).click();
    await page.waitForURL(/\/workspaces\/101$/);
  });
  await check('German project search and product filters', async () => {
    await page.setViewportSize({ width: 1280, height: 900 });
    await goto(dbPath, 'Databases');
    await page.getByRole('button', { name: 'Language: English', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'Deutsch', exact: true }).click();
    await page.getByRole('combobox', { name: 'Nach Projekt filtern', exact: true }).waitFor();
    assert.equal(await page.locator('html').getAttribute('lang'), 'de');
    await page.getByRole('combobox', { name: 'Nach Projekt filtern', exact: true }).selectOption('203');
    await page.getByRole('heading', { name: 'Keine passenden Ressourcen', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Filter zurücksetzen', exact: true }).click();
    await page.getByRole('searchbox', { name: 'Ressourcen suchen', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Workspace oder Projekt finden', exact: true }).click();
    const search = page.getByRole('textbox', { name: 'Workspaces und Projekte suchen', exact: true });
    await search.fill('kundenportal');
    await page.getByRole('dialog').getByRole('button', { name: 'Kundenportal', exact: true }).click();
    await page.waitForURL(new RegExp(projectPath + '$'));
    assert.deepEqual(errors, []);
    assert.deepEqual(externalRequests, []);
  });

} finally {
  await writeFile(new URL('results.json', output), JSON.stringify({ base: '5c9f198d9c9b3bc113714da8bd544203736bec46', fixture: 'loopback HTTP; synthetic data only', checks, pageErrors: errors, externalRequests }, null, 2));
  await context.close(); await browser.close(); await server.close();
}
assert.deepEqual(failures, [], 'customer UX checks');
console.log('Customer UX checks passed. Synthetic HTTP fixtures only; no live provider/auth/full-platform E2E claim.');
