// Test-only fixtures: the mail plugin's own React pages inside a customer project.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { server, previewOrigin } from './preview-server.ts';
import checkedManifest from '@plugins/mail/manifest.json' with { type: 'json' };
const manifest = { ...checkedManifest, actions: [...checkedManifest.actions.filter(a => !['mail.status', 'mail.cancel', 'mail.history'].includes(a.name)), ...['mail.status', 'mail.cancel', 'mail.history'].map(name => ({ name, intent: name === 'mail.cancel' ? 'write' : 'read', audience: 'customer', resource: 'use' }))] };
const domainID = '00000000-0000-4000-8000-0000000000d1';
const relayID = '00000000-0000-4000-8000-0000000000e1';
const mailboxID = '00000000-0000-4000-8000-0000000000b1';
const state = { verified: false, recent: [], calls: [], jobs: [], submissions: new Map(), loseSendReply: true, failedDomainReads: 0, legacy: false, relay: true, mailbox: false, mailboxViews: 0, incoming: 'disabled', desiredEnabled: false };
async function open(browser, member) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  const json = (body) => (route) => route.fulfill({ json: body });
  const path = (p) => (url) => url.pathname === p;
  await page.route(path('/console-config.json'), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path('/v1/auth/session'), json({ data: { user: { id: '2', name: 'Ada Owner', email: 'ada@example.test', role: 'user', emailVerified: true }, session: { id: 's-ada', userId: '2', expiresAt: '2026-12-01T10:00:00Z' } } }));
  await page.route(path('/api/console/tenants'), json([{ id: '101', name: 'Demo customer', status: 'active', role: member }]));
  await page.route(path('/api/console/tenant/101/projects'), json([{ id: '201', name: 'Demo project', status: 'active' }]));
  await page.route(path('/api/console/project/201'), json({ id: '201', name: 'Demo project', customer: { id: '101', name: 'Demo customer' }, role: member }));
  await page.route(path('/api/console/plugins'), (route) => route.fulfill({ json: [{ state: 'enabled', runtime: 'ready', manifest: state.legacy ? { ...manifest, actions: manifest.actions.filter(a => !['mail.history', 'mail.status', 'mail.cancel'].includes(a.name)) } : manifest, generation: 1, manifestSha256: 'a'.repeat(64), launchLinks: [], features: member === 'owner' ? ['add-domains', 'byok-turbosmtp', 'create-mailboxes', 'manage-incoming'] : [], resources: [{ id: domainID, offering: 'domain', name: 'shop.example.com', createdVia: 'self-service' }, ...(state.relay ? [{ id: relayID, offering: 'relay', name: 'turboSMTP account', createdVia: 'self-service' }] : []), ...(state.mailbox ? [{ id: mailboxID, offering: 'mailbox', name: 'anna@shop.example.com', createdVia: 'self-service' }] : [])] }] }));
  await page.route(path('/api/console/plugins/invoke'), (route) => {
    const body = route.request().postDataJSON();
    state.calls.push(body);
    if (body.action === 'domains.view') {
      if (state.failedDomainReads > 0) { state.failedDomainReads--; return route.fulfill({ status: 503, json: { error: { code: 'unavailable', message: 'Domain refresh temporarily unavailable' } } }); }
      return route.fulfill({ json: { domain: 'shop.example.com', verified: state.verified, verificationSkipped: false, transport: 'platform', records: [{ type: 'TXT', name: '_webdock.shop.example.com', value: 'webdock-verify=abc', purpose: 'Proves you own the domain' }], mailboxes: state.mailbox ? ['anna@shop.example.com'] : [], recent: state.recent, incoming: { state: state.incoming, desiredEnabled: state.desiredEnabled } } });
    }
    if (body.action === 'domains.verify') { state.verified = true; return route.fulfill({ json: { verified: true } }); }
    if (body.action === 'mail.send') {
      let job = state.submissions.get(body.idempotencyKey);
      if (job && job.input !== JSON.stringify(body.input)) return route.fulfill({ status: 409, json: { error: { code: 'conflict', message: 'Idempotency conflict' } } });
      if (!job) {
        job = { id: String(700 + state.jobs.length), input: JSON.stringify(body.input), from: `${body.input.from}@shop.example.com`, to: body.input.to.split(',').map(address => address.trim()), subject: body.input.subject, transport: 'platform', state: 'queued', at: '2026-10-22 10:00:00+00' };
        state.submissions.set(body.idempotencyKey, job); state.jobs.push(job);
      }
      if (state.loseSendReply) { const failure = state.loseSendReply; state.loseSendReply = false; state.failedDomainReads = 1; return failure === 'forbidden' ? route.fulfill({ status: 403, json: { error: { code: 'permission_denied', message: 'Authority changed before result release' } } }) : route.abort('connectionfailed'); }
      return route.fulfill({ json: { id: job.id, state: job.state } });
    }
    if (body.action === 'mail.history') return route.fulfill({ json: { items: state.jobs.map(({ id, state, from, to, subject, transport, at }) => ({ id, state, from, to, subject, transport, at })) } });
    if (body.action === 'mail.status') { const job = state.jobs.find(j => j.id === body.input.id); return route.fulfill({ json: { id: job.id, state: job.state } }); }
    if (body.action === 'mail.cancel') { const job = state.jobs.find(j => j.id === body.input.id); if (['staging', 'queued', 'leased', 'preparation_review'].includes(job.state)) job.state = 'cancelled'; return route.fulfill({ json: { id: job.id, state: job.state } }); }
    if (body.action === 'relay.view') return route.fulfill({ json: { provider: 'turboSMTP', region: 'EU', keyHint: '…a1b2', connectedAt: '2026-10-22 09:00:00+00' } });
    if (body.action === 'relay.delete') { state.relay = false; return route.fulfill({ json: {} }); }
    if (body.action === 'incoming.deactivate') { state.incoming = 'suspending'; state.desiredEnabled = false; return route.fulfill({ json: {} }); }
    if (body.action === 'mailboxes.create') { state.mailbox = true; state.incoming = 'ready'; state.desiredEnabled = true; return route.fulfill({ json: { resource: { id: mailboxID, name: 'anna@shop.example.com' } } }); }
    if (body.action === 'mailboxes.view') {
      state.mailboxFirst ||= Date.now();
      const ready = Date.now() - state.mailboxFirst > 1500;
      return route.fulfill({ json: { address: 'anna@shop.example.com', name: 'Anna', state: ['suspending', 'suspended'].includes(state.incoming) ? state.incoming : ready ? 'ready' : 'creating', error: null, host: ready ? '192.168.80.2' : null, imapPort: 1993, smtpPort: 2465, username: 'anna@shop.example.com', quotaMb: 1024, tls: 'Implicit TLS, self-signed certificate (local development)' } });
    }
    if (body.action === 'mailboxes.credentials') return route.fulfill({ json: { value: 'feedc0de' } });
    return route.fulfill({ status: 404, json: {} });
  });
  return { page, context, errors };
}
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  // 1. A new domain must be verified before it can send.
  let session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/mail/${domainID}`);
  await session.page.getByText('Not verified').waitFor();
  assert.ok(await session.page.getByRole('button', { name: 'Send', exact: true }).isDisabled());
  await session.page.getByRole('button', { name: 'Check DNS' }).click();
  await session.page.getByText('Verified', { exact: true }).waitFor();

  // 2. Lost admission replies retry the same logical send; queueing never claims delivery.
  await session.page.getByRole('textbox', { name: 'To' }).fill('someone@example.net');
  await session.page.getByRole('textbox', { name: 'Subject' }).fill('Hello there');
  await session.page.getByRole('textbox', { name: 'Message' }).fill('First message');
  await session.page.getByRole('button', { name: 'Send', exact: true }).click();
  await session.page.getByRole('alert').filter({ hasText: 'Unable to load' }).first().getByRole('button', { name: 'Try again', exact: true }).click();
  await session.page.getByRole('textbox', { name: 'Message', exact: true }).waitFor();
  assert.equal(await session.page.getByRole('textbox', { name: 'Message', exact: true }).inputValue(), 'First message', 'background refresh failure must retain the unresolved input');
  await session.page.getByRole('button', { name: 'Retry same message', exact: true }).waitFor({ timeout: 3000 });
  assert.equal(await session.page.getByRole('textbox', { name: 'Message', exact: true }).isDisabled(), true);
  await session.page.getByRole('button', { name: 'Retry same message', exact: true }).click();
  await session.page.getByText('Queued for someone@example.net.', { exact: true }).waitFor();
  assert.equal(await session.page.getByText('Sent to someone@example.net.', { exact: true }).count(), 0);
  const outbox = session.page.getByRole('region', { name: 'Outbox', exact: true });
  await outbox.getByText('hello@shop.example.com to someone@example.net', { exact: true }).waitFor({ timeout: 3000 });
  await outbox.getByText('Hello there', { exact: true }).waitFor();
  assert.equal(state.jobs.length, 1);
  const retries = state.calls.filter(call => call.action === 'mail.send');
  assert.equal(retries.length, 2); assert.equal(retries[0].idempotencyKey, retries[1].idempotencyKey); assert.deepEqual(retries[0].input, retries[1].input);
  const sent = state.calls.find((call) => call.action === 'mail.send');
  assert.deepEqual(sent.input, { resource_id: domainID, from: 'hello', from_name: '', to: 'someone@example.net', subject: 'Hello there', text: 'First message' });

  state.jobs[0].state = 'unknown'; await outbox.getByRole('button', { name: 'Refresh outbox', exact: true }).click();
  await outbox.getByText('Delivery unconfirmed', { exact: true }).waitFor();
  assert.equal(await outbox.getByRole('button', { name: 'Cancel queued message', exact: true }).count(), 0);
  assert.equal(state.calls.filter(call => call.action === 'mail.send').length, 2, 'status refresh never resends');
  await session.page.getByRole('textbox', { name: 'To' }).fill('second@example.net, third@example.net');
  await session.page.getByRole('textbox', { name: 'Subject' }).fill('Cancel me');
  await session.page.getByRole('textbox', { name: 'Message' }).fill('Second logical message');
  state.loseSendReply = 'forbidden';
  await session.page.getByRole('button', { name: 'Send', exact: true }).click();
  await session.page.getByRole('alert').filter({ hasText: 'Unable to load' }).first().getByRole('button', { name: 'Try again', exact: true }).click();
  await session.page.getByRole('button', { name: 'Retry same message', exact: true }).waitFor({ timeout: 3000 });
  assert.equal(await session.page.getByRole('textbox', { name: 'Message', exact: true }).isDisabled(), true, 'an error response is not proof that admission failed');
  await session.page.getByRole('button', { name: 'Retry same message', exact: true }).click();
  await session.page.getByText('Queued for second@example.net, third@example.net.', { exact: true }).waitFor();
  assert.equal(state.jobs.length, 2); assert.notEqual(state.calls.filter(call => call.action === 'mail.send').at(-1).idempotencyKey, sent.idempotencyKey);
  const cancellableMessage = outbox.getByRole('listitem').filter({ hasText: 'Cancel me' });
  await cancellableMessage.getByText('hello@shop.example.com to second@example.net, third@example.net', { exact: true }).waitFor({ timeout: 3000 });
  await cancellableMessage.getByText('Queued', { exact: true }).waitFor();
  await cancellableMessage.getByRole('button', { name: 'Cancel queued message', exact: true }).click();
  await cancellableMessage.getByText('Cancelled', { exact: true }).waitFor();
  state.jobs[0].state = 'accepted'; await outbox.getByRole('button', { name: 'Refresh outbox', exact: true }).click();
  await outbox.getByText('Accepted by mail service', { exact: true }).waitFor();

  // 3. The own turboSMTP account shows only a key hint and can be removed.
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/mail/${relayID}`);
  await session.page.getByText('…a1b2').waitFor();
  await session.page.getByRole('button', { name: 'Remove', exact: true }).click();
  await session.page.getByRole('button', { name: 'Remove account' }).click();
  await session.page.getByRole('heading', { name: 'Mail', exact: true }).waitFor();
  assert.equal(state.calls.at(-1).action, 'relay.delete', 'nothing reads the removed account again');
  errors.push(...session.errors);
  await session.context.close();

  // 4. A mailbox is created on one of the project's domains and shows its client settings.
  session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + '/workspaces/101/projects/201');
  await session.page.getByText('Add', { exact: true }).click();
  await session.page.getByRole('button', { name: 'Create mailboxes' }).click();
  await session.page.getByRole('combobox', { name: 'Domain' }).waitFor();
  await session.page.getByRole('textbox', { name: 'Address' }).fill('anna');
  await session.page.getByRole('textbox', { name: 'Display name' }).fill('Anna');
  await session.page.getByRole('button', { name: 'Create', exact: true }).click();
  await session.page.getByText('Setting up').waitFor();
  await session.page.getByText('192.168.80.2:1993').waitFor({ timeout: 10000 });
  const created = state.calls.find((call) => call.action === 'mailboxes.create');
  assert.deepEqual(created.input, { domain_id: domainID, local_part: 'anna', name: 'Anna' });
  assert.equal(await session.page.getByText('feedc0de').count(), 0);
  await session.page.getByRole('button', { name: 'Show password' }).click();
  await session.page.getByText('feedc0de').waitFor();
  errors.push(...session.errors);
  await session.context.close();

  // Suspension is customer-wide, explicitly confirmed and polled until observed completion.
  session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/mail/${domainID}`);
  const incoming = session.page.getByRole('region', { name: 'Customer incoming mail', exact: true });
  const suspend = incoming.getByRole('button', { name: 'Suspend incoming mail', exact: true });
  await suspend.waitFor();
  assert.equal(await suspend.isDisabled(), true);
  await incoming.getByRole('checkbox', { name: 'Suspend incoming mail for every project of this customer' }).check();
  await suspend.click();
  await incoming.getByText('Status: suspending', { exact: true }).waitFor();
  state.incoming = 'suspended';
  await incoming.getByText('Status: suspended', { exact: true }).waitFor({ timeout: 7000 });
  assert.ok(state.calls.some((call) => call.action === 'incoming.deactivate' && call.input.resource_id === domainID));
  state.incoming = 'suspending';
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/mail/${mailboxID}`);
  await session.page.getByText('Suspending', { exact: true }).waitFor();
  state.incoming = 'suspended';
  await session.page.getByText('Suspended', { exact: true }).waitFor({ timeout: 7000 });
  errors.push(...session.errors);
  await session.context.close();

  // 5. Members read the domain but cannot send, verify or remove.
  state.jobs[1].state = 'queued';
  session = await open(browser, 'member');
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/mail/${domainID}`);
  await session.page.getByRole('region', { name: 'DNS records' }).waitFor();
  assert.equal(await session.page.getByRole('form', { name: 'Send a message' }).count(), 0);
  assert.equal(await session.page.getByRole('button', { name: 'Remove' }).count(), 0);
  await session.page.getByRole('region', { name: 'Outbox', exact: true }).getByText('Cancel me', { exact: true }).waitFor();
  assert.equal(await session.page.getByRole('button', { name: 'Cancel queued message', exact: true }).count(), 0);
  await session.page.setViewportSize({ width: 390, height: 844 });
  assert.ok(await session.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'no horizontal scroll at phone width');
  errors.push(...session.errors);
  await session.context.close();

  state.jobs[1].state = 'cancelled'; state.legacy = true; state.incoming = 'disabled';
  session = await open(browser, 'owner');
  await session.page.goto(previewOrigin + `/workspaces/101/projects/201/resources/mail/${domainID}`);
  await session.page.getByText('This Mail release needs an update before messages can be queued safely.', { exact: true }).waitFor();
  assert.equal(await session.page.getByRole('form', { name: 'Send a message', exact: true }).count(), 0, 'older inline releases never receive queued submissions');
  assert.equal(await session.page.getByRole('region', { name: 'Outbox', exact: true }).count(), 0);
  errors.push(...session.errors); await session.context.close();

  assert.deepEqual(errors, []);
  console.log('Mail plugin UI checks passed: canonical single/multiple-recipient history arrays, stable retry after lost/403 admission replies, queued versus accepted/unknown, cancellation, legacy release gating, member read-only and retained mailbox controls. Test-only HTTP fixtures.');
} finally { await browser.close(); await server.close(); }
