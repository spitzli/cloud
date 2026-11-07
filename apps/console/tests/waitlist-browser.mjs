// Test-only HTTP fixtures; no production identities, mail or customer writes.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import { fromRepo } from './repo.mjs';

const output = fileURLToPath(fromRepo('.runtime-test/waitlist-operator-20261107'));
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
const routeWarnings = [];
const path = value => url => url.pathname === value;
async function open(locale = 'en', role = 'operator') {
  const context = await browser.newContext({ viewport: { width: 375, height: 812 } });
  const page = await context.newPage();
  page.setDefaultTimeout(15_000);
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (/Generated path.*access-requests/.test(message.text())) routeWarnings.push(message.text()); });
  await page.route(url => url.pathname.startsWith('/api/'), route => route.fulfill({ json: [] }));
  await page.route(path('/console-config.json'), route => route.fulfill({ json: {
    apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [], localePreference: locale,
  } }));
  await page.route(path('/v1/auth/session'), route => route.fulfill({ json: { data: {
    user: { id: '1', name: 'Owned fixture', email: 'operator@example.test', role, emailVerified: true, twoFactorEnabled: true },
    session: { id: 'owned-session', userId: '1', expiresAt: '2099-01-01T00:00:00Z' },
  } } }));
  return { context, page };
}
async function noOverflow(page) {
  for (const width of [320, 375, 1280]) {
    await page.setViewportSize({ width, height: 812 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `Overflow at ${width}px`);
  }
}
try {
  for (const locale of ['en', 'de']) {
    const { context, page } = await open(locale);
    let finish;
    let attempts = 0;
    const authRequests = [];
    page.on('request', request => { if (/^\/(v1\/auth\/|api\/auth\/|api\/console\/session$)/.test(new URL(request.url()).pathname)) authRequests.push(request.url()); });
    await page.route(path('/api/console/waitlist'), async route => {
      assert.equal(route.request().method(), 'POST');
      assert.deepEqual(route.request().postDataJSON(), { email: 'contact@example.test', name: 'Owned public fixture', privacyAccepted: true });
      attempts++;
      if (attempts === 1) {
        await new Promise(resolve => { finish = resolve; });
        return route.fulfill({ status: 429, json: { error: { message: 'internal limiter details' } } });
      }
      return route.fulfill({ status: 202, json: { accepted: true } });
    });
    try {
      await page.goto(previewOrigin + '/waitlist');
      await page.getByRole('heading', { name: locale === 'en' ? 'Get early access' : 'Frühzeitig Zugang erhalten', exact: true }).waitFor();
      await page.locator('input[name=email]').fill('contact@example.test');
      await page.locator('input[name=name]').fill('Owned public fixture');
      await page.locator('input[name=privacy]').check();
      await noOverflow(page);
      await page.getByRole('button', { name: locale === 'en' ? 'Join the waitlist' : 'Auf die Warteliste', exact: true }).click();
      await page.waitForFunction(() => document.querySelector('fieldset')?.disabled === true);
      assert.equal(await page.locator('input[name=email]').isDisabled(), true);
      finish();
      await page.getByRole('alert').filter({ hasText: locale === 'en' ? 'Too many requests' : 'Zu viele Anfragen' }).waitFor();
      assert.equal(await page.locator('input[name=email]').inputValue(), 'contact@example.test');
      assert.equal(await page.locator('input[name=privacy]').isChecked(), true);
      assert.doesNotMatch(await page.locator('body').innerText(), /internal limiter details/);
      await page.locator('button[type=submit], form button').last().focus();
      await page.keyboard.press('Enter');
      await page.waitForURL('**/waitlist/confirmed');
      await page.getByRole('heading', { name: locale === 'en' ? "You're on the waitlist" : 'Du stehst auf der Warteliste', exact: true }).waitFor();
      assert.equal(attempts, 2); assert.deepEqual(authRequests, []);
      await page.screenshot({ path: `${output}/public-${locale}.png`, fullPage: true });
    } finally { await context.close(); }
  }

  // Owned operator fixtures: search/filter/page boundaries and confirmation never write implicitly.
  for (const locale of ['en', 'de']) {
    const audit = await open(locale);
    const rows = [
      { id: '21', email: 'ada@example.test', name: 'Ada Lovelace', status: 'pending', createdAt: '2026-11-07T09:00:00Z', reviewedAt: null, version: 1 },
      { id: '22', email: 'grace@example.test', name: 'Grace Hopper', status: 'approved', createdAt: '2026-11-06T09:00:00Z', reviewedAt: '2026-11-07T08:00:00Z', version: 2 },
      { id: '23', email: 'linus@example.test', name: 'Linus Example', status: 'rejected', createdAt: '2026-11-05T09:00:00Z', reviewedAt: '2026-11-06T08:00:00Z', version: 2 },
    ];
    let reads = 0, writes = 0;
    await audit.page.route(path('/api/console/waitlist'), route => { reads++; return route.fulfill({ json: { entries: rows, nextCursor: 'older-page' } }); });
    await audit.page.route(url => /^\/api\/console\/waitlist\/\d+$/.test(url.pathname), route => { writes++; return route.fulfill({ status: 500 }); });
    try {
      await audit.page.goto(previewOrigin + '/access-requests');
      const list = audit.page.getByRole('list', { name: locale === 'en' ? 'Access requests' : 'Zugangsanfragen', exact: true });
      await list.getByText('Grace Hopper', { exact: true }).waitFor();
      assert.equal(await list.getByRole('listitem').count(), 3);
      await noOverflow(audit.page);
      const status = audit.page.getByRole('group', { name: locale === 'en' ? 'Filter by status' : 'Nach Status filtern', exact: true });
      await status.getByRole('button').first().click();
      assert.equal(await list.getByRole('listitem').count(), 1);
      const search = audit.page.getByRole('searchbox');
      await search.fill('missing');
      await audit.page.getByRole('heading', { name: locale === 'en' ? 'No matching requests' : 'Keine passenden Anfragen', exact: true }).waitFor();
      await audit.page.getByRole('button', { name: locale === 'en' ? 'Clear filters' : 'Filter zurücksetzen', exact: true }).click();
      await search.fill('GRACE@');
      assert.equal(await list.getByRole('listitem').count(), 1);
      await list.getByText('Grace Hopper', { exact: true }).waitFor();
      assert.equal(reads, 1, 'Local filters must not poll the Core or pretend to search unseen pages');
      await search.fill('');
      await list.getByRole('button', { name: locale === 'en' ? 'Reject' : 'Ablehnen', exact: true }).click();
      const confirmation = audit.page.getByRole('group', { name: locale === 'en' ? 'Confirm rejection' : 'Ablehnung bestätigen', exact: true });
      await confirmation.waitFor();
      assert.equal(writes, 0);
      const cancel = confirmation.getByRole('button', { name: locale === 'en' ? 'Keep pending' : 'Offen lassen', exact: true });
      assert.equal(await cancel.evaluate(element => document.activeElement === element), true, 'Confirmation must focus its safe cancellation action');
      await audit.page.keyboard.press('Enter');
      assert.equal(await confirmation.count(), 0);
      const reject = list.getByRole('button', { name: locale === 'en' ? 'Reject' : 'Ablehnen', exact: true });
      assert.equal(await reject.evaluate(element => document.activeElement === element), true, 'Cancellation returns focus to the same request');
      assert.equal(writes, 0);
      await audit.page.setViewportSize({ width: 1440, height: 1024 });
      await audit.page.getByRole('navigation', { name: 'Breadcrumb', exact: true }).getByText(locale === 'en' ? 'Waitlist' : 'Warteliste', { exact: true }).waitFor();
      await audit.page.screenshot({ path: `${output}/operator-${locale}-desktop.png`, fullPage: true });
      await audit.page.setViewportSize({ width: 375, height: 812 });
      await audit.page.screenshot({ path: `${output}/operator-${locale}-mobile.png`, fullPage: true });
      await audit.page.getByRole('button', { name: locale === 'en' ? 'Refresh requests' : 'Anfragen aktualisieren', exact: true }).click();
      await audit.page.waitForFunction(() => !document.querySelector('main button')?.disabled);
      assert.equal(reads, 2);
    } catch (error) { await audit.page.screenshot({ path: `${output}/failure-${locale}.png`, fullPage: true }); throw error; } finally { await audit.context.close(); }
  }

  const { context, page } = await open();
  const email = 'x'.repeat(64) + '@' + ('a'.repeat(61) + '.').repeat(2) + 'example.test';
  const entry = { id: '10', email, name: 'LongContact'.repeat(9), status: 'pending', createdAt: '2026-10-31T00:00:00Z', reviewedAt: null, version: 1 };
  let approved = false, finishReview;
  await page.route(path('/api/console/waitlist/10'), async route => {
    assert.deepEqual(route.request().postDataJSON(), { action: 'approve', version: 1 });
    await new Promise(resolve => { finishReview = resolve; });
    approved = true;
    await route.fulfill({ json: { ...entry, status: 'approved', version: 2, reviewedAt: '2026-10-31T01:00:00Z' } });
  });
  await page.route(path('/api/console/waitlist'), route => route.fulfill({ json: {
    entries: new URL(route.request().url()).searchParams.has('cursor') ? [] : [{ ...entry, status: approved ? 'approved' : 'pending', version: approved ? 2 : 1 }],
    nextCursor: new URL(route.request().url()).searchParams.has('cursor') ? null : 'older-page',
  } }));
  try {
    await page.goto(previewOrigin + '/access-requests');
    await page.getByRole('button', { name: 'Approve', exact: true }).waitFor();
    assert.equal(await page.getByRole('group', { name: email, exact: true }).getByRole('button').count(), 2, 'Review actions identify their contact to assistive technology');
    await noOverflow(page);
    await page.setViewportSize({ width: 375, height: 812 });
    await page.getByRole('button', { name: 'Approve', exact: true }).click();
    await page.getByRole('button', { name: 'Approving…', exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: 'Older requests' }).isDisabled(), true);
    finishReview();
    await page.getByRole('status').filter({ hasText: 'Request approved. Invite this contact' }).waitFor();
    await page.screenshot({ path: `${output}/operator-mobile.png`, fullPage: true });
    await page.getByRole('button', { name: 'Older requests' }).click();
    await page.getByRole('heading', { name: 'No access requests', exact: true }).waitFor();
    assert.equal(await page.getByRole('status').filter({ hasText: 'Request approved' }).count(), 0);
    await page.getByRole('button', { name: 'Newest requests' }).click();
    await page.getByText(email, { exact: false }).waitFor();
    await page.setViewportSize({ width: 1280, height: 812 });
    await page.goto(previewOrigin + '/customers');
    await page.getByRole('link', { name: 'Waitlist', exact: true }).click();
    await page.waitForURL('**/access-requests');
    await page.goto(previewOrigin + '/customers');
    await page.getByRole('heading', { name: 'Customers', exact: true }).waitFor();
    await page.keyboard.press('Control+k');
    await page.getByRole('dialog').getByRole('button', { name: 'Waitlist', exact: true }).click();
    await page.waitForURL('**/access-requests');
    assert.deepEqual(routeWarnings, [], 'Waitlist links must use their dedicated route');
  } finally { await context.close(); }

  const customer = await open('en', 'user');
  let unauthorizedReads = 0;
  await customer.page.route(path('/api/console/waitlist'), route => { unauthorizedReads++; return route.fulfill({ status: 403 }); });
  try {
    await customer.page.goto(previewOrigin + '/access-requests');
    await customer.page.getByRole('heading', { name: 'Operator access required', exact: true }).waitFor();
    assert.equal(unauthorizedReads, 0);
  } finally { await customer.context.close(); }
  assert.deepEqual(errors, []);
  console.log('PASS waitlist browser: EN/DE, 429 retry, retained consent, keyboard submit, 320/375/1280px, review/pagination, customer guard, zero JS errors. Test-only HTTP fixtures; not live auth or server authorization.');
} finally { await browser.close(); await server.close(); }
