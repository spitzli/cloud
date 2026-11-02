import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act, useLayoutEffect } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, notifyManager, defaultScheduler } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { I18nProvider } from "@webdock/i18n/react";
import { ApiClient } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { DatabaseBackupIntentProvider, useBackupRequestMemory } from "../src/database-backup-intents";
import { ConsoleSessionGate } from "../src/ConsoleSessionGate";
import { useSession } from "../src/context";
import type { DatabaseBackupIntent } from "../src/database-backup-intent";
import type { Session } from "../src/api";
import { ManagedDatabases } from "../src/ManagedDatabases";
import { databaseViewSchema, databaseBackupPolicyViewSchema } from "@webdock/hosting-contracts";
import { createHash } from "node:crypto";
notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
import { session, backup, database, policy, history } from "./support/database-backup-fixture";
async function flush(action: () => void = () => undefined) { await act(async () => { action(); await new Promise(resolve => setTimeout(resolve, 0)); }); }
async function until(check: () => boolean) { for (let n = 0; n < 60; n++) { if (check()) return; await flush(); } assert.ok(check(), "UI did not settle"); }
type Command = Record<string, unknown> & { action: string };
function IntentWitness({ observe }: { observe: (cell: DatabaseBackupIntent) => void }) { const memory = useBackupRequestMemory("10", "2", "1"); useLayoutEffect(() => { observe(memory.cell); }, [memory.cell, observe]); return null; }
function GateDatabaseView({ projectIDs, duplicateViews }: { projectIDs: string[]; duplicateViews: boolean }) { const actor = useSession(); return <><ManagedDatabases customerID="10" projects={projectIDs.map(id => ({ id, name: `Project ${id}` }))} operator={actor.user.role === "operator"} />{duplicateViews ? <ManagedDatabases customerID="10" projects={projectIDs.map(id => ({ id, name: `Project ${id}` }))} operator={actor.user.role === "operator"} /> : null}</>; }
function fixture(override?: (command: Command, init?: RequestInit) => Response | Promise<Response> | undefined, realGate = false, duplicateViews = false, observeIntent?: (cell: DatabaseBackupIntent) => void) {
  const browser = new Window({ url: "https://console.example/databases" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, document: browser.document, HTMLElement: browser.HTMLElement, HTMLInputElement: browser.HTMLInputElement, HTMLFormElement: browser.HTMLFormElement, Event: browser.Event, FormData: browser.FormData, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  let current = databaseViewSchema.parse(database), data = databaseBackupPolicyViewSchema.parse(history), permission = { operator: true, canWrite: true };
  const calls: Command[] = [];
  let actor: Session | null = session, sessionStatus = 0;
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://console.example", enabledFeatures: ["databases"] }, async (_url, init) => {
    if (new URL(_url instanceof Request ? _url.url : _url).pathname === "/v1/auth/session") return sessionStatus ? Response.json({}, { status: sessionStatus }) : Response.json({ data: actor });
    assert(typeof init?.body === "string"); const command = JSON.parse(init.body) as Command;
    // Only non-secret requests are retained by this test recorder.
    if (command.action !== "databases.backupDestinations.register") calls.push(command);
    const custom = override?.(command, init); if (custom) return custom;
    const values: Record<string, unknown> = { "databases.permissions": permission, "databases.plans.list": [], "databases.list": [current], "databases.get": current, "databases.backupPolicies.get": data, "databases.capacity": [] };
    assert(command.action in values, `Unexpected ${command.action}`); return Response.json({ data: values[command.action] });
  });
  return { host, client, calls,
    setSessionStatus(status: number) { sessionStatus = status; },
    async logout() { actor = null; await act(async () => { await client.invalidateQueries({ queryKey: ["session"] }); }); },
    setApiOrigin(origin: string) { api.config = { ...api.config, apiOrigin: origin }; },
    setOrderingEnabled(enabled: boolean) { api.config = { ...api.config, databaseOrderingEnabled: enabled }; },
    setDatabase(value: unknown) { current = databaseViewSchema.parse(value); }, setHistory(value: unknown) { data = databaseBackupPolicyViewSchema.parse(value); }, setPermission(value: typeof permission) { permission = value; },
    async render(current = session, projectID = "2", projectIDs = [projectID]) { actor = current; await flush(() => { root.render(<I18nProvider locale="en" preference="en"><QueryClientProvider client={client}><ApiContext.Provider value={api}><DatabaseBackupIntentProvider>{realGate ? <ConsoleSessionGate><GateDatabaseView projectIDs={projectIDs} duplicateViews={duplicateViews} /></ConsoleSessionGate> : <SessionContext.Provider value={current}>{observeIntent ? <IntentWitness observe={observeIntent} /> : null}<ManagedDatabases customerID="10" projects={projectIDs.map(id => ({ id, name: `Project ${id}` }))} operator={current.user.role === "operator"} />{duplicateViews ? <ManagedDatabases customerID="10" projects={projectIDs.map(id => ({ id, name: `Project ${id}` }))} operator={current.user.role === "operator"} /> : null}</SessionContext.Provider>}</DatabaseBackupIntentProvider></ApiContext.Provider></QueryClientProvider></I18nProvider>); }); if (realGate) await act(async () => { await client.invalidateQueries({ queryKey: ["session"] }); }); },
    async open() { await until(() => host.textContent.includes("Manage database")); await flush(() => { const button = [...host.querySelectorAll("button")].find(b => b.textContent === "Manage database"); assert(button); button.click(); }); await until(() => host.querySelector(".database-details") !== null); },
    async close() { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); },
  };
}
await test("retired ordering hides new choices while existing database and backup maintenance remain available", async () => {
  const view = fixture(); view.setOrderingEnabled(false);
  try {
    await view.render(); await view.open();
    assert(![...view.host.querySelectorAll("button")].some(b => b.textContent === "Order database"));
    assert.match(view.host.textContent, /New shared and self-hosted PostgreSQL orders are retired/);
    assert.match(view.host.textContent, /Connect your Neon account in Connections/);
    assert.equal(view.host.querySelector('a[href="https://console.neon.tech"]')?.textContent, "Open Neon Console");
    await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null);
    assert(button(view.host, "Upgrade database"));
    assert(!view.calls.some(c => ["databases.offers.create", "databases.acceptOrder"].includes(c.action)));
  } finally { await view.close(); }
});
await test("a stale open new-order form is retired when ordering is disabled", async () => {
  const view = fixture(); view.setOrderingEnabled(true);
  try {
    await view.render(); await until(() => [...view.host.querySelectorAll("button")].some(b => b.textContent === "Order database"));
    await flush(() => { button(view.host, "Order database").click(); });
    await until(() => view.host.querySelector(".database-order") !== null);
    view.setOrderingEnabled(false); await view.render();
    assert.equal(view.host.querySelector(".database-order"), null);
    assert(!view.calls.some(c => ["databases.offers.create", "databases.acceptOrder"].includes(c.action)));
  } finally { await view.close(); }
});
await test("current backup observation is rendered without rewriting the historical offer", async () => {
  const view = fixture();
  try { await view.render(); await view.open(); await until(() => view.host.textContent.includes("Backup configuration observed")); assert.match(view.host.textContent, /Historical last completed backup/); assert.match(view.host.textContent, /production-qualification-required/); const restore = [...view.host.querySelectorAll("button")].find(b => b.textContent === "Restore database"); assert(restore?.disabled); assert.doesNotMatch(view.host.querySelector(".database-details")?.textContent ?? "", /Backups are not configured/); }
  finally { await view.close(); }
});
function button(host: HTMLElement, name: string) { const value = [...host.querySelectorAll("button")].find(b => b.textContent === name); assert(value, `Missing button ${name}`); return value; }
function form(host: HTMLElement, name: string) { const value = host.querySelector<HTMLFormElement>(`form[aria-label="${name}"]`); assert(value, `Missing form ${name}`); return value; }
function field(parent: HTMLFormElement, name: string, value: string | boolean) { const input = parent.elements.namedItem(name); assert(input instanceof window.HTMLInputElement); if (typeof value === "boolean") input.checked = value; else input.value = value; }
async function submit(parent: HTMLFormElement) { await flush(() => { parent.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })); }); }
function fillDestination(parent: HTMLFormElement, secret = "writer-secret-123456789") {
  for (const [key, value] of Object.entries({ endpointURL: "https://s3.example.test", region: "test", bucket: "private", keyRecoveryID: "11", writeAccessKey: "writer-key", writeSecretKey: secret, recoveryAccessKey: "reader-key", recoverySecretKey: "reader-secret-123456789" })) field(parent, key, value);
}
await test("all five states distinguish retained observations from current confirmation", async () => {
  const view = fixture();
  try {
    await view.render(); await view.open();
    for (const [status, label] of Object.entries({ "not-configured": "Backups are not configured", "configuration-pending": "Backup configuration pending observation", configured: "Backup configuration observed", "suspension-pending": "Backup suspension pending observation", suspended: "Backup schedule suspension observed" })) {
      const suspended = status === "suspended" || status === "suspension-pending";
      const pending = status.endsWith("pending");
      view.setHistory(status === "not-configured" ? { policy: null, backups: [] } : { ...history, policy: { ...policy, spec: { ...policy.spec, suspended }, observedSuspended: pending ? false : suspended, observedRevision: pending ? 1 : 2, revokedAt: status === "suspension-pending" ? new Date().toISOString() : null, observedAt: new Date().toISOString() } });
      view.setDatabase({ ...database, backup: { ...backup, status, observedRevision: pending ? 1 : 2 } });
      await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); });
      await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-management"] }); });
      await until(() => view.host.textContent.includes(label));
      if (status.endsWith("pending")) assert.match(view.host.textContent, /Previous observations — not current confirmation/);
      assert(button(view.host, "Restore database").disabled);
    }
  } finally { await view.close(); }
});
await test("members, previews and read-only operators can read history but never receive mutation controls", async () => {
  for (const [role, preview, canWrite] of [["member", false, true], ["operator", true, true], ["operator", false, false]] as const) {
    const view = fixture(); view.setPermission({ operator: role === "operator", canWrite });
    try { await view.render({ ...session, user: { ...session.user, role, preview } }); await view.open(); await until(() => view.host.textContent.includes("Historical completed backups")); assert.equal(view.host.querySelector('form[aria-label="Backup policy settings"]'), null); assert.equal(view.host.querySelector('form[aria-label="Register backup destination"]'), null); assert(!view.calls.some(c => c.action === "databases.backup")); }
    finally { await view.close(); }
  }
});
await test("independent controls use both revisions and uncertain retries freeze the exact original request", async () => {
  let writes = 0;
  const view = fixture(command => {
    if (command.action !== "databases.backupPolicies.update") return;
    writes++; if (writes === 1) return Promise.reject(new TypeError("lost response"));
    return Response.json({ data: { policyID: "5", revision: 3, operationID: "17", revocationEffective: false } });
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null);
    const settings = form(view.host, "Backup policy settings"); field(settings, "suspended", true); field(settings, "walEnabled", false); field(settings, "retentionDelete", true); await submit(settings);
    await until(() => view.host.textContent.includes("The response was lost"));
    const first = view.calls.find(c => c.action === "databases.backupPolicies.update"); assert(first); assert.equal(first.revision, 4); assert.equal(first.policyRevision, 2); assert.equal(first.suspended, true); assert.equal(first.walEnabled, false); assert.equal(first.retentionDelete, true);
    view.setDatabase({ ...database, revision: 5 }); view.setHistory({ ...history, policy: { ...policy, revision: 3 } });
    await act(async () => { await view.client.invalidateQueries(); });
    await flush(() => { button(view.host, "Retry same request").click(); }); await until(() => writes === 2);
    assert.deepEqual(view.calls.filter(c => c.action === "databases.backupPolicies.update"), [first, first]);
    await until(() => view.host.textContent.includes("17")); assert.match(view.host.textContent, /Waiting for agent observation/); assert.match(view.host.textContent, /Previous observations — not current confirmation/);
  } finally { await view.close(); }
});
await test("destination credentials are wiped, never cached, and an uncertain retry retains its request identity", async () => {
  let count = 0, key: unknown;
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    count++; if (count === 1) { key = command.idempotencyKey; return Promise.reject(new TypeError("lost response")); }
    assert.equal(command.idempotencyKey, key); return Response.json({ data: { destinationID: "71" } });
  });
  const caches = () => JSON.stringify([view.client.getQueryCache().getAll().map(q => q.state), view.client.getMutationCache().getAll().map(m => m.state)]);
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); await submit(registration);
    await until(() => view.host.textContent.includes("The response was lost"));
    for (const input of registration.querySelectorAll<HTMLInputElement>('[data-sensitive="true"]')) assert.equal(input.value, "");
    assert.doesNotMatch(caches(), /writer-key|reader-key|writer-secret|reader-secret/);
    fillDestination(registration, "changed-secret-123456789"); await submit(registration); assert.equal(count, 1);
    fillDestination(registration); await submit(registration); await until(() => view.host.textContent.includes("71")); assert.equal(count, 2); assert.doesNotMatch(caches(), /writer-key|reader-key|writer-secret|reader-secret/);
    await view.render({ ...session, session: { ...session.session, id: "different-session" } }); assert(!view.host.textContent.includes("71"));
  } finally { await view.close(); }
});
await test("scope retirement aborts secret transport and a late receipt cannot publish into a new principal", async () => {
  let finish!: (response: Response) => void, signal: AbortSignal | null | undefined;
  const pending = new Promise<Response>(resolve => { finish = resolve; });
  const view = fixture((command, init) => { if (command.action === "databases.backupDestinations.register") { signal = init?.signal; return pending; } return undefined; });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); await submit(registration); await until(() => signal !== undefined);
    await view.render({ ...session, session: { ...session.session, id: "new-session" } }); assert.equal(signal?.aborted, true);
    finish(Response.json({ data: { destinationID: "919191" } })); await flush(); assert(!view.host.textContent.includes("919191"));
  } finally { await view.close(); }
});
await test("unsubmitted credentials are wiped when the principal changes", async () => {
  const view = fixture();
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration);
    await view.render({ ...session, session: { ...session.session, id: "unsent-retirement" } });
    for (const input of registration.querySelectorAll<HTMLInputElement>('[data-sensitive="true"]')) assert.equal(input.value, "", "detached credential inputs must also be wiped");
  } finally { await view.close(); }
});
await test("a real conflict requires explicit refresh and a forbidden response retires mutation authority", async () => {
  let status = 409, writes = 0;
  const view = fixture(command => {
    if (command.action !== "databases.backup") return;
    writes++; return Response.json({ error: { message: "private provider details must never render" } }, { status });
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now")); await flush(() => { button(view.host, "Back up now").click(); });
    await until(() => view.host.textContent.includes("Backup request conflicts with current state")); assert.equal(writes, 1); assert(button(view.host, "Back up now").disabled); assert.doesNotMatch(view.host.textContent, /private provider details/);
    await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => !button(view.host, "Back up now").disabled);
    status = 403; await flush(() => { button(view.host, "Back up now").click(); }); await until(() => view.host.textContent.includes("Backup authority is no longer available"));
    view.setPermission({ operator: true, canWrite: false }); await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') === null); assert.equal(writes, 2);
  } finally { await view.close(); }
});
await test("a failed history read cannot leave current-looking observations or enabled controls", async () => {
  let fail = false;
  const view = fixture(command => { if (command.action === "databases.backupPolicies.get" && fail) return Response.json({}, { status: 403 }); return undefined; });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Historical completed backups")); fail = true;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); });
    await until(() => view.host.textContent.includes("Backup observation unavailable")); assert.match(view.host.textContent, /Previous observations — not current confirmation/); assert(button(view.host, "Back up now").disabled);
  } finally { await view.close(); }
});
await test("destination endpoints cannot hide credentials in URLs or retain a matching writer/recovery identity", async () => {
  let writes = 0;
  const view = fixture(command => { if (command.action === "databases.backupDestinations.register") { writes++; return Response.json({ data: { destinationID: "71" } }); } return undefined; });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); field(registration, "endpointURL", "https://writer:secret@s3.example.test/"); await submit(registration); assert.equal(writes, 0);
    await flush(() => { button(view.host, "Refresh backup status").click(); });
    fillDestination(registration); field(registration, "recoveryAccessKey", "writer-key"); await submit(registration); assert.equal(writes, 0);
  } finally { await view.close(); }
});
await test("principal and project changes clear the selected database before new scoped reads complete", async () => {
  let hold = false;
  const pending = new Promise<Response>(() => undefined);
  const view = fixture(command => { if (hold && ["databases.list", "databases.get"].includes(command.action)) return pending; return undefined; });
  try {
    await view.render(); await view.open(); hold = true;
    await view.render({ ...session, session: { ...session.session, id: "different-reader" } }, "20");
    assert.equal(view.host.querySelector(".database-details"), null); assert(!view.host.textContent.includes("Actual shape"));
  } finally { await view.close(); }
});
await test("grant uses an existing destination and revoke remains pending without changing WAL or retention", async () => {
  let granted = false;
  const view = fixture(command => {
    if (command.action === "databases.backupPolicies.get") return Response.json({ data: granted ? { ...history, policy: { ...policy, revision: 1, observedRevision: 1, spec: { ...policy.spec, revision: 1 } } } : { policy: null, backups: [] } });
    if (command.action === "databases.backupPolicies.grant") { granted = true; view.setDatabase({ ...database, backup: { ...backup, revision: 1, observedRevision: 1 } }); return Response.json({ data: { policyID: "5", revision: 1, operationID: "18", revocationEffective: false } }); }
    if (command.action === "databases.backupPolicies.revoke") return Response.json({ data: { policyID: "5", revision: 2, operationID: "19", revocationEffective: false } });
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null);
    const settings = form(view.host, "Backup policy settings"); field(settings, "destinationID", "71"); await submit(settings);
    await until(() => view.host.textContent.includes("Accepted operation"));
    const grant = view.calls.find(c => c.action === "databases.backupPolicies.grant"); assert(grant); assert.equal(grant.destinationID, "71"); assert.equal(grant.retentionDelete, false); assert.equal(grant.revision, 4);
    window.confirm = () => true;
    await until(() => !button(view.host, "Revoke backup grant").disabled); await flush(() => { button(view.host, "Revoke backup grant").click(); });
    await until(() => view.calls.some(c => c.action === "databases.backupPolicies.revoke")); const revoke = view.calls.find(c => c.action === "databases.backupPolicies.revoke"); assert(revoke); assert.equal(revoke.policyRevision, 1); assert.equal("walEnabled" in revoke, false); assert.equal("retentionDelete" in revoke, false); assert.match(view.host.textContent, /Waiting for agent observation/);
  } finally { await view.close(); }
});
await test("loss of current read authority aborts an in-flight credential request and suppresses its late receipt", async () => {
  let fail = false, finish!: (response: Response) => void, signal: AbortSignal | null | undefined;
  const pending = new Promise<Response>(resolve => { finish = resolve; });
  const view = fixture((command, init) => {
    if (command.action === "databases.backupDestinations.register") { signal = init?.signal; return pending; }
    if (fail && command.action === "databases.backupPolicies.get") return Response.json({}, { status: 403 });
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => signal !== undefined); fail = true;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); }); assert.equal(signal?.aborted, true);
    finish(Response.json({ data: { destinationID: "919192" } })); await flush(); assert(!view.host.textContent.includes("919192"));
  } finally { await view.close(); }
});
await test("Shared databases do not query or activate unsupported backup controls", async () => {
  const shared = { ...database, kind: "shared", storageEnforcement: "measured-logical", cpuMillicores: null, memoryBytes: null, volumeBytes: null, logicalStorageAllowanceBytes: 1073741824, backup: { status: "not-configured", observedControls: null, lastSuccessAt: null, recoverableUntil: null, lastRestoreTestAt: null, publicRecoveryAvailable: false, disabledReason: "production-qualification-required", rpoSeconds: null, rtoSeconds: null } };
  const view = fixture(command => { if (command.action === "databases.list") return Response.json({ data: [shared] }); if (command.action === "databases.get") return Response.json({ data: shared }); return undefined; });
  try { await view.render(); await view.open(); assert.match(view.host.textContent, /Shared PostgreSQL backup, restore and clone are unavailable/); assert(!view.calls.some(c => c.action === "databases.backupPolicies.get")); assert.equal(view.host.querySelector('form[aria-label="Backup policy settings"]'), null); assert(button(view.host, "Restore database").disabled); }
  finally { await view.close(); }
});
await test("a configured live database cannot rewrite the backup exclusion in an immutable reviewed offer", async () => {
  const plan = { kind: "dedicated", postgresMajor: 18, name: "Reviewed plan", monthlyPriceCents: 1250, currency: "EUR", cpuMillicores: 500, memoryBytes: 1073741824, volumeBytes: 1610612736, logicalStorageAllowanceBytes: null, connectionLimit: 10, terms: "Immutable reviewed terms without a promised recovery window." };
  const view = fixture(command => {
    if (command.action === "databases.plans.list") return Response.json({ data: [{ id: "3", revision: 1, availability: "pilot", plan }] });
    if (command.action === "databases.offers.create") return Response.json({ data: { id: "40", name: "Reviewed DB", expiresAt: "2099-01-01T00:00:00Z", snapshot: { ...plan, backupStatus: "not-configured" } } });
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Backup configuration observed")); await flush(() => { button(view.host, "Order database").click(); });
    const order = view.host.querySelector<HTMLFormElement>(".database-order form"); assert(order); field(order, "name", "Reviewed DB"); await submit(order);
    await until(() => view.host.textContent.includes("Immutable reviewed terms")); assert.match(view.host.querySelector(".database-order")?.textContent ?? "", /Backups are not configured. No recovery window is included/); assert.match(view.host.querySelector(".database-backups")?.textContent ?? "", /Backup configuration observed/);
  } finally { await view.close(); }
});
await test("local session expiry aborts an in-flight request without treating a late receipt as success", async () => {
  let signal: AbortSignal | null | undefined, finish!: (response: Response) => void;
  const pending = new Promise<Response>(resolve => { finish = resolve; });
  const view = fixture((command, init) => { if (command.action === "databases.backup") { signal = init?.signal; return pending; } return undefined; });
  try {
    await view.render({ ...session, session: { ...session.session, expiresAt: new Date(Date.now() + 150).toISOString() } }); await view.open(); await until(() => view.host.textContent.includes("Back up now")); await flush(() => { button(view.host, "Back up now").click(); }); await until(() => signal !== undefined);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 170)); }); assert.equal(signal?.aborted, true); assert(button(view.host, "Back up now").disabled);
    finish(Response.json({ data: { policyID: "5", revision: 2, operationID: "919193", revocationEffective: false } })); await flush(); assert(!view.host.textContent.includes("919193"));
  } finally { await view.close(); }
});
await test("a newer desired policy read makes an older configured view historical until its revision is observed", async () => {
  const view = fixture();
  view.setHistory({ ...history, policy: { ...policy, revision: 3, spec: { ...policy.spec, revision: 3 } } });
  try { await view.render(); await view.open(); await until(() => view.host.textContent.includes("Historical completed backups")); assert.match(view.host.textContent, /Previous observations — not current confirmation/); assert.match(view.host.textContent, /Backup configuration pending observation/); assert(button(view.host, "Back up now").disabled); }
  finally { await view.close(); }
});
await test("review1: failed current parent permissions block dispatch and abort pending registration", async () => {
  let deny = false, writes = 0, signal: AbortSignal | null | undefined, finish!: (response: Response) => void;
  const pending = new Promise<Response>(resolve => { finish = resolve; });
  const view = fixture((command, init) => {
    if (command.action === "databases.permissions" && deny) return Response.json({}, { status: 403 });
    if (command.action === "databases.backupDestinations.register") { signal = init?.signal; return pending; }
    if (command.action === "databases.backup") { writes++; return Response.json({ data: { policyID: "5", revision: 2, operationID: "31", revocationEffective: false } }); }
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); await submit(registration); await until(() => signal !== undefined); deny = true;
    await act(async () => { await view.client.invalidateQueries({ predicate: q => q.queryKey.at(-1) === "permissions" }); });
    assert.equal(signal?.aborted, true, "parent permission failure must abort pending transport"); finish(Response.json({ data: { destinationID: "919194" } })); await flush(); assert(!view.host.textContent.includes("919194"));
    const action = [...view.host.querySelectorAll("button")].find(b => b.textContent === "Back up now"); await flush(() => { action?.click(); }); assert.equal(writes, 0);
    for (const input of registration.querySelectorAll<HTMLInputElement>('input[data-sensitive="true"]')) assert.equal(input.value, "");
  } finally { await view.close(); }
});
await test("review1: a cached parent write permission cannot dispatch backup after its refresh returns 403", async () => {
  let deny = false, writes = 0;
  const view = fixture(command => {
    if (deny && command.action === "databases.permissions") return Response.json({}, { status: 403 });
    if (command.action === "databases.backup") { writes++; return Response.json({ data: { policyID: "5", revision: 2, operationID: "33", revocationEffective: false } }); }
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now")); deny = true;
    const registration = form(view.host, "Register backup destination"); fillDestination(registration);
    await act(async () => { await view.client.invalidateQueries({ predicate: q => q.queryKey.at(-1) === "permissions" }); });
    await flush(() => { [...view.host.querySelectorAll("button")].find(b => b.textContent === "Back up now")?.click(); }); assert.equal(writes, 0);
    for (const input of registration.querySelectorAll<HTMLInputElement>('input[data-sensitive="true"]')) assert.equal(input.value, "");
  } finally { await view.close(); }
});
await test("review2: the same principal's fresh reauthorization preserves an unresolved registration retry identity", async () => {
  let deny = false;
  const received: string[] = [];
  const view = fixture(command => {
    if (deny && command.action === "databases.permissions") return Response.json({}, { status: 403 });
    if (command.action === "databases.backupDestinations.register") {
      received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
      if (received.length === 1) return Promise.reject(new TypeError("lost committed response"));
      return Response.json({ data: { destinationID: "74" } });
    }
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => view.host.textContent.includes("The response was lost"));
    deny = true; await act(async () => { await view.client.invalidateQueries({ predicate: q => q.queryKey.at(-1) === "permissions" }); }); assert.equal(view.host.querySelector('form[aria-label="Register backup destination"]'), null);
    deny = false; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review2: invalid re-entry and refresh cannot erase an uncertain registration identity", async () => {
  const received: { key: unknown; hash: string }[] = [], committed = new Set<unknown>();
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push({ key: command.idempotencyKey, hash: createHash("sha256").update(JSON.stringify(command)).digest("hex") }); committed.add(command.idempotencyKey);
    if (received.length === 1) return Promise.reject(new TypeError("lost committed response"));
    return Response.json({ data: { destinationID: "71" } });
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); await submit(registration); await until(() => view.host.textContent.includes("The response was lost"));
    fillDestination(registration); field(registration, "endpointURL", "http://s3.example.test"); await submit(registration); await flush(() => { button(view.host, "Refresh backup status").click(); }); await flush();
    fillDestination(registration); await submit(registration); await until(() => received.length === 2); assert.deepEqual(received[1], received[0]); assert.equal(committed.size, 1);
  } finally { await view.close(); }
});
await test("review3: same-revision observation invalidation, staleness and suspension mismatch deny manual backup", async () => {
  for (const change of [{ observedAt: null }, { observedAt: new Date(Date.now() - 91000).toISOString() }, { observedSuspended: true }, { observedRevision: 1 }, { observedControls: null }]) {
    const view = fixture(command => { if (command.action === "databases.backupPolicies.get") return Response.json({ data: { ...history, policy: { ...policy, observedAt: new Date().toISOString(), ...change } } }); return undefined; });
    try { await view.render(); await view.open(); await until(() => view.calls.some(c => c.action === "databases.backupPolicies.get")); await flush(); assert(button(view.host, "Back up now").disabled, JSON.stringify(change)); assert.match(view.host.textContent, /Previous observations — not current confirmation/); }
    finally { await view.close(); }
  }
});
await test("review4: unrelated destination receipts cannot clear an outstanding policy observation target", async () => {
  let updates = 0;
  const view = fixture(command => {
    if (command.action === "databases.backupPolicies.update") return Response.json({ data: { policyID: "5", revision: ++updates + 2, operationID: "32", revocationEffective: false } });
    if (command.action === "databases.backupDestinations.register") return Response.json({ data: { destinationID: "72" } });
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null); await submit(form(view.host, "Backup policy settings")); await until(() => view.host.textContent.includes("Waiting for agent observation"));
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); await submit(registration); await until(() => view.host.textContent.includes("72"));
    assert(button(view.host, "Back up now").disabled); assert.match(view.host.textContent, /Waiting for agent observation/);
    view.setHistory({ ...history, policy: { ...policy, revision: 3, spec: { ...policy.spec, revision: 3 } } }); view.setDatabase({ ...database, backup: { ...backup, revision: 3, status: "configuration-pending" } });
    await act(async () => { await view.client.invalidateQueries(); }); await submit(form(view.host, "Backup policy settings")); await until(() => updates === 2);
    view.setHistory({ ...history, policy: { ...policy, revision: 3, observedRevision: 3, observedAt: new Date().toISOString(), spec: { ...policy.spec, revision: 3 } } }); view.setDatabase({ ...database, backup: { ...backup, revision: 3, observedRevision: 3 } });
    await act(async () => { await view.client.invalidateQueries(); }); assert(button(view.host, "Back up now").disabled);
    view.setHistory({ ...history, policy: { ...policy, revision: 4, observedRevision: 4, observedAt: new Date().toISOString(), spec: { ...policy.spec, revision: 4 } } }); view.setDatabase({ ...database, backup: { ...backup, revision: 4, observedRevision: 4 } });
    await act(async () => { await view.client.invalidateQueries(); }); await until(() => !button(view.host, "Back up now").disabled);
  } finally { await view.close(); }
});
await test("review5: history denial wipes unsent fields, retires cached history and suppresses automatic reads until reauthorized", async () => {
  let deny = false;
  const view = fixture(command => { if (deny && command.action === "databases.backupPolicies.get") return Response.json({}, { status: 403 }); return undefined; });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    const registration = form(view.host, "Register backup destination"); fillDestination(registration); deny = true;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); });
    for (const input of registration.querySelectorAll<HTMLInputElement>('input[data-sensitive="true"]')) assert.equal(input.value, "");
    const before = view.calls.filter(c => c.action === "databases.backupPolicies.get").length;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); await view.client.refetchQueries({ queryKey: ["database-backups"] }); });
    assert.equal(view.calls.filter(c => c.action === "databases.backupPolicies.get").length, before); assert.equal(view.client.getQueryCache().findAll({ queryKey: ["database-backups"] }).filter(q => q.state.data !== undefined).length, 0);
    deny = false; view.setPermission({ operator: false, canWrite: false }); await flush(() => { button(view.host, "Refresh backup status").click(); });
    await until(() => view.host.textContent.includes("Historical completed backups")); assert.equal(view.host.querySelector('form[aria-label="Backup policy settings"]'), null);
  } finally { await view.close(); }
});
await test("review5: a denied project's latch cannot block a different project with fresh permissions", async () => {
  let deny = false;
  const view = fixture(command => {
    if (deny && command.action === "databases.backupPolicies.get") return Response.json({}, { status: 403 });
    if (command.action === "databases.list" && command.projectID === "20") return Response.json({ data: [] });
    return undefined;
  });
  try {
    await view.render(session, "2", ["2", "20"]); await view.open(); await until(() => view.host.textContent.includes("Historical completed backups")); deny = true;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); });
    const select = view.host.querySelector<HTMLSelectElement>(".database-management > label select"); assert(select);
    await flush(() => { select.value = "20"; select.dispatchEvent(new window.Event("change", { bubbles: true })); });
    await until(() => view.host.textContent.includes("No databases available.")); assert(view.calls.some(c => c.action === "databases.permissions" && c.projectID === "20"));
  } finally { await view.close(); }
});
await test("review5: overlapping reauthorization cannot treat a cancelled permission read's cached success as fresh authority", async () => {
  let deny = false, hold = false;
  const replies: ((value: Response) => void)[] = [];
  const view = fixture(command => {
    if (hold && command.action === "databases.permissions") return new Promise<Response>(resolve => { replies.push(resolve); });
    if (deny && command.action === "databases.backupPolicies.get") return Response.json({}, { status: 403 });
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Historical completed backups")); deny = true;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); });
    const reads = view.calls.filter(c => c.action === "databases.backupPolicies.get").length; hold = true;
    await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => replies.length === 1);
    await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => replies.length === 2); await flush();
    assert.equal(view.calls.filter(c => c.action === "databases.backupPolicies.get").length, reads);
  } finally {
    for (const reply of replies) reply(Response.json({ data: { operator: false, canWrite: false } }));
    await view.close();
  }
});
async function recoverDetail(view: ReturnType<typeof fixture>) {
  await flush(() => { const retry = [...view.host.querySelectorAll("button")].find(b => b.textContent.trim() === "Try again"); assert(retry); retry.click(); });
  await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
}
await test("review2: lost registration keeps one request identity across detail 503 and 403 remounts", async () => {
  for (const status of [503, 403]) {
    let detailStatus = 0;
    const received: string[] = [], committed = new Set<unknown>();
    const view = fixture(command => {
      if (command.action === "databases.get" && detailStatus) return Response.json({}, { status: detailStatus });
      if (command.action === "databases.backupDestinations.register") {
        received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex")); committed.add(command.idempotencyKey);
        if (received.length === 1) return Promise.reject(new TypeError("lost committed reply"));
        return Response.json({ data: { destinationID: "75" } });
      }
      return undefined;
    });
    try {
      await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
      fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => view.host.textContent.includes("The response was lost"));
      detailStatus = status; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => view.host.querySelector(".database-details") === null);
      detailStatus = 0; await recoverDetail(view); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2);
      assert.equal(received[0], received[1], `detail ${String(status)} must retain the exact original command/key`); assert.equal(committed.size, 1);
    } finally { await view.close(); }
  }
});
await test("review2: accepted policy target survives detail 503 and 403 remounts until fresh revision 3", async () => {
  for (const status of [503, 403]) {
    let detailStatus = 0;
    const view = fixture(command => {
      if (command.action === "databases.get" && detailStatus) return Response.json({}, { status: detailStatus });
      if (command.action === "databases.backupPolicies.update") return Response.json({ data: { policyID: "5", revision: 3, operationID: "34", revocationEffective: false } });
      return undefined;
    });
    try {
      await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null); await submit(form(view.host, "Backup policy settings")); await until(() => view.host.textContent.includes("Waiting for agent observation"));
      detailStatus = status; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => view.host.querySelector(".database-details") === null);
      detailStatus = 0; await recoverDetail(view); assert(button(view.host, "Back up now").disabled); assert.match(view.host.textContent, /Waiting for agent observation/);
      view.setDatabase({ ...database, backup: { ...backup, revision: 3, observedRevision: 3 } }); view.setHistory({ ...history, policy: { ...policy, revision: 3, observedRevision: 3, spec: { ...policy.spec, revision: 3 }, observedAt: new Date().toISOString() } });
      await act(async () => { await view.client.invalidateQueries(); }); await until(() => !button(view.host, "Back up now").disabled);
    } finally { await view.close(); }
  }
});
await test("review2: pending registration detail 403 aborts transport but retains retry identity and suppresses a late receipt", async () => {
  let detailStatus = 0, finish!: (response: Response) => void, signal: AbortSignal | null | undefined;
  const pending = new Promise<Response>(resolve => { finish = resolve; }); const received: string[] = [];
  const view = fixture((command, init) => {
    if (command.action === "databases.get" && detailStatus) return Response.json({}, { status: detailStatus });
    if (command.action === "databases.backupDestinations.register") {
      received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
      if (received.length === 1) { signal = init?.signal; return pending; }
      return Response.json({ data: { destinationID: "76" } });
    }
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => signal !== undefined);
    detailStatus = 403; await act(async () => { await view.client.invalidateQueries({ predicate: q => q.queryKey.at(-1) === "detail" }); }); assert.equal(signal?.aborted, true);
    detailStatus = 0; await recoverDetail(view); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
    finish(Response.json({ data: { destinationID: "919196" } })); await flush(); assert(!view.host.textContent.includes("919196"));
  } finally { await view.close(); }
});
await test("review2: forbidden manual, update, revoke and registration responses wipe connected and detached credential drafts", async () => {
  for (const action of ["databases.backup", "databases.backupPolicies.update", "databases.backupPolicies.revoke", "databases.backupDestinations.register"]) {
    let finish!: (response: Response) => void, sent = false;
    const pending = new Promise<Response>(resolve => { finish = resolve; });
    const view = fixture(command => { if (command.action === action) { sent = true; return pending; } return undefined; });
    try {
      await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
      const registration = form(view.host, "Register backup destination"); fillDestination(registration);
      if (action === "databases.backup") await flush(() => { button(view.host, "Back up now").click(); });
      if (action === "databases.backupPolicies.update") await submit(form(view.host, "Backup policy settings"));
      if (action === "databases.backupPolicies.revoke") { window.confirm = () => true; await flush(() => { button(view.host, "Revoke backup grant").click(); }); }
      if (action === "databases.backupDestinations.register") await submit(registration);
      await until(() => sent); fillDestination(registration); finish(Response.json({}, { status: 403 })); await until(() => view.host.textContent.includes("Backup authority is no longer available"));
      for (const input of registration.querySelectorAll<HTMLInputElement>('input[data-sensitive="true"]')) assert.equal(input.value, "", action);
      assert.doesNotMatch(JSON.stringify([view.client.getQueryCache().getAll().map(q => q.state), view.client.getMutationCache().getAll().map(m => m.state)]), /writer-secret|reader-secret|writer-key|reader-key/);
      await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
      for (const input of form(view.host, "Register backup destination").querySelectorAll<HTMLInputElement>('input[data-sensitive="true"]')) assert.equal(input.value, "");
    } finally { await view.close(); }
  }
});
await test("review2: initial history pending cannot label a configured cached view as currently observed", async () => {
  let finish!: (response: Response) => void; const pending = new Promise<Response>(resolve => { finish = resolve; });
  const view = fixture(command => { if (command.action === "databases.backupPolicies.get") return pending; return undefined; });
  try {
    await view.render(); await view.open(); assert.match(view.host.querySelector('.database-backups [role="status"]')?.textContent ?? "", /Backup configuration pending observation/); assert.equal(view.host.querySelector('form[aria-label="Backup policy settings"]'), null);
    finish(Response.json({ data: history })); await until(() => view.host.textContent.includes("Backup configuration observed"));
  } finally { await view.close(); }
});
await test("review2: closing and reopening the same resource keeps an unresolved non-secret request identity", async () => {
  const received: string[] = [];
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
    if (received.length === 1) return Promise.reject(new TypeError("lost committed reply"));
    return Response.json({ data: { destinationID: "77" } });
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => view.host.textContent.includes("The response was lost"));
    await flush(() => { button(view.host, "Close").click(); }); assert.equal(view.host.querySelector(".database-details"), null); await view.open();
    await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review2: read-write role changes and project-list metadata keep same-resource retry identity", async () => {
  const received: string[] = [];
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
    if (received.length === 1) return Promise.reject(new TypeError("lost committed reply"));
    return Response.json({ data: { destinationID: "78" } });
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => view.host.textContent.includes("The response was lost"));
    await view.render({ ...session, user: { ...session.user, role: "member" } }); assert.equal(view.host.querySelector('form[aria-label="Register backup destination"]'), null);
    await view.render(session, "2", ["2", "20"]); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review2: an API-origin change retires the previous platform's request identity", async () => {
  const received: unknown[] = [];
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push(command.idempotencyKey);
    if (received.length === 1) return Promise.reject(new TypeError("lost committed reply"));
    return Response.json({ data: { destinationID: "79" } });
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => view.host.textContent.includes("The response was lost"));
    view.setApiOrigin("https://different-platform.example.test"); await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.notEqual(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review3: actual SessionGate role remount retains committed registration identity", async () => {
  const received: string[] = [];
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
    if (received.length === 1) return Promise.reject(new TypeError("lost committed reply"));
    return Response.json({ data: { destinationID: "81" } });
  }, true);
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => view.host.textContent.includes("The response was lost"));
    await view.render({ ...session, user: { ...session.user, role: "member" } }); await view.open(); assert.equal(view.host.querySelector('form[aria-label="Register backup destination"]'), null);
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review3: actual SessionGate preserves revision 3 expectation across same-session roles", async () => {
  const view = fixture(command => { if (command.action === "databases.backupPolicies.update") return Response.json({ data: { policyID: "5", revision: 3, operationID: "35", revocationEffective: false } }); return undefined; }, true);
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null); await submit(form(view.host, "Backup policy settings")); await until(() => view.host.textContent.includes("Waiting for agent observation"));
    await view.render({ ...session, user: { ...session.user, role: "member" } }); await view.open(); await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now")); assert(button(view.host, "Back up now").disabled); assert.match(view.host.textContent, /Waiting for agent observation/);
  } finally { await view.close(); }
});
await test("review3: stale overlapping refresh completion cannot clear a newly dispatched registration", async () => {
  let conflict = true, historyHold = false, permissionHold = false, holdBHistory = false, finishBHistory!: (response: Response) => void, finishA!: (response: Response) => void, finishB!: (response: Response) => void, loseC!: () => void;
  const received: string[] = [];
  const view = fixture(command => {
    if (command.action === "databases.backup" && conflict) { conflict = false; return Response.json({}, { status: 409 }); }
    if (command.action === "databases.permissions" && permissionHold) { permissionHold = false; return new Promise<Response>(resolve => { finishB = resolve; }); }
    if (command.action === "databases.backupPolicies.get" && historyHold) { historyHold = false; return new Promise<Response>(resolve => { finishA = resolve; }); }
    if (command.action === "databases.backupPolicies.get" && holdBHistory) { holdBHistory = false; return new Promise<Response>(resolve => { finishBHistory = resolve; }); }
    if (command.action === "databases.backupDestinations.register") {
      received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
      if (received.length === 1) return new Promise<Response>((_resolve, reject) => { loseC = () => { reject(new TypeError("lost C commit")); }; });
      return Response.json({ data: { destinationID: "82" } });
    }
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now")); await flush(() => { button(view.host, "Back up now").click(); }); await until(() => view.host.textContent.includes("conflicts with current state"));
    historyHold = true; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => typeof finishA === "function"); permissionHold = true; holdBHistory = true; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => typeof finishB === "function");
    finishA(Response.json({ data: history })); await flush(); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination"));
    finishB(Response.json({ data: { operator: true, canWrite: true } })); await until(() => typeof finishBHistory === "function"); finishBHistory(Response.json({ data: history })); for (let n = 0; n < 8; n++) await flush();
    if (received.length === 0) { fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); }
    await until(() => received.length === 1); loseC(); await until(() => view.host.textContent.includes("The response was lost")); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review3: wrong-action and malformed registration receipts remain ambiguous under the original key", async () => {
  for (const wrong of [{ policyID: "5", revision: 2, operationID: "36", revocationEffective: false }, { destinationID: "83", unexpected: true }, { destinationID: 83 }]) {
    const received: string[] = [];
    const view = fixture(command => {
      if (command.action !== "databases.backupDestinations.register") return;
      received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex"));
      if (received.length === 1) return Promise.reject(new TypeError("lost commit"));
      return Response.json({ data: received.length === 2 ? wrong : { destinationID: "83" } });
    });
    try {
      await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
      for (let n = 0; n < 3; n++) { fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === n + 1); await flush(); }
      assert.equal(received[0], received[1]); assert.equal(received[0], received[2]);
    } finally { await view.close(); }
  }
});
await test("review3: update receipts must bind exact policy identity and next revision before clearing intent", async () => {
  for (const wrong of [{ destinationID: "84" }, { policyID: "6", revision: 3, operationID: "37", revocationEffective: false }, { policyID: "5", revision: 2, operationID: "37", revocationEffective: false }, { policyID: "5", revision: 4, operationID: "37", revocationEffective: false }]) {
    const received: string[] = [];
    const view = fixture(command => { if (command.action === "databases.backupPolicies.update") { received.push(String(command.idempotencyKey)); return Response.json({ data: received.length === 1 ? wrong : { policyID: "5", revision: 3, operationID: "37", revocationEffective: false } }); } return undefined; });
    try {
      await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null); await submit(form(view.host, "Backup policy settings")); await until(() => view.host.textContent.includes("The response was lost")); await flush(() => { button(view.host, "Retry same request").click(); }); await until(() => received.length === 2); assert.equal(received[0], received[1]);
    } finally { await view.close(); }
  }
});
await test("review3: actual SessionGate transient failure preserves intent but confirmed logout retires it", async () => {
  const received: string[] = [];
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push(String(command.idempotencyKey));
    if (received.length === 1 || received.length === 3) return Promise.reject(new TypeError("lost commit"));
    return Response.json({ data: { destinationID: "85" } });
  }, true);
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 1); await flush();
    view.setSessionStatus(503); await view.render(); await until(() => view.host.querySelector(".database-details") === null); view.setSessionStatus(0); await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
    fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 3); await flush();
    await view.logout(); await until(() => view.host.querySelector(".database-management") === null); await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 4); assert.notEqual(received[2], received[3]);
  } finally { await view.close(); }
});
await test("review3: authority denial fences an older held refresh until a newly authorized refresh", async () => {
  let deny = false, hold = false, finish!: (response: Response) => void;
  const view = fixture(command => {
    if (deny && command.action === "databases.permissions") return Response.json({}, { status: 403 });
    if (hold && command.action === "databases.backupPolicies.get") { hold = false; return new Promise<Response>(resolve => { finish = resolve; }); }
    return undefined;
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
    hold = true; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => typeof finish === "function"); deny = true;
    await act(async () => { await view.client.invalidateQueries({ predicate: q => q.queryKey.at(-1) === "permissions" }); }); finish(Response.json({ data: history })); for (let n = 0; n < 6; n++) await flush();
    assert.equal(view.host.querySelector('form[aria-label="Register backup destination"]'), null);
    deny = false; await flush(() => { button(view.host, "Refresh backup status").click(); }); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null);
  } finally { await view.close(); }
});
await test("review3: a session 401 hides private views without assuming confirmed logout of the same identity", async () => {
  const received: string[] = [];
  const view = fixture(command => {
    if (command.action !== "databases.backupDestinations.register") return;
    received.push(String(command.idempotencyKey));
    if (received.length === 1) return Promise.reject(new TypeError("lost commit"));
    return Response.json({ data: { destinationID: "86" } });
  }, true);
  try {
    await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 1); await flush();
    view.setSessionStatus(401); await view.render(); await until(() => view.host.querySelector(".database-management") === null); view.setSessionStatus(0); await view.render(); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]);
  } finally { await view.close(); }
});
await test("review3: same-resource duplicate views share denial fencing without automatic authority restoration", async () => {
  let deny = false;
  const view = fixture(command => { if (deny && command.action === "databases.backupPolicies.get") return Response.json({}, { status: 403 }); return undefined; }, false, true);
  try {
    await view.render(); await view.open();
    const second = view.host.querySelectorAll<HTMLElement>(".database-management")[1]; assert(second);
    await until(() => second.textContent.includes("Manage database")); await flush(() => { button(second, "Manage database").click(); }); await until(() => view.host.querySelectorAll('form[aria-label="Register backup destination"]').length === 2);
    deny = true; await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); }); await until(() => view.host.querySelectorAll('form[aria-label="Register backup destination"]').length === 0);
    const reads = view.calls.filter(c => c.action === "databases.backupPolicies.get").length;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); await view.client.refetchQueries({ queryKey: ["database-backups"] }); });
    assert.equal(view.calls.filter(c => c.action === "databases.backupPolicies.get").length, reads);
  } finally { await view.close(); }
});
await test("review3: a known permission denial fences shared intent before query-error observers or React paint", async () => {
  let deny = false, cell: DatabaseBackupIntent | undefined, observed = false, fenced = false;
  const view = fixture(command => { if (deny && command.action === "databases.permissions") return Response.json({}, { status: 403 }); return undefined; }, false, false, value => { cell = value; });
  const unsubscribe = view.client.getQueryCache().subscribe(event => {
    const key: unknown = event.query.queryKey;
    if (Array.isArray(key) && key[key.length - 1] === "permissions" && event.query.state.status === "error") { observed = true; fenced = cell?.snapshot().readDenied === true; }
  });
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now")); deny = true;
    await act(async () => { await view.client.invalidateQueries({ predicate: q => q.queryKey.at(-1) === "permissions" }); }); assert(observed); assert(fenced);
  } finally { unsubscribe(); await view.close(); }
});
async function selectProject(view: ReturnType<typeof fixture>, value: string) {
  const select = view.host.querySelector<HTMLSelectElement>(".database-management > label select"); assert(select);
  await flush(() => { select.value = value; select.dispatchEvent(new window.Event("change", { bubbles: true })); });
}
await test("review4: restoring non-default project after actual Gate roles preserves one registration mutation", async () => {
  const received: string[] = [], committed = new Set<unknown>();
  const view = fixture(command => {
    if (command.action === "databases.list" && command.projectID === "2") return Response.json({ data: [] });
    if (command.action === "databases.backupDestinations.register") {
      received.push(createHash("sha256").update(JSON.stringify(command)).digest("hex")); committed.add(command.idempotencyKey);
      if (received.length === 1) return Promise.reject(new TypeError("lost committed reply"));
      return Response.json({ data: { destinationID: "87" } });
    }
    return undefined;
  }, true);
  view.setDatabase({ ...database, projectID: "20" }); view.setHistory({ ...history, policy: { ...policy, spec: { ...policy.spec, projectID: "20", destination: { ...policy.spec.destination, archivePath: "postgresql/10/20/6/5" } } } });
  try {
    await view.render(session, "2", ["2", "20"]); await selectProject(view, "20"); await view.open(); await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 1); await flush();
    for (const role of ["member", "operator"]) { await view.render({ ...session, user: { ...session.user, role } }, "2", ["2", "20"]); await selectProject(view, "20"); await view.open(); }
    await until(() => view.host.querySelector('form[aria-label="Register backup destination"]') !== null); fillDestination(form(view.host, "Register backup destination")); await submit(form(view.host, "Register backup destination")); await until(() => received.length === 2); assert.equal(received[0], received[1]); assert.equal(committed.size, 1);
  } finally { await view.close(); }
});
await test("review4: restoring non-default project after actual Gate roles retains highest policy expectation", async () => {
  const view = fixture(command => {
    if (command.action === "databases.list" && command.projectID === "2") return Response.json({ data: [] });
    if (command.action === "databases.backupPolicies.update") return Response.json({ data: { policyID: "5", revision: 3, operationID: "41", revocationEffective: false } });
    return undefined;
  }, true);
  const inProject = { ...database, projectID: "20" }, inHistory = { ...history, policy: { ...policy, spec: { ...policy.spec, projectID: "20", destination: { ...policy.spec.destination, archivePath: "postgresql/10/20/6/5" } } } };
  view.setDatabase(inProject); view.setHistory(inHistory);
  try {
    await view.render(session, "2", ["2", "20"]); await selectProject(view, "20"); await view.open(); await until(() => view.host.querySelector('form[aria-label="Backup policy settings"]') !== null); await submit(form(view.host, "Backup policy settings")); await until(() => view.host.textContent.includes("Waiting for agent observation"));
    for (const role of ["member", "operator"]) { await view.render({ ...session, user: { ...session.user, role } }, "2", ["2", "20"]); await selectProject(view, "20"); await view.open(); }
    await until(() => view.host.textContent.includes("Back up now")); assert(button(view.host, "Back up now").disabled); assert.match(view.host.textContent, /Waiting for agent observation/);
    view.setDatabase({ ...inProject, backup: { ...backup, revision: 3, observedRevision: 3 } }); view.setHistory({ ...inHistory, policy: { ...inHistory.policy, revision: 3, observedRevision: 3, spec: { ...inHistory.policy.spec, revision: 3 }, observedAt: new Date().toISOString() } }); await act(async () => { await view.client.invalidateQueries(); }); await until(() => !button(view.host, "Back up now").disabled);
  } finally { await view.close(); }
});
function holdFirstDigest() {
  const original = crypto.subtle.digest.bind(crypto.subtle); let held = false, release!: () => void;
  crypto.subtle.digest = async (algorithm, data) => { if (!held) { held = true; await new Promise<void>(resolve => { release = resolve; }); } return original(algorithm, data); };
  return { held: () => held, release: () => { release(); }, restore: () => { crypto.subtle.digest = original; } };
}
for (const mode of ["invalidation", "expiry"]) await test(`review4: held digest cannot dispatch after ${mode}`, async () => {
    let posts = 0; const originalNow = Date.now, base = originalNow(); let now = base; Date.now = () => now;
    const view = fixture(command => { if (command.action === "databases.backup") { posts++; return Response.json({ data: { policyID: "5", revision: 2, operationID: "42", revocationEffective: false } }); } return undefined; }, true);
    view.setHistory({ ...history, policy: { ...policy, observedAt: new Date(base).toISOString() } });
    const digest = holdFirstDigest();
    try {
      await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now")); await flush(() => { button(view.host, "Back up now").click(); }); await until(digest.held);
      if (mode === "expiry") now = base + 90001; else view.setHistory({ ...history, policy: { ...policy, observedAt: null } });
      await act(async () => { await view.client.invalidateQueries({ queryKey: ["database-backups"] }); }); assert(button(view.host, "Back up now").disabled);
      digest.release(); for (let n = 0; n < 8; n++) await flush(); assert.equal(posts, 0, mode);
    } finally { digest.restore(); Date.now = originalNow; await view.close(); }
});
await test("review4: held digest rechecks the current policy target accepted by another actual parent", async () => {
  let posts = 0;
  const view = fixture(command => { if (command.action === "databases.backup") { posts++; return Response.json({ data: { policyID: "5", revision: 2, operationID: "43", revocationEffective: false } }); } if (command.action === "databases.backupPolicies.update") return Response.json({ data: { policyID: "5", revision: 3, operationID: "44", revocationEffective: false } }); return undefined; }, true, true);
  const digest = holdFirstDigest();
  try {
    await view.render(); await view.open(); await until(() => view.host.textContent.includes("Back up now"));
    const second = view.host.querySelectorAll<HTMLElement>(".database-management")[1]; assert(second); await until(() => second.textContent.includes("Manage database")); await flush(() => { button(second, "Manage database").click(); }); await until(() => second.querySelector('form[aria-label="Backup policy settings"]') !== null);
    await flush(() => { button(view.host, "Back up now").click(); }); await until(digest.held); await submit(form(second, "Backup policy settings")); await until(() => second.textContent.includes("Waiting for agent observation"));
    digest.release(); for (let n = 0; n < 8; n++) await flush(); assert.equal(posts, 0);
  } finally { digest.restore(); await view.close(); }
});
