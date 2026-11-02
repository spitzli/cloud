// Explicit owned Mail acceptance, never part of default browser tests. No intercepted responses.
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { existsSync } from 'node:fs';
import { openFixture, ownEnvironment, custodyIntent, custodyConfirm, expectNamespace, processIdentity, requirePrivateEqual, verifyExecution } from '../../../tools/tests/native-plugin-fixture.mjs';
import { noEngines, metadataSnapshot, engineObjects, runtimeGenerations, mailboxSentinel, observe } from '../../../tools/tests/native-mail-proof.mjs';
import { lifecycle, waitMailReady, assertForeignDenied } from './support/native-plugin-lifecycle.mjs';

export async function mailboxCreateForm(page, domainID) {
  await page.getByText('Add', { exact: true }).click();
  await page.getByRole('list', { name: 'Add a resource', exact: true }).getByRole('button').filter({ has: page.getByText('Create mailboxes', { exact: true }) }).click();
  const form = page.getByRole('form', { name: 'Create mailboxes', exact: true });
  await form.locator('select').selectOption(domainID);
  await form.locator('input[name="local_part"]').fill('anna');
  await form.locator('input[name="name"]').fill('Owned mailbox');
  return form;
}

export async function acceptance(reportPath, loadFixture = openFixture) {
  // Read only the explicitly selected private report to find its frozen input; no discovery/reuse.
  const { readPrivate } = await import('../../../tools/tests/native-plugin-fixture.mjs');
  const report = JSON.parse(await readPrivate(reportPath));
  const f = await loadFixture(report.paths.input, 'acceptance');
  let browser, browserServer, browserCustody, context;
  try {
    await verifyExecution(f);
    assert.equal(reportPath, f.path('report.json')); assert(f.report.completedStages.includes('start'), 'start_stage_required');
    assert(!existsSync(f.path('acceptance-state.json')), 'partial_acceptance_requires_root_reconciliation');
    const { chromium } = await import('playwright');
    browserCustody = await custodyIntent(f.custody, f.saveCustody, 'process', 'browser');
    const previousTmp = process.env.TMPDIR;
    process.env.TMPDIR = f.path('tmp');
    try { browserServer = await chromium.launchServer({ headless: true, host: '127.0.0.1', env: ownEnvironment(f.directory) }); }
    finally { if (previousTmp === undefined) delete process.env.TMPDIR; else process.env.TMPDIR = previousTmp; }
    const pid = browserServer.process().pid;
    await custodyConfirm(browserCustody, f.saveCustody, { pid, ...processIdentity(pid) });
    browser = await chromium.connect(browserServer.wsEndpoint());
    context = await browser.newContext({ viewport: { width: 1280, height: 900 }, acceptDownloads: false });
    const page = await context.newPage(); page.setDefaultTimeout(20000);
    const errors = []; page.on('pageerror', () => errors.push('page_error'));
    const state = { version: 1, phase: 'account' }, save = () => f.write('acceptance-state.json', state, true);
    const ui = lifecycle(f, page, state, save), origin = f.report.origins.console;
    const invoke = (projectID, action, input) => ui.write('/api/console/plugins/invoke', { pluginID: 'mail', projectID, action, input, idempotencyKey: crypto.randomUUID() });
    const resourceURL = (project, resource) => `${origin}/workspaces/${state.customerID}/projects/${project}/resources/mail/${resource}`;
    const waitMailbox = (projectID, mailboxID) => waitMailReady(f,
      () => invoke(projectID, 'mailboxes.view', { resource_id: mailboxID }),
      () => invoke(projectID, 'domains.view', { resource_id: projectID === state.projectID ? state.domainID : state.secondDomainID }));
    await save(); await ui.account(); state.phase = 'install'; await save();
    await ui.install('0.1.0'); await runtimeGenerations(f, 1, '0.1.0'); await noEngines(f);
    for (const id of ['add-domains', 'byok-turbosmtp', 'create-mailboxes', 'manage-incoming']) await ui.feature(id);
    await ui.write('/api/console/clusters/' + f.report.clusterID, { offered: true }, 'PUT');
    const customer = await ui.write('/api/console/registry/customers', { name: 'Owned Mail ' + f.input.runID, customerType: 'company' });
    state.customerID = customer.id; await save(); await ui.owner(customer.id);
    state.projectID = (await ui.get(`/api/console/tenant/${customer.id}/projects`))[0].id;
    const second = await ui.write(`/api/console/tenant/${customer.id}/projects`, { name: 'Second Mail project' }); state.secondProjectID = second.id; await save();
    await ui.write('/api/console/projects/' + state.projectID + '/placement', { clusterId: f.report.clusterID }, 'PUT');
    await ui.write('/api/console/projects/' + second.id + '/placement', { clusterId: f.report.clusterID }, 'PUT');
    const domain = await invoke(state.projectID, 'domains.create', { domain: `a-${f.input.runID}.example.test` }); state.domainID = domain.resource.id;
    const otherDomain = await invoke(second.id, 'domains.create', { domain: `b-${f.input.runID}.example.test` }); state.secondDomainID = otherDomain.resource.id;
    await invoke(state.projectID, 'relay.connect', { consumer_key: 'fixture-only-key-' + f.input.runID, consumer_secret: 'fixture-only-secret-' + f.input.runID });
    await noEngines(f); state.phase = 'explicit-mailbox'; await save();
    const namespace = 'wd-mail-c' + customer.id;
    await expectNamespace(f, namespace);
    const request = await custodyIntent(f.custody, f.saveCustody, 'api-request', 'first-mailbox', { expectedNamespace: namespace });
    await page.goto(`${origin}/workspaces/${customer.id}/projects/${state.projectID}`);
    const form = await mailboxCreateForm(page, state.domainID);
    const created = page.waitForResponse(r => new URL(r.url()).pathname === '/api/console/plugins/invoke' && r.request().postDataJSON()?.action === 'mailboxes.create');
    await form.getByRole('button', { name: 'Create', exact: true }).click(); const reply = await created; assert.equal(reply.status(), 200);
    state.mailboxID = (await reply.json()).resource.id; await custodyConfirm(request, f.saveCustody, { responseID: state.mailboxID }); await save();
    const view = await waitMailbox(state.projectID, state.mailboxID);
    // Same-customer reuse is a real second project invocation, not a fixture database row.
    state.secondMailboxID = (await invoke(second.id, 'mailboxes.create', { domain_id: state.secondDomainID, local_part: 'bob', name: 'Second mailbox' })).resource.id;
    await waitMailbox(second.id, state.secondMailboxID);
    await page.goto(resourceURL(state.projectID, state.mailboxID));
    const secretReply = page.waitForResponse(r => new URL(r.url()).pathname === '/api/console/plugins/invoke' && r.request().postDataJSON()?.action === 'mailboxes.credentials');
    await page.getByRole('button', { name: 'Show password', exact: true }).click(); state.originalPassword = (await (await secretReply).json()).value;
    await page.getByRole('button', { name: 'Hide', exact: true }).click();
    const metadata = await metadataSnapshot(f, customer.id), engine = await engineObjects(f, customer.id);
    const mailbox = metadata.data.mailboxes.find(m => m.id === state.mailboxID); assert(mailbox.account_id);
    state.username = view.username; state.accountID = mailbox.account_id;
    state.sentinel = await mailboxSentinel(f, customer.id, state.username, state.originalPassword, state.accountID);
    state.baseline = { metadata, engine }; await save();
    async function retained(generation, version) {
      await runtimeGenerations(f, generation, version); await ui.owner(customer.id);
      await waitMailbox(state.projectID, state.mailboxID);
      requirePrivateEqual((await invoke(state.projectID, 'mailboxes.credentials', { resource_id: state.mailboxID })).value, state.originalPassword, 'original_mailbox_credential_changed');
      requirePrivateEqual(await metadataSnapshot(f, customer.id), state.baseline.metadata, 'retained_metadata_changed');
      assert.deepEqual(await engineObjects(f, customer.id), state.baseline.engine, 'retained_engine_or_pvc_changed');
      await mailboxSentinel(f, customer.id, state.username, state.originalPassword, state.accountID, state.sentinel);
    }
    state.phase = 'update'; await save(); await ui.change('update', '0.2.0', 2); await retained(2, '0.2.0');
    state.phase = 'rollback'; await save(); await ui.change('rollback', '0.1.0', 3); await retained(3, '0.1.0');
    state.phase = 'suspend'; await save();
    await page.goto(resourceURL(state.projectID, state.domainID));
    const incoming = page.getByRole('region', { name: 'Customer incoming mail', exact: true });
    await incoming.getByRole('checkbox', { name: 'Suspend incoming mail for every project of this customer' }).check();
    await incoming.getByRole('button', { name: 'Suspend incoming mail', exact: true }).click();
    await f.wait(async () => (await invoke(state.projectID, 'domains.view', { resource_id: state.domainID })).incoming.state === 'suspended', 180000);
    const stopped = await f.kubeJSON(['-n', namespace, 'get', 'deployment', 'engine']); assert.equal(stopped.spec.replicas, 0);
    assert.equal((await f.kubeJSON(['-n', namespace, 'get', 'pods'])).items.length, 0);
    assert.deepEqual(await engineObjects(f, customer.id), state.baseline.engine);
    // No fallback to Turbo or SMTP for an incoming reservation, including suspension.
    const deniedSend = await page.request.post(origin + '/api/console/plugins/invoke', { headers: { Origin: origin }, data: { pluginID: 'mail', projectID: state.projectID, action: 'mail.send', input: { resource_id: state.domainID, from: 'anna', to: 'nobody@example.test', subject: 'Must not send', text: 'Blocked' }, idempotencyKey: crypto.randomUUID() } });
    assert.equal(deniedSend.status(), 409, 'incoming_send_must_fail_closed');
    // This activation resumes deliberately suspended incoming; first creation used no extra activation.
    await invoke(second.id, 'incoming.activate', { resource_id: state.secondDomainID }); await retained(3, '0.1.0');
    state.phase = 'foreign-denial'; await save(); await ui.exitOwner();
    const foreign = await ui.write('/api/console/registry/customers', { name: 'Foreign Mail ' + f.input.runID, customerType: 'company' }); state.foreignCustomerID = foreign.id; await save();
    await ui.owner(foreign.id); const foreignProject = (await ui.get(`/api/console/tenant/${foreign.id}/projects`))[0].id;
    const denied = await page.request.post(origin + '/api/console/plugins/invoke', { headers: { Origin: origin }, data: { pluginID: 'mail', projectID: foreignProject, action: 'mailboxes.view', input: { resource_id: state.mailboxID }, idempotencyKey: crypto.randomUUID() } });
    assertForeignDenied(denied.status(), await denied.json(), [state.originalPassword]);
    assert.deepEqual(errors, []); state.phase = 'complete'; state.complete = true; await save();
    await observe(f); f.report.completedStages.push('acceptance'); await f.saveReport();
    process.stdout.write('native_mail_console_acceptance_complete:no_delivery_claim\n');
  } finally {
    try { await context?.close(); await browser?.close(); await browserServer?.close(); if (browserCustody?.pid && !existsSync(`/proc/${browserCustody.pid}`)) { browserCustody.status = 'removed'; await f.saveCustody(); } }
    finally { await f.release(); }
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [flag, path] = process.argv.slice(2);
  if (flag !== '--report' || !path || process.argv.length !== 4) { process.stderr.write('native_mail_acceptance_cli_invalid\n'); process.exitCode = 1; }
  else acceptance(resolve(path)).catch(() => { process.stderr.write('native_mail_console_acceptance_failed; retain private custody for root review\n'); process.exitCode = 1; });
}
