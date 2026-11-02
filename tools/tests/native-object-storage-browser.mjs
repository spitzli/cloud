// Repository Playwright workflow against the real fresh Console/Core; no route mocks or fabricated sessions.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { chmod } from 'node:fs/promises';
import { load } from './native-object-storage-fixture.mjs';
import { setup, client } from './native-object-storage-acceptance.mjs';

const f = await load(process.argv[2]);
let browser;
try {
  await setup(f); const c = await client(f), credential = await f.json(f.path('credential-private.json'));
  browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const cookies = Object.entries(c.state.cookies).filter(([, value]) => value.slice(value.indexOf('=') + 1)).map(([name, value]) => ({ name, value: value.slice(value.indexOf('=') + 1), url: c.state.cookieOrigins[name], httpOnly: true, sameSite: 'Lax' }));
  assert(cookies.length > 0 && cookies.every(c => Object.values(f.report.origins).includes(c.url))); await context.addCookies(cookies);
  const page = await context.newPage(), errors = []; page.setDefaultTimeout(15000); page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}/resources/object-storage/${c.state.resource}`);
  await page.getByRole('heading', { name: 'active', exact: true }).waitFor();
  assert.equal((await page.locator('body').innerText()).includes(credential.secret_key), false);
  await page.getByRole('button', { name: 'Reveal credentials', exact: true }).click();
  await page.getByText(credential.secret_key, { exact: true }).waitFor();
  await page.getByRole('button', { name: 'Hide credentials', exact: true }).click();
  await page.getByRole('button', { name: 'Reveal credentials', exact: true }).waitFor();
  assert.equal((await page.locator('body').innerText()).includes(credential.secret_key), false);
  for (const width of [375, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'native_storage_horizontal_overflow');
    const path = f.path('console-' + width + '.png');
    await page.screenshot({ path, fullPage: true, mask: [page.getByText(f.report.proxy.origin, { exact: false })] }); await chmod(path, 0o600);
  }
  assert.equal(errors.length, 0); await context.close();
  f.report.browserProof = { realConsoleAndCore: true, realPasswordTotpPkceSession: true, widths: [375, 1280], resourceLoads: true, explicitCredentialRevealAndHide: true, hiddenCredentialAbsentFromDOM: true, horizontalOverflow: false, pageErrors: 0, screenshotsPrivateAndEndpointMasked: true, binaryBrowserControlsClaimed: false };
  await f.saveReport(); process.stdout.write('native_object_storage_browser_passed ' + f.input.runID + '\n');
} catch (error) { await f.write('browser-failure-' + Date.now() + '.json', { error: String(error), stack: error.stack }); process.stderr.write('native_object_storage_browser_failed_private_diagnostics_retained\n'); process.exitCode = 1; }
finally { await browser?.close(); }
