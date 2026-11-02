// Opt-in acceptance against a native-CLI-created platform and an owned enrolled local cluster.
// The report references private files; credentials and one-time codes never enter output/screenshots.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, chmod } from 'node:fs/promises';
import { randomBytes, createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { chromium } from 'playwright';
import { createOTP } from '@better-auth/utils/otp';
import { base32 } from '@better-auth/utils/base32';
import { fromRepo } from './repo.mjs';

const reportPath = process.env.WEBDOCK_NATIVE_INSTALL_FIXTURE;
assert.ok(reportPath && ['.runtime-test/native-installer/', '.runtime-test/native-plugin-updates/'].some((base) => reportPath.startsWith(fromRepo(base).pathname)), 'An owned native installer report is required');
const readJSON = async (path) => JSON.parse(await readFile(path, 'utf8'));
const report = await readJSON(reportPath);
assert.equal(report.fixture, true);
const paths = report.paths, directory = paths.directory;
const core = await readJSON(paths.coreEnvironment);
const operator = await readJSON(paths.operatorInput);
const catalog = await readJSON(paths.catalog);
const manifest = JSON.parse(catalog.releases[0].manifestRaw);
const consoleOrigin = core.WEBDOCK_CONSOLE_ORIGIN, authOrigin = core.WEBDOCK_AUTH_ORIGIN;
assert.equal(consoleOrigin, 'http://127.0.0.1:6121');
assert.equal(authOrigin, 'http://localhost:6125');
const statePath = directory + '/acceptance-state.json', cookiePath = directory + '/acceptance-cookies.json';
let state;
try { state = await readJSON(statePath); } catch (error) { if (error.code !== 'ENOENT') throw error; state = { replacement: 'Owned-' + randomBytes(24).toString('hex') + '!', phase: 'account' }; }
const save = async () => writeFile(statePath, JSON.stringify(state), { mode: 0o600 });
const privateValues = [operator.password, state.replacement, state.totpSecret];
await save();
let storageState;
try { storageState = await readJSON(cookiePath); } catch (error) { if (error.code !== 'ENOENT') throw error; }
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, ...(storageState ? { storageState } : {}) });
const page = await context.newPage(); page.setDefaultTimeout(15000);
const errors = []; page.on('pageerror', (error) => errors.push(error.message));
const matchAction = (action) => (response) => {
  if (new URL(response.url()).pathname !== '/api/console/plugins/invoke' || response.request().method() !== 'POST') return false;
  return response.request().postDataJSON().action === action;
};
const post = async (path, data) => {
  const response = await page.request.post(consoleOrigin + path, { data, headers: { Origin: consoleOrigin } });
  assert.equal(response.status(), 200, `Native request rejected: ${path}`);
  return response.json();
};
async function account() {
  const existing = await page.request.get(consoleOrigin + '/api/console/session');
  if (existing.ok()) {
    const session = (await existing.json()).data;
    if (session?.user.preview) {
      assert.equal(session.user.email, operator.email, 'only the owned operator scope may be restored');
      await post('/api/console/tenant-preview/exit', {});
    }
  }
  await page.goto(consoleOrigin + '/extensions');
  if (new URL(page.url()).origin !== authOrigin) {
    await Promise.race([page.getByRole('heading', { name: 'Installed extensions', exact: true }).waitFor(), page.waitForURL(authOrigin + '/**')]);
    if (new URL(page.url()).origin !== authOrigin) return;
  }
  await page.getByRole('heading', { name: 'Sign in to Webdock', exact: true }).waitFor();
  await page.getByRole('textbox', { name: 'Email', exact: true }).fill(operator.email);
  await page.getByLabel('Password', { exact: true }).fill(state.passwordChanged ? state.replacement : operator.password);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  if (state.totpEnabled) {
    await page.getByRole('heading', { name: 'Verify your identity', exact: true }).waitFor();
    const otp = createOTP(new TextDecoder().decode(base32.decode(state.totpSecret)));
    await page.getByLabel('Authentication code', { exact: true }).fill(await otp.totp());
    await page.getByRole('button', { name: 'Verify code', exact: true }).click();
  }
  if (!state.passwordChanged) {
    await page.getByRole('heading', { name: 'Account security', exact: true }).waitFor();
    await page.getByLabel('Current password', { exact: true }).fill(operator.password);
    await page.getByLabel('New password', { exact: true }).fill(state.replacement);
    await page.getByLabel('Confirm new password', { exact: true }).fill(state.replacement);
    await page.getByRole('button', { name: 'Update password', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Password updated.' }).waitFor();
    state.passwordChanged = true; await save();
  }
  if (!state.totpEnabled) {
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
    state.totpEnabled = true; await save();
  }
  if (new URL(page.url()).origin === authOrigin) {
    const allow = page.getByRole('button', { name: 'Allow access', exact: true });
    await allow.waitFor(); await allow.click();
  }
  await page.waitForURL(consoleOrigin + '/**');
  await writeFile(cookiePath, JSON.stringify(await context.storageState()), { mode: 0o600 });
}

try {
  await account();
  state.phase = 'installation'; await save();
  await page.goto(consoleOrigin + '/extensions');
  if (!state.operationID) {
    const before = await page.request.get(consoleOrigin + '/api/console/registry/customers');
    assert.equal((await before.json()).totalDocs, 0, 'installation must not need dummy customer/project');
    await page.getByRole('link', { name: `Review postgres ${manifest.version}`, exact: true }).click();
    await page.getByRole('heading', { name: 'Install postgres', exact: true }).waitFor();
    await page.getByRole('checkbox', { name: 'I reviewed this release, its permissions and the selected target.' }).check();
    const reply = page.waitForResponse((r) => new URL(r.url()).pathname === '/api/console/plugins/install' && r.request().method() === 'POST');
    await page.getByRole('button', { name: 'Install extension', exact: true }).click();
    const accepted = await reply; assert.equal(accepted.status(), 200, 'native install admission');
    state.operationID = (await accepted.json()).id; await save();
  }
  await page.goto(consoleOrigin + '/extensions/operations/' + state.operationID);
  const operation = await (await page.request.get(consoleOrigin + '/api/console/plugins/operations/' + state.operationID)).json();
  if (['blocked', 'failed'].includes(operation.status)) {
    assert.equal(process.env.WEBDOCK_NATIVE_INSTALL_RETRY, 'true', `Installation requires explicit retry: ${operation.condition}`);
    const retried = page.waitForResponse((r) => new URL(r.url()).pathname.endsWith('/retry') && r.request().method() === 'POST');
    await page.getByRole('button', { name: 'Retry installation', exact: true }).click();
    const response = await retried; assert.equal(response.status(), 200);
    state.operationID = (await response.json()).id; await save();
    await page.waitForURL(consoleOrigin + '/extensions/operations/' + state.operationID);
  }
  const deadline = Date.now() + 240000;
  let finished;
  do {
    finished = await (await page.request.get(consoleOrigin + '/api/console/plugins/operations/' + state.operationID)).json();
    if (['succeeded', 'blocked', 'failed'].includes(finished.status)) break;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  } while (Date.now() < deadline);
  assert.equal(finished.status, 'succeeded', `Installation stopped: ${finished.condition ?? finished.status}`);
  await page.getByText('succeeded', { exact: true }).waitFor();
  await page.getByRole('link', { name: 'Configure self-service', exact: true }).click();
  const feature = manifest.selfService.features.find((item) => item.id === 'create-dedicated');
  const row = page.getByRole('list', { name: 'Self-service', exact: true }).getByRole('listitem').filter({ has: page.getByRole('heading', { name: feature.label, exact: true }) });
  const select = row.getByRole('combobox'); await select.waitFor();
  if (await select.inputValue() !== 'everyone') {
    const changed = page.waitForResponse((r) => new URL(r.url()).pathname.endsWith('/features/create-dedicated') && r.request().method() === 'PUT');
    await select.selectOption('everyone'); assert.equal((await changed).status(), 200);
  }
  const shared = manifest.selfService.features.find((item) => item.id === 'create-shared');
  assert.equal(await page.getByRole('listitem').filter({ has: page.getByRole('heading', { name: shared.label, exact: true }) }).getByRole('combobox').isDisabled(), true);
  console.log('Native CLI operator MFA, signed Console installation and unsupported-offering ceiling passed.');
  state.phase = 'resource'; await save();
  if (!state.customerID) {
    const customer = await post('/api/console/registry/customers', { name: 'Native installer acceptance', customerType: 'company' });
    state.customerID = customer.id; await save();
  }
  const current = (await (await page.request.get(consoleOrigin + '/api/console/session')).json()).data;
  if (!current.user.acting) {
    await page.goto(consoleOrigin + '/customers/' + state.customerID);
    await page.getByRole('button', { name: 'Act as customer owner', exact: true }).click();
    await page.waitForURL(consoleOrigin + '/workspaces/' + state.customerID);
  }
  const projects = await (await page.request.get(consoleOrigin + '/api/console/tenant/' + state.customerID + '/projects')).json();
  state.projectID ??= projects[0].id; await save();
  if (!state.resourceID) {
    await page.goto(consoleOrigin + `/workspaces/${state.customerID}/projects/${state.projectID}`);
    await page.getByText('Add', { exact: true }).click();
    await page.getByRole('list', { name: 'Add a resource' }).getByRole('button').filter({ has: page.getByText(feature.label, { exact: true }) }).click();
    await page.getByRole('textbox', { name: 'Name', exact: true }).fill('Installer database');
    await page.getByRole('combobox', { name: 'PostgreSQL version', exact: true }).selectOption('17');
    const created = page.waitForResponse(matchAction('instances.create'));
    await page.getByRole('button', { name: 'Create', exact: true }).click();
    const response = await created; assert.equal(response.status(), 200);
    state.resourceID = (await response.json()).resource.id; await save();
  }
  const resourceURL = consoleOrigin + `/workspaces/${state.customerID}/projects/${state.projectID}/resources/postgres/${state.resourceID}`;
  await page.goto(resourceURL);
  await page.getByText('Ready', { exact: true }).waitFor({ timeout: 240000 });
  await mkdir(directory + '/screens', { recursive: true, mode: 0o700 });
  for (const width of [375, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    await page.screenshot({ path: directory + `/screens/resource-${width}.png`, fullPage: true });
    await chmod(directory + `/screens/resource-${width}.png`, 0o600);
  }
  const view = await post('/api/console/plugins/invoke', { pluginID: 'postgres', projectID: state.projectID, action: 'databases.view', input: { resource_id: state.resourceID }, idempotencyKey: crypto.randomUUID() });
  const secretReply = page.waitForResponse(matchAction('databases.credentials'));
  await page.getByRole('button', { name: 'Show password', exact: true }).click();
  const secret = await (await secretReply).json(); assert.ok(typeof secret.value === 'string');
  privateValues.push(secret.value, secret.uri);
  await page.getByRole('button', { name: 'Hide', exact: true }).click();
  state.phase = 'sql'; await save();
  const { Client } = await import('pg');
  const ca = await readFile(process.env.WEBDOCK_NATIVE_DATABASE_CA ?? fromRepo('.dev-local/ca/ca.crt'));
  const sql = async (query) => {
    const client = new Client({ host: '127.0.0.1', port: view.port, database: view.database, user: view.user, password: secret.value, connectionTimeoutMillis:10000,query_timeout:10000, ssl: { ca, servername: view.host, rejectUnauthorized: true } });
    await client.connect(); try { return await client.query(query); } finally { await client.end(); }
  };
  await sql('CREATE TABLE IF NOT EXISTS native_installer_acceptance(value integer PRIMARY KEY)');
  await sql('INSERT INTO native_installer_acceptance VALUES(42) ON CONFLICT DO NOTHING');
  assert.equal((await sql('SELECT value FROM native_installer_acceptance')).rows[0].value, 42);
  state.phase = 'retention'; await save();
  const identity = catalog.platformID, plugin = 'postgres';
  const namespace = 'wd-plugin-' + createHash('sha256').update(`${identity.length}:${identity}${plugin.length}:${plugin}`).digest('hex').slice(0, 24);
  const kube = ['--kubeconfig', paths.kubeconfig, '--context', 'webdock-dev', '-n', namespace];
  const pods = spawnSync('kubectl', [...kube, 'get', 'pods', '-o', 'json'], { encoding: 'utf8' });
  assert.equal(pods.status, 0);
  const running = JSON.parse(pods.stdout).items.filter((pod) => pod.metadata.ownerReferences?.some((owner) => owner.kind === 'ReplicaSet'));
  assert.equal(running.length, 1, 'one owned installation runtime');
  const pod = running[0];
  const restart = spawnSync('kubectl', ['--kubeconfig', paths.kubeconfig, '--context', 'webdock-dev', 'delete', '--raw', `/api/v1/namespaces/${namespace}/pods/${pod.metadata.name}`, '-f', '-'], {
    input: JSON.stringify({ apiVersion: 'v1', kind: 'DeleteOptions', preconditions: { uid: pod.metadata.uid, resourceVersion: pod.metadata.resourceVersion } }), encoding: 'utf8',
  });
  assert.equal(restart.status, 0, 'replace the owned pod without changing its approved deployment');
  const replacement = spawnSync('kubectl', [...kube, 'wait', '--for=delete', `pod/${pod.metadata.name}`, '--timeout=60s'], { encoding: 'utf8' });
  assert.equal(replacement.status, 0);
  const ready = spawnSync('kubectl', [...kube, 'rollout', 'status', 'deployment/runtime', '--timeout=120s'], { encoding: 'utf8' }); assert.equal(ready.status, 0, 'owned runtime replacement convergence');
  await page.reload(); await page.getByText('Ready', { exact: true }).waitFor();
  const retainedReply = page.waitForResponse(matchAction('databases.credentials'));
  await page.getByRole('button', { name: 'Show password', exact: true }).click();
  assert.equal((await (await retainedReply).json()).value, secret.value, 'retained metadata and sealing key');
  await page.getByRole('button', { name: 'Hide', exact: true }).click();
  assert.equal((await sql('SELECT value FROM native_installer_acceptance')).rows[0].value, 42);
  assert.deepEqual(errors, []);
  state.phase = 'complete'; await save();
  await writeFile(cookiePath, JSON.stringify(await context.storageState()), { mode: 0o600 });
  console.log('Native fresh Console install passed: signed actual OCI, enrolled agent, private storage, customer-owner PostgreSQL SQL write/read and retained runtime replacement.');
} catch (error) {
  let message = String(error.message).split('\n')[0];
  for (const value of privateValues) if (value) message = message.replaceAll(value, '[redacted]');
  console.error(`Native installer acceptance failed at ${state.phase}: ${message}`);
  process.exitCode = 1;
} finally {
  await writeFile(cookiePath, JSON.stringify(await context.storageState()), { mode: 0o600 });
  await context.close(); await browser.close();
}
