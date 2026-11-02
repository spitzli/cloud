// A real native customer member: invitation, one-use activation, password login and PKCE.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { randomUUID } from 'node:crypto';
import { chmod, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { load } from './native-object-storage-fixture.mjs';
import { setup, client } from './native-object-storage-acceptance.mjs';
import { random, sha256 } from './native-plugin-fixture.mjs';
const f = await load(process.argv[2]); let browser;
try {
  assert(Date.now() < f.report.deadline); await setup(f); const operator = await client(f);
  const account = existsSync(f.path('reader-account-private.json')) ? await f.json(f.path('reader-account-private.json')) : { email: `storage-reader-${f.input.runID}@example.test`, password: 'Owned-' + random() + '!' };
  const { email, password } = account;
  const invite = existsSync(f.path('reader-invitation-private.json')) ? await f.json(f.path('reader-invitation-private.json')) : await operator.json('/api/console/tenant/' + f.report.customer, 'POST', { action: 'invite', email, name: 'Owned storage reader', role: 'member' });
  if (!existsSync(f.path('reader-invitation-private.json'))) await f.write('reader-invitation-private.json', invite); assert(invite.link && invite.id);
  const link = new URL(invite.link); assert.equal(link.origin, f.report.origins.account); const token = link.searchParams.get('token'); assert(token);
  if (!existsSync(f.path('reader-account-private.json'))) await f.write('reader-account-private.json', account);
  browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
  const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, acceptDownloads: true });
  const auth = async (path, data) => { const response = await context.request.post(f.report.origins.account + '/api/auth/' + path, { data, headers: { origin: f.report.origins.account } }); assert.equal(response.status(), 200); return response.json(); };
  const activated = await context.request.post(f.report.origins.account + '/api/auth/reset-password', { data: { token, newPassword: password }, headers: { origin: f.report.origins.account } });
  assert([200, 401].includes(activated.status()));
  const login = await auth('sign-in/email', { email, password }); assert(!login.twoFactorRedirect);
  const invitationResponse = await context.request.get(f.report.origins.account + '/api/invitation/' + invite.id); assert.equal(invitationResponse.status(), 200);
  const invitation = await invitationResponse.json(); assert.equal(invitation.role, 'member');
  let membership = null;
  if (invitation.status === 'pending') {
    const acceptedResponse = await context.request.post(f.report.origins.account + '/api/invitation/' + invite.id, { data: { accept: true }, headers: { origin: f.report.origins.account } });
    const accepted = { status: acceptedResponse.status(), value: await acceptedResponse.json() }; assert.equal(accepted.status, 200); assert.equal(accepted.value.member.role, 'member'); membership = accepted.value.member;
    await f.write('reader-membership-accepted-private.json', accepted);
  } else assert.equal(invitation.status, 'accepted');
  let url = f.report.origins.console + '/api/sso/login?returnTo=/projects'; let reached = false;
  for (let n = 0; n < 12; n++) {
    assert(Object.values(f.report.origins).includes(new URL(url).origin));
    const response = await context.request.get(url, { maxRedirects: 0 });
    if (response.status() >= 300 && response.status() < 400) { url = new URL(response.headers().location, url).href; continue; }
    if (response.headers()['content-type']?.includes('application/json')) { const value = await response.json(); if (value.redirect && value.url) { url = new URL(value.url, url).href; continue; } }
    assert.equal(response.status(), 200); assert.equal(new URL(url).origin, f.report.origins.console); reached = true; break;
  }
  assert(reached);
  const post = async (path, data) => { const response = await context.request.post(f.report.origins.console + path, { data, headers: { origin: f.report.origins.console } }); return { status: response.status(), value: await response.json() }; };
  const session = await (await context.request.get(f.report.origins.console + '/api/console/session')).json();
  assert.equal(session.data.user.role, 'user'); assert(!session.data.user.preview && !session.data.user.acting);
  const projectResponse = await context.request.get(f.report.origins.console + '/api/console/project/' + f.report.project); assert.equal(projectResponse.status(), 200);
  const project = await projectResponse.json(); assert.equal(project.role, 'member');
  const viewsResponse = await context.request.get(f.report.origins.console + '/api/console/plugins?projectID=' + f.report.project); assert.equal(viewsResponse.status(), 200);
  const views = await viewsResponse.json(); assert(views.some(v => v.manifest.name === 'object-storage' && v.resources.some(r => r.id === operator.state.resource)));
  const page = await context.newPage(); page.setDefaultTimeout(15000); const errors = []; page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${f.report.origins.console}/workspaces/${f.report.customer}/projects/${f.report.project}/resources/object-storage/${operator.state.resource}`);
  await page.getByRole('heading', { name: 'active', exact: true }).waitFor(); assert.equal(await page.getByRole('button', { name: 'Upload object', exact: true }).count(), 0);
  await page.getByLabel('Download object key', { exact: true }).fill('browser/proof.txt'); await page.getByRole('button', { name: 'Inspect object', exact: true }).click();
  await page.getByRole('button', { name: 'Download observed object', exact: true }).waitFor();
  const replacement = Buffer.from('controlled application replacement\n');
  await page.getByLabel('Expected SHA-256 (optional)', { exact: true }).fill(sha256(replacement));
  await page.getByRole('button', { name: 'Download observed object', exact: true }).click(); const save = page.getByRole('link', { name: 'Save verified file', exact: true }); await save.waitFor();
  const downloading = page.waitForEvent('download'); await save.click(); const download = await downloading, path = f.path('reader-downloaded.bin'); await download.saveAs(path); await chmod(path, 0o600); assert.equal(sha256(await readFile(path)), sha256(replacement));
  const denied = await post('/api/console/plugins/invoke', { pluginID: 'object-storage', projectID: f.report.project, action: 'objects.upload', input: { resource_id: operator.state.resource, key: 'member-denied.txt', exact_bytes: 3, expected_sha256: 'a'.repeat(64), filename: 'proof.txt', content_type: 'text/plain', expires_at: Math.floor(Date.now() / 1000) + 120 }, idempotencyKey: randomUUID() }); assert.equal(denied.status, 403);
  assert.equal(errors.length, 0);
  await f.write('reader-private-proof.json', { userID: session.data.user.id, membershipID: membership?.id ?? null, invitationID: invite.id, invitationAccepted: true, projectRole: project.role, writeStatus: denied.status });
  f.report.projectReaderBrowserProof = { supportedInvitationActivationAndPkce: true, realNativeMembership: true, memberRole: 'member', projectRole: 'member', principalIsPreview: false, pluginEntitlementVerified: true, uploadControlAbsent: true, uploadWriteDenied: true, actualInspectedObjectDownload: true, knownShaVerifiedBeforeSave: true, noForgedRows: true, pageErrors: 0 };
  f.report.binaryBrowserProof.projectReaderMembershipQualified = true; await f.saveReport(); console.log('native_object_storage_project_reader_browser_passed ' + f.input.runID);
} catch (error) { await f.write('reader-browser-failure-' + Date.now() + '.json', { error: String(error), stack: error.stack }); console.error('native_object_storage_reader_browser_failed_private_diagnostics_retained'); process.exitCode = 1; }
finally { await browser?.close(); }
