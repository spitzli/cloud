import assert from 'node:assert/strict';
import { setWorkbenchMode, workbenchRequests } from './workbench-fixtures.ts';
import { previewOrigin as origin } from './preview-server.ts';
const updates = () => workbenchRequests.filter(value => value.body.action === 'apps.update');
async function openAvailability(page, desired) {
  await page.getByRole('button', { name: 'Change availability', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Requested availability', exact: true }).selectOption(desired);
  await dialog.getByRole('checkbox').check();
  return dialog;
}
export async function workbenchBrowserChecks(page) {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(`${origin}/tenants`);
  await page.getByRole('heading', { name: 'Your workspaces' }).waitFor();
  await page.getByRole('link', { name: 'Applications', exact: true }).last().click();
  await page.getByRole('link', { name: 'Acceptance hosting project', exact: true }).click();
  await page.getByRole('link', { name: 'Acceptance application', exact: true }).click();
  await assertHostingContext(page);
  await page.getByRole('link', { name: 'Settings', exact: true }).click();
  await assertHostingContext(page);
  await page.getByRole('textbox', { name: 'CPU', exact: true }).fill('0.5 cores');
  setWorkbenchMode('conflict');
  await page.getByRole('button', { name: 'Request update', exact: true }).click();
  await page.getByRole('heading', { name: 'Request was not accepted' }).waitFor();
  assert.equal(await page.getByRole('textbox', { name: 'CPU', exact: true }).inputValue(), '0.5 cores', 'conflict preserves draft');
  page.once('dialog', dialog => dialog.dismiss());
  await page.getByRole('link', { name: 'Overview', exact: true }).click();
  assert.ok(page.url().endsWith('/settings'), 'dirty hosting form blocks navigation');
  page.once('dialog', dialog => dialog.accept());
  await page.getByRole('button', { name: 'Load current settings', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('input[name=cpuMillicores]')?.value === '0.1 cores');
  await page.getByRole('textbox', { name: 'CPU', exact: true }).fill('0.5 cores');
  setWorkbenchMode('uncertain');
  await page.getByRole('button', { name: 'Request update', exact: true }).click();
  await page.getByRole('heading', { name: 'Request outcome is unknown' }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Request update', exact: true }).isDisabled(), true);
  await page.getByRole('button', { name: 'Retry same request', exact: true }).click();
  await page.getByRole('heading', { name: 'Request recorded', exact: true }).waitFor();
  assert.equal(await page.getByText('Unsaved settings', { exact: true }).count(), 0, 'successful retry clears dirty state');
  assert.equal(await page.getByText('Check the image digest, arguments and resource values. Use valid JSON for startup arguments.', { exact: true }).count(), 0, 'retry does not submit the disabled form again');
  const sent = updates();
  assert.deepEqual(sent.at(-1).body, sent.at(-2).body, 'uncertain retry keeps exact payload and identity');
  await page.getByRole('link', { name: 'View operation', exact: true }).click();
  await page.getByRole('status').filter({ hasText: /^succeeded$/ }).waitFor();
  await assertHostingContext(page);
  await page.getByRole('link', { name: 'Back to applications', exact: true }).click();
  await page.getByRole('link', { name: 'Acceptance application', exact: true }).click();
  const overview = page.url();
  await page.getByRole('button', { name: 'Restart application', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Confirm restart application', exact: true }).click();
  await page.waitForFunction(() => [...document.querySelectorAll('button')].some(button => button.textContent === 'Stop application' && !button.disabled));
  assert.equal(page.url(), overview, 'completed lifecycle controls must reopen on the overview');
  assert.equal(await page.getByRole('heading', { name: 'Request recorded', exact: true }).count(), 0);
  setWorkbenchMode('failed');
  await page.getByRole('button', { name: 'Stop application', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Confirm stop application', exact: true }).click();
  await page.getByRole('link', { name: 'View operation', exact: true }).click();
  await page.getByRole('status').filter({ hasText: /^failed$/ }).waitFor();
  await page.goto(`${origin}/hosting/apps/301/logs`);
  await page.getByRole('button', { name: 'Request fresh logs', exact: true }).click();
  await page.getByText('Fresh sanitized fixture log output', { exact: true }).first().waitFor();
  await page.getByText('This snapshot was truncated to the permitted size.', { exact: true }).waitFor();
  setWorkbenchMode('denied');
  await page.getByRole('button', { name: 'Request fresh logs', exact: true }).click();
  await page.getByRole('heading', { name: 'Request was not accepted' }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Retry same request', exact: true }).count(), 0, 'denial is not an uncertain retry');
  await page.getByRole('link', { name: 'Extensions', exact: true }).click();
  await page.getByRole('heading', { name: 'Installed extensions', exact: true }).waitFor();
  setWorkbenchMode('conflict');
  let dialog = await openAvailability(page, 'disabled');
  await dialog.getByRole('button', { name: 'Request availability change' }).click();
  await dialog.getByRole('heading', { name: 'Request was not accepted' }).waitFor();
  assert.equal(await dialog.getByRole('combobox', { name: 'Requested availability', exact: true }).inputValue(), 'disabled');
  await dialog.getByRole('button', { name: /Close Availability/ }).click();
  setWorkbenchMode('uncertain');
  dialog = await openAvailability(page, 'disabled');
  await dialog.getByRole('button', { name: 'Request availability change' }).click();
  await dialog.getByRole('heading', { name: 'Request outcome is unknown' }).waitFor();
  await dialog.getByRole('button', { name: 'Retry same request', exact: true }).click();
  await dialog.getByRole('link', { name: 'View operation', exact: true }).waitFor();
  const changes = workbenchRequests.filter(value => value.path === '/api/console/plugins/installed');
  assert.deepEqual(changes.at(-1).body, changes.at(-2).body, 'extension retry keeps exact revision and identity');
  await dialog.getByRole('link', { name: 'View operation', exact: true }).click();
  await page.getByRole('status').filter({ hasText: /^succeeded$/ }).waitFor();
  await page.getByRole('link', { name: 'Installed extensions', exact: true }).click();
  await page.getByRole('cell', { name: 'ready', exact: true }).waitFor();
  await page.getByRole('cell', { name: /disabled\s*Requested: disabled/ }).waitFor();
  setWorkbenchMode('blocked');
  dialog = await openAvailability(page, 'enabled');
  await dialog.getByRole('button', { name: 'Request availability change' }).click();
  await dialog.getByRole('link', { name: 'View operation', exact: true }).click();
  await page.getByRole('status').filter({ hasText: /^blocked$/ }).waitFor();
  await page.getByRole('alert').filter({ hasText: 'active_continuations' }).waitFor();
  await page.route('**/v1/auth/session', async route => {
    const response = await route.fetch(); const data = await response.json(); data.data.user.preview = true;
    await route.fulfill({ response, json: data });
  });
  await page.goto(`${origin}/hosting/apps/301`);
  await page.getByRole('heading', { name: 'Acceptance application', exact: true }).waitFor();
  assert.equal(await page.getByRole('button', { name: /Stop application|Restart application|Rollback application/ }).count(), 0);
  await page.getByRole('link', { name: 'Logs', exact: true }).click();
  assert.equal(await page.getByRole('button', { name: 'Request fresh logs' }).isDisabled(), true);
  await page.goto(`${origin}/extensions`);
  await page.getByRole('heading', { name: 'Operator access required' }).waitFor();
  await page.unroute('**/v1/auth/session');
  await page.route('**/api/console/hosting', async route => {
    if (route.request().postDataJSON().action !== 'apps.list') { await route.continue(); return; }
    await route.fulfill({ json: { docs: [], page: 1, limit: 20, totalDocs: 0, totalPages: 0, operator: true, canManage: true, ownImages: true, subscriptionRevision: 3 } });
  });
  await page.goto(`${origin}/hosting/projects/201`);
  await page.getByRole('heading', { name: 'No applications', exact: true }).waitFor();
  await page.unroute('**/api/console/hosting');
  const registryRequests = [];
  const observeRegistry = request => { if (new URL(request.url()).pathname.startsWith('/v1/operator/registry/')) registryRequests.push(request.url()); };
  await page.route('**/v1/auth/session', async route => {
    const response = await route.fetch(); const data = await response.json(); data.data.user.role = 'reader';
    await route.fulfill({ response, json: data });
  });
  page.on('request', observeRegistry);
  const operationID = workbenchRequests.filter(value => value.body.action === 'operations.get').at(-1).body.operationID;
  for (const path of ['/hosting/apps/301', '/hosting/apps/301/settings', '/hosting/apps/301/logs', `/hosting/operations/${operationID}`]) {
    await page.goto(`${origin}${path}?customer=999`);
    await assertHostingContext(page);
  }
  assert.deepEqual(registryRequests, [], 'nonoperator hosting context must never query the operator registry');
  page.off('request', observeRegistry);
  await page.unroute('**/v1/auth/session');
  await page.goto(`${origin}/hosting/apps/301`);
  for (const width of [320, 375, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `hosting fits ${width}px`);
  }
}

async function assertHostingContext(page) {
  await page.getByRole('navigation', { name: 'Primary navigation', exact: true }).getByRole('link', { name: 'Acceptance hosting project', exact: true }).waitFor();
  const workspace = await page.getByRole('button', { name: /^Switch (?:customer or project|workspace)$/ }).innerText();
  assert.ok(workspace.includes('Acceptance customer'), 'Hosting context must retain the authorized customer');
  const breadcrumb = await page.getByRole('navigation', { name: 'Breadcrumb', exact: true }).innerText();
  assert.ok(breadcrumb.includes('Acceptance customer') && breadcrumb.includes('Acceptance hosting project'));
}
