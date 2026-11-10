import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { I18nProvider } from "@spitzli/i18n/react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, defaultScheduler, notifyManager } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { ApiClient } from "../src/api";
import { ApiContext } from "../src/context";
import Neon from "../../../plugins/neon/ui/index";
import type { ResourceProps } from "../src/plugins/sdk";

notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const secret = "postgresql://synthetic:fixture-secret@ep-one.neon.tech/synthetic?sslmode=require";
const panel = { version: 2, title: "Synthetic database", blocks: [{ kind: "status", label: "Compute", state: "healthy", text: "Neon compute is active" }, { kind: "table", title: "Connection settings", columns: [{ key: "field", label: "Setting" }, { key: "value", label: "Value" }], rows: [{ field: "Database", value: "Database" }] }] };
async function flush(run: () => void = () => undefined) { await act(async () => { run(); await new Promise(resolve => setTimeout(resolve, 0)); }); }
async function until(check: () => boolean) { for (let i = 0; i < 50; i++) { if (check()) return; await flush(); } assert.ok(check(), "Neon UI did not settle"); }
function button(host: HTMLElement, text: string) { return [...host.querySelectorAll("button")].find(x => x.textContent === text); }
function click(host: HTMLElement, text: string) { const b = button(host, text); assert.ok(b, "Missing button: " + text); b.click(); }
function cache(client: QueryClient) { return JSON.stringify([client.getQueryCache().getAll().map(x => x.state), client.getMutationCache().getAll().map(x => x.state)]); }
function setup(options: { failedView?: boolean; pendingSecret?: boolean; writable?: boolean; locale?: "en" | "de" } = {}) {
  const browser = new Window({ url: "https://console.example" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, document: browser.document, HTMLElement: browser.HTMLElement, Event: browser.Event, IS_REACT_ACT_ENVIRONMENT: true });
  let hidden = false, signal: AbortSignal | null | undefined, deleted = 0;
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: () => Promise.resolve(undefined) } });
  let resolveSecret: ((value: Response) => void) | undefined;
  const pending = new Promise<Response>(resolve => { resolveSecret = resolve; });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false, gcTime: Infinity } } });
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://console.example", enabledFeatures: [] }, (_url, init) => {
    assert.ok(init && typeof init.body === "string"); const body = JSON.parse(init.body) as { action: string };
    if (body.action === "databases.view") return Promise.resolve(options.failedView ? Response.json({ error: "Provider unavailable" }, { status: 503 }) : Response.json(panel));
    if (body.action === "databases.branches") return Promise.resolve(Response.json({ version: 2, title: "Branches", blocks: [] }));
    if (body.action === "databases.credentials") { signal = init.signal; return options.pendingSecret ? pending : Promise.resolve(Response.json({ value: secret })); }
    if (body.action === "databases.detach") return Promise.resolve(Response.json({ detached: true, resource_id: "bound-1" }));
    throw Error("Unexpected fixture action: " + body.action);
  });
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
  const props: ResourceProps = { plugin: "neon", projectID: "201", customer: "101", resource: { id: "bound-1", offering: "database", name: "Synthetic database", createdVia: "self-service" }, features: ["connect-databases"], writable: options.writable ?? true, actions: ["databases.view", "databases.branches", "databases.credentials", "databases.detach"], onDeleted: () => { deleted++; } };
  const Resource = Neon.Resource;
  return { host, client, browser, get signal() { return signal; }, get deleted() { return deleted; },
    render: () => flush(() => { root.render(<QueryClientProvider client={client}><I18nProvider locale={options.locale ?? "en"} preference={options.locale ?? "en"}><ApiContext.Provider value={api}><h1>{props.resource.name}</h1><Resource {...props} /></ApiContext.Provider></I18nProvider></QueryClientProvider>); }),
    hide: () => flush(() => { hidden = true; document.dispatchEvent(new Event("visibilitychange")); }),
    show: () => flush(() => { hidden = false; document.dispatchEvent(new Event("visibilitychange")); }),
    complete: () => flush(() => { assert.ok(resolveSecret); resolveSecret(Response.json({ value: secret })); }),
    close: async () => { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); },
  };
}

await test("Neon attachment can detach locally when its provider view is unavailable", async () => {
  const f = setup({ failedView: true });
  try { await f.render(); await until(() => f.client.getQueryCache().getAll().some(x => x.state.status === "error"));
    assert.ok(button(f.host, "Detach database")); assert.ok(!button(f.host, "Reveal URI"));
    await flush(() => { click(f.host, "Detach database"); }); await flush(() => { click(f.host, "Confirm detach"); }); await until(() => f.deleted === 1);
  } finally { await f.close(); }
});

await test("Hiding during a pending Neon reveal cancels it and discards a late URI", async () => {
  const f = setup({ pendingSecret: true });
  try { await f.render(); await until(() => !!button(f.host, "Reveal URI")); await flush(() => { click(f.host, "Reveal URI"); }); assert.equal(f.signal?.aborted, false);
    await f.hide(); assert.equal(f.signal.aborted, true); await f.complete(); await f.show();
    assert.ok(!f.host.textContent.includes(secret)); assert.ok(!cache(f.client).includes(secret)); assert.ok(button(f.host, "Reveal URI"));
  } finally { await f.close(); }
});

await test("Copy does not extend the original Neon URI disclosure deadline", async () => {
  const f = setup(); const deadlines: (() => void)[] = []; const original = window.setTimeout.bind(window);
  window.setTimeout = ((callback: TimerHandler, delay?: number, ...args: unknown[]) => {
    if (delay === 60_000) { assert.equal(typeof callback, "function"); deadlines.push(callback as () => void); return 777; }
    return original(callback, delay, ...args);
  }) as typeof window.setTimeout;
  try { await f.render(); await until(() => !!button(f.host, "Reveal URI")); await flush(() => { click(f.host, "Reveal URI"); }); await until(() => f.host.textContent.includes(secret));
    assert.equal(deadlines.length, 1); await flush(() => { click(f.host, "Copy URI"); }); assert.equal(deadlines.length, 1, "Copy restarted the reveal deadline");
    await flush(() => { deadlines[0]?.(); }); assert.ok(!f.host.textContent.includes(secret)); assert.ok(!cache(f.client).includes(secret));
  } finally { await f.close(); }
});

await test("Read-only Neon resource view exposes no reveal or detach controls", async () => {
  const f = setup({ writable: false });
  try { await f.render(); await until(() => f.host.textContent.includes("Connection settings")); assert.ok(!button(f.host, "Reveal URI")); assert.ok(!button(f.host, "Detach database")); }
  finally { await f.close(); }
});

await test("German Neon resource translates controls and panel metadata while preserving customer data", async () => {
  const f = setup({ locale: "de" });
  try {
    await f.render(); await until(() => !!button(f.host, "URI anzeigen"));
    assert.ok(f.host.textContent.includes("Verbindungseinstellungen"));
    assert.ok(f.host.textContent.includes("Neon-Compute ist aktiv"));
    assert.ok(f.host.textContent.includes("Datenbank"));
    assert.ok(f.host.textContent.includes("Synthetic database"));
    assert.equal(f.host.querySelector("td:nth-child(2)")?.textContent, "Database");
    assert.ok(f.host.querySelector('section[aria-label="Verbindungs-URI"]'));
    await flush(() => { click(f.host, "URI anzeigen"); });
    await until(() => f.host.textContent.includes(secret));
    assert.ok(button(f.host, "URI kopieren"));
    await flush(() => { click(f.host, "URI ausblenden"); });
    assert.ok(!f.host.textContent.includes(secret));
    await flush(() => { click(f.host, "Datenbankverknüpfung entfernen"); });
    assert.ok(button(f.host, "Verknüpfung entfernen bestätigen"));
    assert.ok(button(f.host, "Abbrechen"));
  } finally { await f.close(); }
});
