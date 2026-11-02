// Focused shared building blocks for new owned native acceptance. Existing PG scripts stay intact.
import assert from 'node:assert/strict';
import { createOTP } from '@better-auth/utils/otp';
import { base32 } from '@better-auth/utils/base32';
import { random, sha256, custodyIntent, custodyConfirm, expectNamespace, runtimeNamespace } from '../../../../tools/tests/native-plugin-fixture.mjs';

export function assertForeignDenied(status, body, privateValues = []) {
  assert([403, 404].includes(status), 'foreign_mailbox_access_not_denied');
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  assert(object(body) && Object.keys(body).length === 1 && Object.keys(body)[0] === 'error' && object(body.error), 'foreign_denial_payload_unsafe');
  assert(Object.keys(body.error).length === 2 && typeof body.error.code === 'string' && body.error.code.length > 0 && typeof body.error.message === 'string', 'foreign_denial_payload_unsafe');
  assert(Object.keys(body.error).every(key => ['code', 'message'].includes(key)) && !privateValues.filter(Boolean).some(value => JSON.stringify(body).includes(value)), 'foreign_denial_payload_unsafe');
}

export function waitMailReady(fixture, readMailbox, readDomain) {
  return fixture.wait(async () => {
    const value = await readMailbox(); assert(value.state !== 'failed', 'mailbox_reconciliation_failed');
    const { incoming } = await readDomain();
    assert(incoming && !['failed', 'migration-required'].includes(incoming.state), 'incoming_reconciliation_failed');
    return value.state === 'ready' && incoming.state === 'ready' && incoming.desiredEnabled === true && value;
  }, 300000);
}
export async function finishAccountNavigation(fixture, page, origin) {
  await fixture.wait(async () => {
    if (new URL(page.url()).origin === origin) return true;
    const allow = page.getByRole('button', { name: 'Allow access', exact: true });
    if (await allow.isVisible()) await allow.click();
    return false;
  }, 60000);
}

export function lifecycle(fixture, page, state, save) {
  const origin = fixture.report.origins.console, auth = fixture.report.origins.account;
  const responseFor = (path, method = 'POST') => page.waitForResponse(r => new URL(r.url()).pathname === path && r.request().method() === method);
  const get = async path => { const r = await page.request.get(origin + path); assert.equal(r.status(), 200, 'console_read_failed'); return r.json(); };
  const write = async (path, body, method = 'POST', expected = 200) => {
    const item = await custodyIntent(fixture.custody, fixture.saveCustody, 'api-request', String(fixture.custody.entries.length), { path, method, bodySha256: sha256(JSON.stringify(body)) });
    const r = await page.request.fetch(origin + path, { method, data: body, headers: { Origin: origin } });
    assert.equal(r.status(), expected, 'console_write_failed');
    const value = await r.json(); await custodyConfirm(item, fixture.saveCustody, { responseID: value.id ?? value.resource?.id ?? null }); return value;
  };
  async function account() {
    const request = await custodyIntent(fixture.custody, fixture.saveCustody, 'api-request', 'account-security');
    const operator = await fixture.json(fixture.report.paths.operator);
    await page.goto(origin + '/extensions');
    await page.waitForURL(auth + '/**');
    await page.getByRole('textbox', { name: 'Email', exact: true }).fill(operator.email);
    await page.getByLabel('Password', { exact: true }).fill(operator.password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.getByRole('heading', { name: 'Account security', exact: true }).waitFor();
    state.replacement = 'Owned-' + random() + '!'; await save();
    await page.getByLabel('Current password', { exact: true }).fill(operator.password);
    await page.getByLabel('New password', { exact: true }).fill(state.replacement);
    await page.getByLabel('Confirm new password', { exact: true }).fill(state.replacement);
    await page.getByRole('button', { name: 'Update password', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Password updated.' }).waitFor();
    await page.getByLabel('Password', { exact: true }).fill(state.replacement);
    await page.getByRole('button', { name: 'Set up authenticator', exact: true }).click();
    await page.getByRole('button', { name: 'Verify and enable', exact: true }).waitFor();
    state.totpSecret = (await page.locator('.setup-key').textContent()).trim(); await save();
    const otp = createOTP(new TextDecoder().decode(base32.decode(state.totpSecret)));
    await page.getByLabel('Authentication code', { exact: true }).fill(await otp.totp());
    await page.getByRole('button', { name: 'Verify and enable', exact: true }).click();
    await page.getByRole('heading', { name: 'Save your recovery codes', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Download codes', exact: true }).click();
    await page.getByRole('button', { name: 'I saved my recovery codes', exact: true }).click();
    await finishAccountNavigation(fixture, page, origin);
    state.accountReady = true; await save();
    await custodyConfirm(request, fixture.saveCustody, {});
  }
  async function waitOperation(id) {
    let value;
    await fixture.wait(async () => { value = await get('/api/console/plugins/operations/' + id); return ['succeeded', 'failed', 'blocked'].includes(value.status); }, 300000);
    assert.equal(value.status, 'succeeded', 'native_operation_not_succeeded');
    await page.goto(origin + '/extensions/operations/' + id);
    await page.getByText('succeeded', { exact: true }).waitFor(); return value;
  }
  async function exitOwner() {
    const s = (await get('/api/console/session')).data;
    if (s.user.preview || s.user.acting) await write('/api/console/tenant-preview/exit', {});
  }
  async function owner(customerID) {
    await exitOwner(); await page.goto(origin + '/customers/' + customerID);
    await page.getByRole('button', { name: 'Act as customer owner', exact: true }).click();
    await page.waitForURL(origin + '/workspaces/' + customerID);
    assert.equal((await write('/api/console/tenant-preview/current', {})).customerID, customerID);
  }
  async function install(version) {
    await expectNamespace(fixture, runtimeNamespace(fixture.report.platformID, 1));
    await page.goto(origin + '/extensions');
    assert.equal((await get('/api/console/registry/customers')).totalDocs, 0, 'fresh_install_has_customers');
    await page.getByRole('link', { name: `Review mail ${version}`, exact: true }).click();
    await page.getByRole('heading', { name: 'Install mail', exact: true }).waitFor();
    await page.getByRole('checkbox', { name: 'I reviewed this release, its permissions and the selected target.' }).check();
    const item = await custodyIntent(fixture.custody, fixture.saveCustody, 'api-request', 'console-install', { path: '/api/console/plugins/install' });
    const response = responseFor('/api/console/plugins/install');
    await page.getByRole('button', { name: 'Install extension', exact: true }).click();
    const reply = await response; assert.equal(reply.status(), 200, 'mail_install_admission_failed');
    const operation = await reply.json(); await custodyConfirm(item, fixture.saveCustody, { responseID: operation.id });
    state.installOperationID = operation.id; await save(); await waitOperation(operation.id);
  }
  async function feature(featureID) {
    await page.goto(origin + '/extensions/mail');
    const catalog = await fixture.json(fixture.report.paths.catalog), manifest = JSON.parse(catalog.releases[0].manifestRaw);
    const label = manifest.selfService.features.find(f => f.id === featureID).label;
    const row = page.getByRole('list', { name: 'Self-service', exact: true }).getByRole('listitem').filter({ has: page.getByRole('heading', { name: label, exact: true }) });
    const select = row.getByRole('combobox'); await select.waitFor();
    if (await select.inputValue() !== 'everyone') {
      const request = await custodyIntent(fixture.custody, fixture.saveCustody, 'api-request', 'feature-' + featureID);
      const response = responseFor('/api/console/plugins/mail/self-service/features/' + featureID, 'PUT');
      await select.selectOption('everyone'); assert.equal((await response).status(), 200, 'mail_feature_admission_failed');
      await custodyConfirm(request, fixture.saveCustody, {});
    }
  }
  async function change(kind, version, generation) {
    await exitOwner(); await page.goto(origin + '/extensions/mail/releases');
    const installed = await get('/api/console/plugins/installed');
    if (installed.find(p => p.id === 'mail').state !== 'disabled') {
      const request = await custodyIntent(fixture.custody, fixture.saveCustody, 'api-request', 'disable-' + kind);
      await page.getByRole('checkbox', { name: 'Disable this extension before changing its version.' }).check();
      await page.getByRole('button', { name: 'Disable for version change', exact: true }).click();
      await custodyConfirm(request, fixture.saveCustody, {});
    }
    const review = page.getByRole('button', { name: `Review ${kind === 'update' ? 'update' : 'rollback'} to ${version}`, exact: true });
    await fixture.wait(async () => !(await review.isDisabled()), 120000);
    await expectNamespace(fixture, runtimeNamespace(fixture.report.platformID, generation));
    await review.click(); await page.getByRole('checkbox', { name: 'I reviewed this release, its permissions and schema compatibility.' }).check();
    const item = await custodyIntent(fixture.custody, fixture.saveCustody, 'api-request', 'console-' + kind, { path: '/api/console/plugins/mail/' + kind });
    const response = responseFor('/api/console/plugins/mail/' + kind);
    await page.getByRole('button', { name: kind === 'update' ? 'Update extension' : 'Roll back extension', exact: true }).click();
    const reply = await response; assert.equal(reply.status(), 200, 'mail_lifecycle_admission_failed'); const value = await reply.json();
    await custodyConfirm(item, fixture.saveCustody, { responseID: value.id }); state[kind + 'OperationID'] = value.id; await save();
    await waitOperation(value.id); const releases = await get('/api/console/plugins/mail/releases');
    assert.equal(releases.currentGeneration, String(generation)); assert.equal(releases.candidateGeneration, null);
  }
  return { get, write, owner, exitOwner, account, install, feature, change, waitOperation, responseFor };
}
