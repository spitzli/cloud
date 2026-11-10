import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, notifyManager, defaultScheduler } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { I18nProvider } from "@spitzli/i18n/react";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { WorkspaceConnections } from "../src/Connections";
notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const actor: Session = { user: { id: "1", name: "Owner", email: "owner@example.test", emailVerified: true, role: "customer" }, session: { id: "2", userId: "1", expiresAt: "2099-01-01T00:00:00Z" } };
const connected = { id: "3", revision: 1, status: "connected", authMethod: "api-key", organizationID: "org-test", neonProjectID: null as string | null, connectedAt: "2026-11-01T00:00:00Z" };
type Command = Record<string, unknown> & { action: string };
async function flush(action: () => void = () => undefined) { await act(async () => { action(); await new Promise(resolve => setTimeout(resolve, 0)); }); }
async function until(check: () => boolean) { for (let i = 0; i < 60; i++) { if (check()) return; await flush(); } assert.ok(check(), "UI did not settle"); }
function fixture(override?: (command: Command) => Response | Promise<Response> | undefined) {
  const browser = new Window({ url: "https://console.example/workspaces/10/connections" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, document: browser.document, HTMLElement: browser.HTMLElement, HTMLInputElement: browser.HTMLInputElement, HTMLFormElement: browser.HTMLFormElement, Event: browser.Event, FormData: browser.FormData, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const calls: Omit<Command, "apiKey">[] = [];
  let value: typeof connected | null = null, canManage = true;
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://console.example", enabledFeatures: [] }, async (_url, init) => {
    assert.equal(typeof init?.body, "string");
    const command = JSON.parse(init?.body as string) as Command;
    const safe = { ...command }; delete safe.apiKey; calls.push(safe);
    const custom = override?.(command); if (custom) return custom;
    if (command.action === "connect") value = { ...connected, revision: (value?.revision ?? 0) + 1, neonProjectID: typeof command.neonProjectID === "string" ? command.neonProjectID : null };
    if (command.action === "disconnect") value = { ...connected, revision: 2, status: "disconnected" };
    return Response.json({ data: { connection: value, canManage, ...(command.action === "projects" ? { projects: [{ id: value?.neonProjectID ?? (command.cursor ? "neon-2" : "neon-1"), name: value?.neonProjectID ? "Scoped project" : command.cursor ? "Second project" : "First project", regionId: "aws-eu-central-1" }], nextCursor: value?.neonProjectID || command.cursor ? null : "page-2" } : {}) } });
  });
  return { host, calls, client, setConnected() { value = connected; }, setCanManage(value: boolean) { canManage = value; },
    async render(id = "10", session = actor) { await flush(() => { root.render(<I18nProvider locale="en" preference="en"><QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><WorkspaceConnections id={id} /></SessionContext.Provider></ApiContext.Provider></QueryClientProvider></I18nProvider>); }); },
    async close() { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); },
  };
}
function button(host: HTMLElement, text: string) { const element = [...host.querySelectorAll("button")].find(b => b.textContent === text); assert(element, `Missing ${text}`); return element; }
function form(host: HTMLElement) { const element = host.querySelector<HTMLFormElement>('form[aria-label="Connect Neon"]'); assert(element); return element; }
function key(host: HTMLElement) { const element = host.querySelector<HTMLInputElement>('input[type="password"]'); assert(element); return element; }
async function submit(host: HTMLElement, value = "fixture-api-key") { key(host).value = value; await flush(() => form(host).dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }))); }
await test("new customer connects with a cleared key and only public metadata in caches", async () => {
  const view = fixture(); try {
    await view.render(); await until(() => !!view.host.querySelector("form"));
    assert.equal(key(view.host).autocomplete, "new-password");
    await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    assert.equal(key(view.host).value, ""); assert.equal(view.calls.find(c => c.action === "connect")?.expectedRevision, 0);
    assert.equal(view.calls.find(c => c.action === "connect")?.neonProjectID, undefined);
    assert(!JSON.stringify(view.client.getQueryCache().getAll()).includes("fixture-api-key"));
    assert(!JSON.stringify(view.client.getMutationCache().getAll()).includes("fixture-api-key"));
    assert.match(view.host.textContent, /Neon bills your own account/);
    assert.equal(view.calls.filter(c => c.action === "projects").length, 0);
  } finally { await view.close(); }
});
await test("failed connect clears the DOM and retains non-secret organization", async () => {
  const view = fixture(command => command.action === "connect" ? Response.json({}, { status: 400 }) : undefined); try {
    await view.render(); await until(() => !!view.host.querySelector("form"));
    const organization = view.host.querySelector<HTMLInputElement>('input[name="organizationID"]'); assert(organization);
    organization.value = "org-retained";
    await submit(view.host); await until(() => view.host.textContent.includes("Neon rejected"));
    assert.equal(key(view.host).value, ""); assert.equal(organization.value, "org-retained");
  } finally { await view.close(); }
});
await test("uncertain retry freezes request identity and refuses a different key", async () => {
  let attempts = 0;
  const view = fixture(command => { if (command.action === "connect" && ++attempts === 1) return Promise.reject(new TypeError("lost response")); return undefined; }); try {
    await view.render(); await until(() => !!view.host.querySelector("form")); await submit(view.host);
    await until(() => view.host.textContent.includes("The result is uncertain"));
    assert.equal(key(view.host).value, ""); await submit(view.host, "different-key");
    await until(() => view.host.textContent.includes("Re-enter the same API key")); assert.equal(attempts, 1);
    await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    const writes = view.calls.filter(c => c.action === "connect"); assert.equal(writes.length, 2); assert.deepEqual(writes[0], writes[1]);
  } finally { await view.close(); }
});
await test("conflict refreshes metadata before a deliberate new revision request", async () => {
  let attempts = 0;
  const view = fixture(command => { if (command.action === "connect" && ++attempts === 1) return Response.json({}, { status: 409 }); return undefined; }); try {
    await view.render(); await until(() => !!view.host.querySelector("form")); view.setConnected(); await submit(view.host);
    await until(() => view.host.textContent.includes("The connection changed")); await until(() => view.host.textContent.includes("Replace connection key"));
    await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    const writes = view.calls.filter(c => c.action === "connect"); assert.equal(writes[1]?.expectedRevision, 1); assert.notEqual(writes[0]?.idempotencyKey, writes[1].idempotencyKey);
  } finally { await view.close(); }
});
await test("disconnect requires confirmation, supports cancel, and renders tombstone", async () => {
  const view = fixture(); view.setConnected(); try {
    await view.render(); await until(() => view.host.textContent.includes("Disconnect Neon"));
    await flush(() => { button(view.host, "Disconnect Neon").click(); }); assert.match(view.host.textContent, /projects and data remain intact/);
    await flush(() => { button(view.host, "Cancel").click(); }); assert.equal(view.calls.filter(c => c.action === "disconnect").length, 0);
    await flush(() => { button(view.host, "Disconnect Neon").click(); }); await flush(() => { button(view.host, "Confirm disconnect").click(); });
    await until(() => view.host.textContent.includes("Neon disconnected.")); assert.match(view.host.textContent, /Disconnected/); assert.equal(view.calls.find(c => c.action === "disconnect")?.expectedRevision, 1);
  } finally { await view.close(); }
});
await test("inventory is explicit, bounded to one cursor page and provider failure has no fallback", async () => {
  let fail = false;
  const view = fixture(command => fail && command.action === "projects" ? Response.json({}, { status: 503 }) : undefined); view.setConnected(); try {
    await view.render(); await until(() => view.host.textContent.includes("Refresh projects")); await flush(() => { button(view.host, "Refresh projects").click(); });
    await until(() => view.host.textContent.includes("First project")); await flush(() => { button(view.host, "Next project page").click(); });
    await until(() => view.host.textContent.includes("Second project")); assert(!view.host.textContent.includes("First project")); assert.equal(view.calls.filter(c => c.action === "projects").at(-1)?.cursor, "page-2");
    fail = true; await flush(() => { button(view.host, "Refresh projects").click(); }); await until(() => view.host.textContent.includes("Neon inventory is unavailable")); assert(!view.host.textContent.includes("First project")); assert(!view.host.textContent.includes("Second project"));
  } finally { await view.close(); }
});
await test("members and read-only previews never get mutation or provider controls", async () => {
  for (const preview of [false, true]) {
    const view = fixture(); view.setConnected(); view.setCanManage(preview); try {
      await view.render("10", { ...actor, user: { ...actor.user, role: preview ? "operator" : "member", preview } }); await until(() => view.host.textContent.includes("Read-only view"));
      assert.equal(view.host.querySelector("form"), null); assert.equal(view.host.querySelector("button"), null); assert.deepEqual(view.calls.map(c => c.action), ["get"]);
    } finally { await view.close(); }
  }
});
await test("switching customer drops secret drafts and ignores late previous-customer results", async () => {
  let resolve!: (response: Response) => void;
  const view = fixture(command => command.action === "connect" ? new Promise<Response>(done => { resolve = done; }) : undefined); try {
    await view.render(); await until(() => !!view.host.querySelector("form")); await submit(view.host);
    await view.render("20"); await until(() => !!view.host.querySelector("form")); assert.equal(key(view.host).value, "");
    resolve(Response.json({ data: { connection: connected, canManage: true } })); await flush();
    assert(!view.host.textContent.includes("Neon connected.")); assert.match(view.host.textContent, /Disconnected/);
    assert(!view.client.getQueryCache().getAll().some(q => String(q.queryKey[1]).includes(',"10"]')));
    key(view.host).value = "unsent-draft"; await view.render("30"); await until(() => !!view.host.querySelector("form")); assert.equal(key(view.host).value, "");
  } finally { await view.close(); }
});

await test("a malformed receipt keeps the original retry identity instead of claiming success", async () => {
  let attempts = 0;
  const view = fixture(command => {
    if (command.action === "connect" && ++attempts === 1) return Response.json({ data: { canManage: true, connection: { ...connected, revision: 99 } } });
    return undefined;
  }); try {
    await view.render(); await until(() => !!view.host.querySelector("form")); await submit(view.host);
    await until(() => view.host.textContent.includes("The result is uncertain")); assert(!view.host.textContent.includes("Neon connected."));
    await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    const writes = view.calls.filter(c => c.action === "connect"); assert.deepEqual(writes[0], writes[1]);
  } finally { await view.close(); }
});
await test("a forbidden initial read never renders a secret field", async () => {
  const view = fixture(() => Response.json({}, { status: 403 })); try {
    await view.render(); await until(() => view.host.textContent.includes("Workspace administrator access required"));
    assert.equal(view.host.querySelector("input"), null); assert.deepEqual(view.calls.map(c => c.action), ["get"]);
  } finally { await view.close(); }
});
await test("a draft is wiped even from a detached input when changing customers", async () => {
  const view = fixture(); try {
    await view.render(); await until(() => !!view.host.querySelector("form")); const prior = key(view.host); prior.value = "unsent-private-draft";
    await view.render("20"); await until(() => !!view.host.querySelector("form")); assert.equal(prior.value, ""); assert.equal(key(view.host).value, "");
  } finally { await view.close(); }
});
await test("disconnect uncertain retry preserves the exact revision and request ID", async () => {
  let attempts = 0;
  const view = fixture(command => {
    if (command.action === "disconnect" && ++attempts === 1) return Promise.reject(new TypeError("lost response"));
    return undefined;
  }); view.setConnected(); try {
    await view.render(); await until(() => view.host.textContent.includes("Disconnect Neon"));
    await flush(() => { button(view.host, "Disconnect Neon").click(); }); await flush(() => { button(view.host, "Confirm disconnect").click(); });
    await until(() => view.host.textContent.includes("Retry same disconnection")); assert.equal(view.host.querySelector("form"), null);
    await flush(() => { button(view.host, "Retry same disconnection").click(); }); await until(() => view.host.textContent.includes("Neon disconnected."));
    const writes = view.calls.filter(c => c.action === "disconnect"); assert.deepEqual(writes[0], writes[1]);
  } finally { await view.close(); }
});

await test("a project-scoped key sends the exact project ID and reads a singleton inventory", async () => {
  const view = fixture(); try {
    await view.render(); await until(() => !!view.host.querySelector("form"));
    const scope = view.host.querySelector<HTMLInputElement>('input[name="neonProjectID"]'); assert(scope); scope.value = "neon-project-selected";
    await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    assert.equal(view.calls.find(c => c.action === "connect")?.neonProjectID, "neon-project-selected");
    assert.equal(scope.value, "neon-project-selected"); assert.match(view.host.textContent, /Neon project scope: neon-project-selected/);
    await flush(() => { button(view.host, "Refresh projects").click(); }); await until(() => view.host.textContent.includes("Scoped project"));
    assert(![...view.host.querySelectorAll("button")].some(b => b.textContent === "Next project page"));
    assert.equal(view.calls.find(c => c.action === "projects")?.cursor, undefined);
  } finally { await view.close(); }
});
await test("an uncertain connection cannot reuse its request ID with a changed project scope", async () => {
  let attempts = 0;
  const view = fixture(command => { if (command.action === "connect" && ++attempts === 1) return Promise.reject(new TypeError("lost response")); return undefined; }); try {
    await view.render(); await until(() => !!view.host.querySelector("form"));
    const scope = view.host.querySelector<HTMLInputElement>('input[name="neonProjectID"]'); assert(scope); scope.value = "original-project";
    await submit(view.host); await until(() => view.host.textContent.includes("The result is uncertain")); assert(scope.disabled);
    scope.value = "different-project"; await submit(view.host); await until(() => view.host.textContent.includes("keep the original organization and Neon project IDs")); assert.equal(attempts, 1); assert.equal(key(view.host).value, "");
    scope.value = "original-project"; await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    const writes = view.calls.filter(c => c.action === "connect"); assert.deepEqual(writes[0], writes[1]); assert.equal(writes[1]?.neonProjectID, "original-project");
  } finally { await view.close(); }
});
await test("a definitive provider rejection explains project scope and permits a corrected new request", async () => {
  let attempts = 0;
  const view = fixture(command => { if (command.action === "connect" && ++attempts === 1) return Response.json({ error: "Invalid Neon credentials or scope." }, { status: 400 }); return undefined; }); try {
    await view.render(); await until(() => !!view.host.querySelector("form"));
    const scope = view.host.querySelector<HTMLInputElement>('input[name="neonProjectID"]'); assert(scope); scope.value = "wrong-project";
    await submit(view.host); await until(() => view.host.textContent.includes("Neon rejected this request"));
    assert.match(view.host.textContent, /Check your API key, Neon project ID and permissions/); assert.equal(key(view.host).value, ""); assert.equal(scope.value, "wrong-project"); assert(!scope.disabled);
    scope.value = "correct-project"; await submit(view.host); await until(() => view.host.textContent.includes("Neon connected."));
    const writes = view.calls.filter(c => c.action === "connect"); assert.notEqual(writes[0]?.idempotencyKey, writes[1]?.idempotencyKey); assert.equal(writes[1]?.neonProjectID, "correct-project");
  } finally { await view.close(); }
});
