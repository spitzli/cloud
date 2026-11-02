// Owned controlled HTTP fixture: real Console components, no fake Native/Core readiness claim.
import { chromium } from "playwright";
import { createServer } from "vite";
import react from "@vitejs/plugin-react";
import tailwind from "@tailwindcss/vite";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, writeFile, mkdtemp, rm, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { databaseCommandSchema } from "../../../packages/hosting-contracts/src/databases.ts";
import { database as initial, history as initialHistory, session as initialSession } from "./support/database-backup-fixture.ts";
const proofDatabase = { ...initial, projectID: "20" };
const proofHistory = { ...initialHistory, policy: { ...initialHistory.policy, spec: { ...initialHistory.policy.spec, projectID: "20", destination: { ...initialHistory.policy.spec.destination, archivePath: "postgresql/10/20/6/5" } } } };
const output = await mkdtemp(`${tmpdir()}/webdock-pg-console-http-`), cacheDir = `${output}/vite-cache`;
let database = structuredClone(proofDatabase), history = structuredClone(proofHistory), nextID = 20, failNext = 0;
const receipts = new Map(), commands = [];
let denyHistory = false, denyPermissions = false, readonly = false, dropRegistration = false, registrationMutations = 0;
let readCounts = {}, registrationAttempts = [], detailStatus = 0, frozenViews = null, historyDelayMs = 0, sessionRole = "operator", wrongRegistrationReply = false;
const reviewEvidence = [];
const server = await createServer({ configFile: false, root: fileURLToPath(new URL("../", import.meta.url)), cacheDir, plugins: [react(), tailwind(), { name: "owned-backup-http-proof", configureServer(server) { server.middlewares.use((req, res, next) => { void serve(req, res, next).catch(() => { res.statusCode = 500; res.end(); }); }); } }], optimizeDeps: { entries: ["tests/database-backup-proof.tsx"], noDiscovery: true, include: ["react", "react-dom/client", "@tanstack/react-query", "zod", "tailwind-merge", "@neon/realtime/client"] }, resolve: { alias: { "@webdock/plugin-ui": fileURLToPath(new URL("../src/plugins/sdk.ts", import.meta.url)), "@plugins": fileURLToPath(new URL("../../../plugins", import.meta.url)) }, dedupe: ["react", "react-dom", "@tanstack/react-query", "zod"] }, server: { host: "127.0.0.1", port: 0, strictPort: true } });
async function body(req) { const chunks = []; for await (const part of req) chunks.push(part); return JSON.parse(Buffer.concat(chunks).toString()); }
async function serve(req, res, next) {
  const send = value => { res.setHeader("Content-Type", "application/json"); res.setHeader("Cache-Control", "no-store"); res.end(JSON.stringify({ data: value })); };
  if (req.url === "/__backup-proof") { res.setHeader("Content-Type", "text/html"); res.end(await server.transformIndexHtml(req.url, '<!doctype html><html lang="en"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/tests/database-backup-proof.tsx"></script></body></html>')); return; }
  if (req.url === "/__backup-proof-control") {
    const input = await body(req);
    if (input.reset) { database = structuredClone(proofDatabase); history = structuredClone(proofHistory); receipts.clear(); commands.length = 0; denyHistory = false; denyPermissions = false; readonly = false; dropRegistration = false; registrationMutations = 0; readCounts = {}; registrationAttempts = []; detailStatus = 0; frozenViews = null; historyDelayMs = 0; sessionRole = "operator"; wrongRegistrationReply = false; }
    if (input.noPolicy) { history = { policy: null, backups: [] }; database.backup = { status: "not-configured", observedControls: null, lastSuccessAt: null, recoverableUntil: null, lastRestoreTestAt: null, publicRecoveryAvailable: false, disabledReason: "production-qualification-required", rpoSeconds: null, rtoSeconds: null }; }
    if (input.sessionRole) sessionRole = input.sessionRole;
    if (input.wrongRegistrationReply) wrongRegistrationReply = true;
    if (input.freezeViews) frozenViews = { database: structuredClone(database), history: structuredClone(history) };
    if (typeof input.detailStatus === "number") { assert([0,403,503].includes(input.detailStatus)); detailStatus = input.detailStatus; }
    if (input.historyDelayMs) historyDelayMs = 500;
    if (input.observe && history.policy) {
      frozenViews = null;
      const p = history.policy; p.observedRevision = p.revision; p.observedSuspended = p.spec.suspended; p.observedAt = new Date().toISOString(); p.revocationEffective = p.revokedAt !== null;
      p.observedControls = { ...proofHistory.policy.observedControls, schedule: p.spec.schedule, walEnabled: p.spec.walEnabled, retentionDelete: p.spec.retentionDelete, retentionDays: p.spec.retentionDays };
      database.backup = { ...database.backup, status: p.spec.suspended ? "suspended" : "configured", policyID: p.id, revision: p.revision, observedRevision: p.revision, walEnabled: p.spec.walEnabled, retentionDelete: p.spec.retentionDelete, observedControls: p.observedControls };
    }
    if (input.invalidateObservation && history.policy) history.policy.observedAt = null;
    if (typeof input.denyHistory === "boolean") denyHistory = input.denyHistory;
    if (typeof input.denyPermissions === "boolean") denyPermissions = input.denyPermissions;
    if (typeof input.readonly === "boolean") readonly = input.readonly;
    if (input.dropRegistration) dropRegistration = true;
    failNext = input.failNext ?? 0; send({ ok: true, reads: readCounts, registrationMutations, registrationAttempts }); return;
  }
  if (req.url === "/v1/auth/session") { send({ ...initialSession, user: { ...initialSession.user, role: sessionRole === "preview" ? "operator" : sessionRole, preview: sessionRole === "preview" } }); return; }
  if (req.url !== "/api/console/databases") { next(); return; }
  const input = await body(req);
  if (input.action === "databases.capacity") { send([]); return; }
  const cmd = databaseCommandSchema.parse(input);
  if (cmd.action === "databases.backupPolicies.get" && historyDelayMs) { const delay = historyDelayMs; historyDelayMs = 0; await new Promise(resolve => setTimeout(resolve, delay)); }
  const shownDatabase = frozenViews?.database ?? database, shownHistory = frozenViews?.history ?? history;
  const reads = { "databases.permissions": { operator: !readonly, canWrite: !readonly }, "databases.plans.list": [], "databases.list": cmd.projectID === shownDatabase.projectID ? [shownDatabase] : [], "databases.get": shownDatabase, "databases.backupPolicies.get": shownHistory };
  if (cmd.action in reads) {
    readCounts[cmd.action] = (readCounts[cmd.action] ?? 0) + 1;
    if (cmd.action === "databases.get" && detailStatus) { res.statusCode = detailStatus; send({ error: "controlled detail failure" }); return; }
    if (cmd.action === "databases.permissions" && denyPermissions || cmd.action === "databases.backupPolicies.get" && denyHistory) { res.statusCode = 403; send({ error: "controlled read denial" }); return; }
    send(reads[cmd.action]); return;
  }
  if (failNext) { res.statusCode = failNext; failNext = 0; send({ error: "controlled rejection" }); return; }
  // Fixture recorder excludes credential-bearing registration input entirely.
  if (cmd.action !== "databases.backupDestinations.register") commands.push(cmd);
  if (cmd.action === "databases.backupDestinations.register") registrationAttempts.push({ key: cmd.idempotencyKey, sha256: createHash("sha256").update(JSON.stringify(cmd)).digest("hex") });
  if (receipts.has(cmd.idempotencyKey)) { if (cmd.action === "databases.backupDestinations.register" && wrongRegistrationReply) { wrongRegistrationReply = false; send({ policyID: "5", revision: 2, operationID: "99", revocationEffective: false }); return; } send(receipts.get(cmd.idempotencyKey)); return; }
  if (cmd.action === "databases.backupDestinations.register") { registrationMutations++; const result = { destinationID: String(nextID++) }; receipts.set(cmd.idempotencyKey, result); if (dropRegistration) { dropRegistration = false; res.statusCode = 502; send({ error: "controlled committed write with lost receipt" }); return; } send(result); return; }
  if (cmd.action === "databases.backupPolicies.grant") history.policy = { ...structuredClone(proofHistory.policy), revision: 1, observedRevision: 0, observedSuspended: null, observedAt: null, observedControls: null, spec: { ...structuredClone(proofHistory.policy.spec), revision: 1, destination: { ...proofHistory.policy.spec.destination, id: cmd.destinationID }, schedule: cmd.schedule, retentionDays: cmd.retentionDays, retentionDelete: cmd.retentionDelete } };
  if (cmd.action === "databases.backupPolicies.update") { history.policy.revision++; Object.assign(history.policy.spec, { revision: history.policy.revision, schedule: cmd.schedule, suspended: cmd.suspended, walEnabled: cmd.walEnabled, retentionDays: cmd.retentionDays, retentionDelete: cmd.retentionDelete }); }
  if (cmd.action === "databases.backupPolicies.revoke") { history.policy.revision++; history.policy.spec.revision = history.policy.revision; history.policy.spec.suspended = true; history.policy.revokedAt = new Date().toISOString(); }
  assert(history.policy); database.backup = { ...database.backup, status: history.policy.revokedAt ? "suspension-pending" : "configuration-pending", policyID: history.policy.id, revision: history.policy.revision };
  const result = { policyID: history.policy.id, revision: history.policy.revision, operationID: String(nextID++), revocationEffective: false }; receipts.set(cmd.idempotencyKey, result); send(result);
}
let browser, lastPage;
try {
  await server.listen(); const address = server.httpServer.address(); assert(address && typeof address !== "string"); const origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch({ headless: true }); const context = await browser.newContext(); const page = await context.newPage(); lastPage = page; page.setDefaultTimeout(10000); const failures = []; page.on("pageerror", e => { failures.push(e.message); });
  const control = async input => { const response = await context.request.post(`${origin}/__backup-proof-control`, { data: input }); assert(response.ok()); return (await response.json()).data; };
  const openResource = async () => { await page.getByRole("combobox", { name: "Project", exact: true }).selectOption("20"); await page.getByRole("button", { name: "Manage database", exact: true }).click(); };
  const open = async () => { await page.goto(`${origin}/__backup-proof`); await page.waitForLoadState("networkidle"); await openResource(); await page.getByText("Backup configuration pending observation", { exact: true }).waitFor(); await page.getByText("Backup configuration observed", { exact: true }).waitFor(); };
  const refresh = async () => { await page.getByRole("button", { name: "Refresh backup status", exact: true }).click(); };
  const changeActor = async role => { await page.getByLabel("Fixture actor", { exact: true }).selectOption(role); await page.waitForFunction(value => document.querySelector("[data-fixture-role]")?.textContent === value, role); };
  const breakDetail = async status => {
    await control({ detailStatus: status });
    // An already-dispatched detail read can enter the error state before the click.
    await page.evaluate(() => { [...document.querySelectorAll("button")].find(button => button.textContent === "Refresh backup status")?.click(); });
    await page.locator(".database-backups").waitFor({ state: "detached" });
  };
  for (const width of [375, 1280]) {
    await control({ reset: true, historyDelayMs: true }); await page.setViewportSize({ width, height: 1000 }); await open();
    const section = page.getByRole("region", { name: "Backups", exact: true }); assert(await section.getByRole("button", { name: "Restore database", exact: true }).isDisabled());
    const manualBefore = commands.filter(c => c.action === "databases.backup").length;
    await page.evaluate(() => {
      const original = crypto.subtle.digest.bind(crypto.subtle); let held = false;
      window.digestHeld = false; window.restoreDigest = () => { crypto.subtle.digest = original; };
      crypto.subtle.digest = async (algorithm, data) => { if (!held) { held = true; window.digestHeld = true; await new Promise(resolve => { window.releaseDigest = resolve; }); } return original(algorithm, data); };
    });
    await section.getByRole("button", { name: "Back up now", exact: true }).click(); await page.waitForFunction(() => window.digestHeld === true);
    await control({ invalidateObservation: true }); await refresh(); await section.getByText("Backup configuration pending observation", { exact: true }).waitFor(); assert(await section.getByRole("button", { name: "Back up now", exact: true }).isDisabled());
    await page.evaluate(() => { window.releaseDigest(); window.restoreDigest(); }); await section.getByText("Check the backup request fields. Refresh before trying again.", { exact: true }).waitFor(); assert.equal(commands.filter(c => c.action === "databases.backup").length, manualBefore);
    await control({ observe: true }); await refresh(); await section.getByText("Backup configuration observed", { exact: true }).waitFor();
    await section.getByRole("button", { name: "Back up now", exact: true }).click(); await section.getByText("Accepted operation", { exact: false }).waitFor(); await control({ observe: true }); await refresh();
    await control({ invalidateObservation: true }); await refresh(); await section.getByText("Backup configuration pending observation", { exact: true }).waitFor(); assert(await section.getByRole("button", { name: "Back up now", exact: true }).isDisabled()); await control({ observe: true }); await refresh(); await section.getByText("Backup configuration observed", { exact: true }).waitFor();
    await control({ freezeViews: true });
    const settings = section.getByRole("form", { name: "Backup policy settings", exact: true }); await settings.getByLabel("Suspend scheduled backups", { exact: true }).check(); await settings.getByLabel("Allow WAL archiving", { exact: true }).uncheck(); await settings.getByLabel("Allow deletion under the retention policy", { exact: true }).check(); await settings.getByRole("button", { name: "Apply backup policy", exact: true }).click(); await section.getByText("Previous observations — not current confirmation", { exact: true }).waitFor();
    assert.equal(commands.at(-1).walEnabled, false); assert.equal(commands.at(-1).retentionDelete, true);
    for (const status of [503,403]) {
      await breakDetail(status); await control({ detailStatus: 0 }); await page.getByRole("button", { name: "Try again", exact: true }).click(); await section.getByText("Backup request accepted. Waiting for agent observation.", { exact: true }).waitFor(); assert(await section.getByRole("button", { name: "Back up now", exact: true }).isDisabled());
    }
    for (const role of ["member", "operator"]) { await changeActor(role); await openResource(); }
    await section.getByText("Backup request accepted. Waiting for agent observation.", { exact: true }).waitFor(); assert(await section.getByRole("button", { name: "Back up now", exact: true }).isDisabled());
    await control({ observe: true }); await refresh(); await section.getByText("Backup schedule suspension observed", { exact: true }).waitFor();
    page.once("dialog", d => { void d.accept(); }); await section.getByRole("button", { name: "Revoke backup grant", exact: true }).click(); await section.getByText("Backup grant revocation is pending observation. WAL and retention permissions remain separate.", { exact: true }).waitFor(); await control({ observe: true }); await refresh(); await section.getByText("Backup grant revocation observed. WAL and retention permissions remain separate.", { exact: true }).waitFor();
    const update = commands.find(c => c.action === "databases.backupPolicies.update"); assert.equal(update.revision, 4); assert.equal(update.policyRevision, 2);
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)); await page.screenshot({ path: `${output}/backup-${width}.png`, fullPage: true });
    await changeActor("member"); await openResource(); assert.equal(await page.getByRole("form", { name: "Register backup destination", exact: true }).count(), 0); assert(await section.getByRole("button", { name: "Restore database", exact: true }).isDisabled());
    await changeActor("preview"); await openResource(); assert.equal(await page.getByRole("form", { name: "Backup policy settings", exact: true }).count(), 0);
    await control({ reset: true, noPolicy: true }); await page.goto(`${origin}/__backup-proof`); await openResource(); await section.getByText("Backups are not configured", { exact: true }).waitFor();
    await section.getByText("Register backup destination", { exact: true }).click(); const registration = section.locator('form[aria-label="Register backup destination"]');
    const fillRegistration = async () => { const details = section.locator("details"); if (await details.getAttribute("open") === null) await details.locator("summary").click(); for (const [label, value] of Object.entries({ "HTTPS S3 endpoint": "https://s3.example.test", "S3 region": "test", "Private bucket": "owned-proof", "Retained recovery key reference": "11", "Writer access key": "fixture-writer-key", "Writer secret key": "fixture-writer-secret-1234", "Recovery access key": "fixture-reader-key", "Recovery secret key": "fixture-reader-secret-1234" })) await registration.getByLabel(label, { exact: true }).fill(value); };
    await fillRegistration(); await control({ dropRegistration: true }); await registration.getByRole("button", { name: "Register destination", exact: true }).click(); await section.getByText("The response was lost.", { exact: false }).waitFor();
    await fillRegistration(); await registration.getByLabel("HTTPS S3 endpoint", { exact: true }).fill("http://s3.example.test"); await registration.getByRole("button", { name: "Re-enter identical credentials and retry registration", exact: true }).click(); await refresh();
    for (const status of [503,403]) {
      await breakDetail(status); await control({ detailStatus: 0 }); await page.getByRole("button", { name: "Try again", exact: true }).click(); await registration.waitFor({ state: "attached" });
    }
    await page.getByRole("button", { name: "Close", exact: true }).click(); await section.waitFor({ state: "detached" }); await openResource(); await registration.waitFor({ state: "attached" });
    for (const role of ["member", "operator"]) { await changeActor(role); await openResource(); }
    await registration.waitFor({ state: "attached" }); await control({ wrongRegistrationReply: true }); await fillRegistration(); await registration.getByRole("button", { name: "Re-enter identical credentials and retry registration", exact: true }).click(); await section.getByText("The response was lost.", { exact: false }).waitFor();
    await fillRegistration(); await registration.getByRole("button", { name: "Re-enter identical credentials and retry registration", exact: true }).click(); const destination = section.getByRole("status").filter({ hasText: "Registered destination ID:" }); await destination.waitFor(); const replay = await control({ inspect: true }); assert.equal(replay.registrationMutations, 1); assert.equal(replay.registrationAttempts.length, 3); for (const attempt of replay.registrationAttempts) assert.deepEqual(attempt, replay.registrationAttempts[0]);
    const destinationID = await destination.locator("code").innerText();
    for (const input of await registration.locator('[data-sensitive="true"]').all()) assert.equal(await input.inputValue(), "");
    assert(!await page.evaluate(() => /fixture-writer|fixture-reader/.test(window.backupProofCache() + window.backupProofIntent() + JSON.stringify(localStorage) + JSON.stringify(sessionStorage))));
    const grant = section.getByRole("form", { name: "Backup policy settings", exact: true }); await grant.getByLabel("Registered destination ID", { exact: true }).fill(destinationID); await grant.getByRole("button", { name: "Grant backup policy", exact: true }).click(); await section.getByText("Backup request accepted. Waiting for agent observation.", { exact: true }).waitFor(); await control({ observe: true }); await refresh(); await section.getByText("Backup configuration observed", { exact: true }).waitFor();
    await control({ failNext: 409 }); await section.getByRole("button", { name: "Back up now", exact: true }).click(); await section.getByText("Backup request conflicts with current state. Refresh before trying again.", { exact: true }).waitFor(); assert(await section.getByRole("button", { name: "Back up now", exact: true }).isDisabled()); await refresh();
    await fillRegistration(); const forbiddenDrafts = await registration.locator('[data-sensitive="true"]').elementHandles();
    await control({ failNext: 403 }); await section.getByRole("button", { name: "Back up now", exact: true }).click(); await section.getByText("Backup authority is no longer available. Sign in again or request current operator access.", { exact: true }).waitFor(); assert(await section.getByRole("button", { name: "Back up now", exact: true }).isDisabled());
    for (const input of forbiddenDrafts) assert.equal(await input.evaluate(node => node.value), ""); assert.equal(await registration.count(), 0);
    await refresh(); await section.getByText("Backup configuration observed", { exact: true }).waitFor();
    await fillRegistration(); const detachedSecrets = await registration.locator('[data-sensitive="true"]').elementHandles(); await control({ denyHistory: true }); await refresh(); await section.getByText("Backup observation unavailable. Refresh before making changes.", { exact: true }).waitFor();
    for (const input of detachedSecrets) assert.equal(await input.evaluate(node => node.value), "");
    await page.waitForTimeout(500); const beforeDeniedPoll = await control({ inspect: true }); await page.waitForTimeout(10250); const afterDeniedPoll = await control({ inspect: true }); assert.deepEqual(afterDeniedPoll.reads, beforeDeniedPoll.reads);
    assert(!await page.evaluate(() => /fixture-writer|fixture-reader|20261025T100000/.test(window.backupProofCache())));
    await control({ denyHistory: false, readonly: true }); await refresh(); await section.getByText("Desired controls", { exact: true }).waitFor(); assert.equal(await section.getByRole("form").count(), 0);
    await control({ readonly: false }); await refresh(); await section.getByRole("form", { name: "Backup policy settings", exact: true }).waitFor();
    await control({ denyPermissions: true }); await refresh(); await section.getByText("Backup observation unavailable. Refresh before making changes.", { exact: true }).waitFor(); assert.equal(await section.getByRole("form").count(), 0);
    await control({ denyPermissions: false, readonly: true }); await refresh(); await section.getByText("Desired controls", { exact: true }).waitFor(); assert.equal(await section.getByRole("form").count(), 0);
    reviewEvidence.push({ width, registrationMutations: replay.registrationMutations, identicalReplay: true, deniedAutomaticReadWaitMs: 10250, deniedReadCountsStable: true, cacheAndDetachedSecretRetired: true, freshReadonlyReauthorization: true, parentPermissionsDenial: true, sameRevisionInvalidation: true, detailRemountStatuses: [503,403], policyRevision3RequiredAcrossRemount: true, registrationIdentityAcrossRemountAndClose: true, forbiddenDraftsWiped: true, initialHistoryPending: true, actualSessionGateRoleBarrier: true, wrongActionReceiptAmbiguous: true, restoredNondefaultProject: "20", heldDigestInvalidationPosts: 0 });
    await page.getByLabel("Fixture language", { exact: true }).selectOption("de"); await page.locator(".database-backups").getByRole("heading", { name: "Datensicherungen", exact: true }).waitFor(); assert(await page.getByRole("button", { name: "Datenbank wiederherstellen", exact: true }).isDisabled()); assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)); await page.screenshot({ path: `${output}/backup-de-${width}.png`, fullPage: true });

  }
  assert.equal(failures.length, 0, failures.join("\n"));
  await mkdir(output, { recursive: true }); await writeFile(`${output}/evidence.json`, JSON.stringify({ status: "controlled-http-only", widths: [375,1280], assertions: ["list-detail-contract", "on-demand-receipt", "separate-controls", "revision-fences", "pending-effective-revoke", "member-preview-readonly", "restore-disabled", "ephemeral-destination-registration", "grant-pending-observed", "conflict-explicit-refresh", "forbidden-locked", "english-german", "no-horizontal-overflow"], review1Transitions: reviewEvidence, nativeProof: false, consoleErrors: failures.length }, null, 2));
  console.log(`Controlled HTTP browser evidence: ${output}`);
} catch (error) { if (lastPage) { console.log("Fixture status", await lastPage.locator(".database-backups [role=status], .database-backups [role=alert]").allTextContents(), "recorded actions", commands.map(c => c.action), "metadata read counts", readCounts); await lastPage.screenshot({ path: `${output}/failure.png`, fullPage: true }); } throw error; } finally { await browser?.close(); await server.close(); await rm(cacheDir, { recursive: true, force: true }); assert.equal(server.httpServer?.listening ?? false, false); assert.equal(browser?.isConnected() ?? false, false); await assert.rejects(access(cacheDir), { code: "ENOENT" }); await writeFile(`${output}/cleanup.json`, JSON.stringify({ ownedHttpListenerAbsent: true, browserDisconnected: true, viteCacheAbsent: true, nativeCoreFixtureStarted: false, existingServicesTouched: false }, null, 2)); console.log("Owned HTTP listener/browser/cache absence verified"); }
