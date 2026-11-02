import assert from 'node:assert/strict';
import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { z } from 'zod';
const source = process.env.WEBDOCK_HOSTING_BROWSER_FIXTURE;
assert.ok(source, 'An owned native hosting fixture file is required');
const fixture = z.object({
  origin: z.url(), cookie: z.object({ name: z.string().min(1), value: z.string().min(1) }),
  customerID: z.string(), customerName: z.string(), projectID: z.string(), projectName: z.string(), appID: z.string(), appName: z.string(),
  updateCPU: z.string().optional(), expectedLog: z.string().optional(),
  createName: z.string(), createImage: z.string(), createArgs: z.array(z.string()),
}).parse(JSON.parse(readFileSync(source, 'utf8')));
const origin = new URL(fixture.origin);
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname), 'Native acceptance requires an owned loopback fixture');
assert.equal(origin.origin, fixture.origin, 'Fixture origin must not contain a path or credentials');
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
page.setDefaultTimeout(60000);
const errors = [];
page.on('pageerror', error => errors.push(error.message));
async function accepted(action, click) {
  const response = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/console/hosting' && response.request().postDataJSON().action === action);
  await click();
  const result = await response;
  assert.equal(result.ok(), true, `${action} was not accepted by native Core (HTTP ${String(result.status())})`);
  const receipt = await result.json();
  assert.equal(typeof receipt.id, 'string');
  assert.ok(['queued', 'running'].includes(receipt.status), `${action} must produce a durable operation receipt`);
  return receipt;
}
async function assertContext() {
  await page.getByRole('navigation', { name: 'Primary navigation', exact: true }).getByRole('link', { name: fixture.projectName, exact: true }).waitFor();
  assert.ok((await page.getByRole('button', { name: /^Switch (?:customer or project|workspace)$/ }).innerText()).includes(fixture.customerName), 'Native hosting customer context was lost');
  assert.ok((await page.getByRole('navigation', { name: 'Breadcrumb', exact: true }).innerText()).includes(fixture.projectName), 'Native hosting project breadcrumb was lost');
}
async function completedOperation() {
  await page.getByRole('link', { name: 'View operation', exact: true }).click();
  const terminal = page.getByRole('status').filter({ hasText: /^(succeeded|failed|needs-reconciliation)$/ });
  await terminal.waitFor({ timeout: 120000 });
  assert.equal(await terminal.textContent(), 'succeeded', 'Native runtime did not confirm successful completion');
  await assertContext();
  await page.getByRole('link', { name: 'Back to applications', exact: true }).click();
  await page.getByRole('link', { name: fixture.appName, exact: true }).click();
  await page.getByRole('heading', { name: fixture.appName, exact: true }).waitFor();
  await assertContext();
}
async function terminalOperation() {
  await page.getByRole('link', { name: 'View operation', exact: true }).click();
  const terminal = page.getByRole('status').filter({ hasText: /^(succeeded|failed|needs-reconciliation)$/ });
  await terminal.waitFor({ timeout: 120000 });
  assert.equal(await terminal.textContent(), 'succeeded', 'Native runtime did not confirm successful completion');
}
async function lifecycle(name, action, nextControl) {
  const overview = page.url();
  await page.getByRole('button', { name, exact: true }).click();
  const receipt = await accepted(action, () => page.getByRole('dialog').getByRole('button', { name: `Confirm ${name.toLowerCase()}`, exact: true }).click());
  const latest = page.getByRole('status').filter({ hasText: /Latest operation: (succeeded|failed|needs-reconciliation)/ });
  await latest.waitFor({ timeout: 120000 });
  assert.ok((await latest.innerText()).includes('Latest operation: succeeded'), 'Native lifecycle did not complete successfully');
  await page.waitForFunction(label => [...document.querySelectorAll('button')].some(button => button.textContent === label && !button.disabled), nextControl, { timeout: 120000 });
  assert.equal(page.url(), overview, 'Lifecycle controls must reopen without leaving the overview');
  assert.equal(await page.getByRole('heading', { name: 'Request recorded', exact: true }).count(), 0, 'The completed request ticket must retire after the fresh app read');
  await assertContext();
  return receipt;
}
try {
  await context.addCookies([{ name: fixture.cookie.name, value: fixture.cookie.value, url: fixture.origin, httpOnly: true, sameSite: 'Lax', secure: origin.protocol === 'https:' }]);
  await page.goto(`${fixture.origin}/tenants`);
  await page.getByRole('heading', { name: 'Your workspaces', exact: true }).waitFor();
  const workspace = page.getByRole('listitem').filter({ has: page.getByText(fixture.customerName, { exact: true }) });
  await workspace.getByRole('link', { name: 'Applications', exact: true }).click();
  await page.getByRole('link', { name: fixture.projectName, exact: true }).click();
  await page.getByRole('link', { name: fixture.appName, exact: true }).click();
  await assertContext();
  for (const suffix of ['/settings', '/logs', '']) {
    await page.goto(`${fixture.origin}/hosting/apps/${fixture.appID}${suffix}`);
    await assertContext();
  }
  await lifecycle('Restart application', 'apps.restart', 'Stop application');
  await lifecycle('Stop application', 'apps.stop', 'Start application');
  await lifecycle('Start application', 'apps.start', 'Stop application');
  await page.getByRole('link', { name: 'Logs', exact: true }).click();
  await accepted('apps.logs', () => page.getByRole('button', { name: 'Request fresh logs', exact: true }).click());
  const logPanel = page.getByRole('region', { name: 'Requested log snapshot', exact: true });
  await logPanel.getByRole('heading', { name: /^Log request: (succeeded|failed|needs-reconciliation)$/ }).waitFor({ timeout: 120000 });
  assert.equal(await logPanel.getByRole('heading').textContent(), 'Log request: succeeded');
  const logs = await logPanel.locator('pre').textContent();
  assert.ok(logs.length <= 12000, 'Native log result exceeded the bounded snapshot');
  if (fixture.expectedLog) assert.ok(logs.includes(fixture.expectedLog), 'Expected sanitized fixture log output was absent');
  await page.getByRole('link', { name: 'Settings', exact: true }).click();
  await page.getByRole('textbox', { name: 'CPU', exact: true }).fill(fixture.updateCPU ?? '0.2 cores');
  await page.getByRole('button', { name: 'Add variable', exact: true }).click();
  await page.getByRole('textbox', { name: 'Variable name 1', exact: true }).fill('WEBDOCK_PROOF_TOKEN');
  await page.getByLabel('Value 1', { exact: true }).fill('browser-proof-value');
  const update = page.waitForRequest(request => new URL(request.url()).pathname === '/api/console/hosting' && request.postDataJSON().action === 'apps.update');
  await accepted('apps.update', () => page.getByRole('button', { name: 'Request update', exact: true }).click());
  assert.deepEqual((await update).postDataJSON().environment, [{ name: 'WEBDOCK_PROOF_TOKEN', value: 'browser-proof-value' }]);
  await completedOperation();
  // A fresh read returns the name only; the saved value stays hidden and is never re-sent.
  await page.getByRole('link', { name: 'Settings', exact: true }).click();
  await page.getByRole('textbox', { name: 'Variable name 1', exact: true }).waitFor();
  assert.equal(await page.getByRole('textbox', { name: 'Variable name 1', exact: true }).inputValue(), 'WEBDOCK_PROOF_TOKEN');
  await page.getByText('Saved value — hidden', { exact: true }).waitFor();
  assert.ok(!(await page.locator('body').textContent()).includes('browser-proof-value'), 'Saved environment value was rendered');
  // Create a second application from the Console, wait for the runtime, then delete it by plan.
  await page.goto(`${fixture.origin}/hosting/projects/${fixture.projectID}`);
  await page.getByRole('button', { name: 'New application', exact: true }).click();
  const creation = page.getByRole('region', { name: 'New application', exact: true });
  await creation.getByRole('textbox', { name: 'Application name', exact: true }).fill(fixture.createName);
  await creation.getByRole('textbox', { name: 'Image digest', exact: true }).fill(fixture.createImage);
  await creation.getByRole('textbox', { name: 'Startup arguments', exact: true }).fill(JSON.stringify(fixture.createArgs));
  await creation.getByRole('textbox', { name: 'Memory', exact: true }).fill('32 MB');
  await creation.getByRole('textbox', { name: 'Temporary storage', exact: true }).fill('16 MB');
  await accepted('apps.create', () => creation.getByRole('button', { name: 'Request application', exact: true }).click());
  await terminalOperation();
  await page.goto(`${fixture.origin}/hosting/projects/${fixture.projectID}`);
  await page.getByRole('link', { name: fixture.createName, exact: true }).click();
  await page.getByRole('heading', { name: fixture.createName, exact: true }).waitFor();
  await page.getByRole('button', { name: 'Review deletion', exact: true }).click();
  const dialog = page.getByRole('dialog');
  assert.match(await dialog.innerText(), /Deployment, Service, Secret/);
  const confirm = dialog.getByRole('button', { name: 'Confirm deletion', exact: true });
  await dialog.getByRole('textbox', { name: 'Type the application name to confirm', exact: true }).fill(fixture.createName.toUpperCase());
  assert.equal(await confirm.isDisabled(), true);
  await dialog.getByRole('textbox', { name: 'Type the application name to confirm', exact: true }).fill(fixture.createName);
  await accepted('apps.delete', () => confirm.click());
  await terminalOperation();
  for (const width of [320, 375, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `Native hosting layout overflow at ${width}px`);
  }
  const screenshotDir = process.env.WEBDOCK_HOSTING_BROWSER_SCREENSHOT_DIR;
  if (screenshotDir) {
    mkdirSync(screenshotDir, { recursive: true, mode: 0o700 });
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.screenshot({ path: join(screenshotDir, 'hosting-overview-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 375, height: 900 });
    await page.screenshot({ path: join(screenshotDir, 'hosting-overview-mobile.png'), fullPage: true });
  }
  assert.ok(!(await page.locator('body').textContent()).includes(fixture.cookie.value), 'Session credential appeared in the rendered interface');
  assert.deepEqual(errors, [], 'Native hosting browser emitted page errors');
  console.log('Native hosting browser passed: workspace navigation, restart/stop/start, durable completion, sanitized log snapshot, settings and write-only environment update reaching the container, Console creation and plan-confirmed deletion, and responsive layout.');
} finally { await context.close(); await browser.close(); }
