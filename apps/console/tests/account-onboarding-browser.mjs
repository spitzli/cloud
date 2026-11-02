// Test-only fixtures: the account surface's invitation steps and workspace list, no live services.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const answers = [];
  await page.route('**/console-config.json', json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [], surface: 'account', consoleOrigin: 'https://console.example.test' }));
  await page.route('**/api/auth/get-session', json({ user: { id: '1', name: 'Ada Lovelace', email: 'ada@example.test', role: 'user', emailVerified: true, twoFactorEnabled: false }, session: { id: 's1', userId: '1', expiresAt: '2026-11-20T10:00:00Z' } }));
  await page.route('**/api/tenants', json([{ id: '101', name: 'Example GmbH', status: 'active', role: 'owner' }]));
  await page.route('**/api/invitations', json([{ id: '9', role: 'admin', name: 'Acme Labs' }]));
  await page.route('**/api/invitation/**', (route) => {
    if (route.request().method() === 'POST') { answers.push(route.request().postDataJSON()); return route.fulfill({ json: {} }); }
    return route.fulfill({ json: { organizationName: 'Example GmbH', email: 'ada@example.test', role: 'owner', status: 'pending', customerID: '101' } });
  });
  await page.route('**/api/tenant/**', json({ tenant: { customer_type: 'company', name: 'Example GmbH', company_name: 'Example GmbH', country: 'DE' } }));
  // The first run after a new account offers every step: join, customer details, passkey, done.
  await page.goto(previewOrigin + '/invitation?id=9&welcome=1');
  const progress = page.getByRole('list', { name: 'Setup progress', exact: true });
  await page.getByRole('heading', { name: 'Join Example GmbH' }).waitFor();
  assert.equal(await progress.locator('[aria-current="step"]').innerText(), '2\nJoin');
  await page.getByRole('button', { name: 'Accept invitation', exact: true }).click();
  assert.deepEqual(answers, [{ accept: true }]);
  await page.getByRole('heading', { name: 'Confirm your customer details' }).waitFor();
  assert.equal(await page.getByRole('textbox', { name: 'Company name', exact: true }).inputValue(), 'Example GmbH', 'prepared details are prefilled');
  await page.getByRole('button', { name: 'Later', exact: true }).click();
  const passkey = page.getByRole('heading', { name: 'Add a passkey' });
  if (await page.evaluate(() => 'PublicKeyCredential' in window)) {
    await passkey.waitFor();
    await page.getByRole('button', { name: 'Not now', exact: true }).click();
  }
  await page.getByRole('heading', { name: "You're all set" }).waitFor();
  assert.equal(await page.getByRole('link', { name: 'Open Console', exact: true }).getAttribute('href'), 'https://console.example.test/api/sso/login?returnTo=%2Fworkspaces%2F101', 'owners land on their workspace overview in Console');
  // Workspaces replace the old websites page; older links still arrive here.
  await page.goto(previewOrigin + '/sites');
  await page.getByRole('heading', { name: 'Workspaces', exact: true }).waitFor();
  await page.getByRole('link', { name: 'Review invitation', exact: true }).waitFor();
  assert.equal(await page.getByRole('link', { name: 'Open in Console', exact: true }).getAttribute('href'), 'https://console.example.test/api/sso/login?returnTo=%2Fworkspaces%2F101');
  assert.deepEqual(errors, []);
  console.log('Account onboarding checks passed: join, prefilled details, optional passkey, Console hand-off and workspaces. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
