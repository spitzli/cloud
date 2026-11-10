import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, notifyManager, defaultScheduler } from "@tanstack/react-query";
import { I18nProvider } from "@spitzli/i18n/react";
import { commandSchema } from "@spitzli/hosting-contracts";
import { Window } from "happy-dom";
import { ApiClient } from "../src/api";
import { ApiContext } from "../src/context";
import { WorkspaceUsage } from "../src/Usage";

notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const extensions = {
  plan: null, overrides: {}, limits: { projects: 2 }, allocated: { projects: 1 },
  meters: [{ id: "projects", label: "Projects", unit: "count" }], units: [], history: [],
};
const native = {
  effective: { apps: 4, cpuMillicores: 800, memoryBytes: 512000000, volumeBytes: null, ephemeralBytes: 1000000000, replicasPerApp: 3, concurrentDeployments: 2 },
  allocated: { apps: 2, cpuMillicores: 300, memoryBytes: 256000000, volumeBytes: 0, ephemeralBytes: 64000000, replicasPerApp: 2, concurrentDeployments: 0 },
  reserved: { apps: 1, cpuMillicores: 200, memoryBytes: 64000000, volumeBytes: 0, ephemeralBytes: 32000000, replicasPerApp: 3, concurrentDeployments: 1 },
  subscriptionRevision: 3, limits: [], measured: null, checkedAt: null, enforcement: "webdock-enforced", overallocated: false,
};
async function flush(action: () => void = () => undefined) { await act(async () => { action(); await new Promise(resolve => setTimeout(resolve, 0)); }); }
async function until(check: () => boolean) { for (let n = 0; n < 100; n++) { if (check()) return; await flush(); } assert.ok(check(), "Usage UI did not settle"); }
function fixture(options: { enabled?: boolean; status?: number; body?: unknown; extensionStatus?: number; holdSecond?: boolean } = {}) {
  const browser = new Window({ url: "https://console.example/workspaces/101/usage" });
  Object.assign(globalThis, { window: browser, document: browser.document, HTMLElement: browser.HTMLElement, Event: browser.Event, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host), client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const calls: string[] = [];
  let release: (() => void) | undefined;
  const api = new ApiClient({ apiOrigin: "https://api.example", authOrigin: "https://auth.example", enabledFeatures: options.enabled === false ? [] : ["hosting"] }, async (input, init) => {
    assert.ok(input instanceof URL); assert.equal(init?.credentials, "include");
    if (input.pathname.startsWith("/api/console/customers/")) return Response.json(extensions, { status: options.extensionStatus ?? 200 });
    assert.equal(input.pathname, "/api/console/hosting"); assert.equal(init.method, "POST");
    const command = commandSchema.parse(JSON.parse(typeof init.body === "string" ? init.body : "{}"));
    assert.ok(command.action === "usage.get");
    calls.push(command.customerID);
    if (options.holdSecond && command.customerID === "102") await new Promise<void>(resolve => { release = resolve; });
    return Response.json(options.body ?? native, { status: options.status ?? 200 });
  });
  return { host, client, calls, options, release: () => { release?.(); }, render: async (customer = "101") => { await flush(() => { root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><I18nProvider locale="en" preference="en"><WorkspaceUsage id={customer} /></I18nProvider></ApiContext.Provider></QueryClientProvider>); }); }, close: async () => { release?.(); await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); } };
}
function nativeSection(host: ParentNode) { const section = host.querySelector('[aria-label="Application hosting"]'); assert.ok(section, "Application hosting section is missing"); return section; }
function row(section: ParentNode, name: string) { const value = [...section.querySelectorAll("tbody tr")].find(row => row.querySelector("th")?.textContent === name); assert.ok(value, `Missing native hosting row ${name}`); return [...value.querySelectorAll("td")].map(cell => cell.textContent); }

await test("native-hosting-only workspaces show returned allowances and pending reservations separately from empty extension workloads", async () => {
  const view = fixture();
  try {
    await view.render(); await until(() => view.host.querySelector('[aria-label="Application hosting"] table') !== null);
    const section = nativeSection(view.host);
    assert.deepEqual(view.calls, ["101"]);
    assert.deepEqual(row(section, "Hosting apps"), ["2", "1", "4"]);
    assert.deepEqual(row(section, "CPU cores"), ["0.3", "0.2", "0.8"]);
    assert.deepEqual(row(section, "Memory"), ["256 MB", "64 MB", "512 MB"]);
    assert.deepEqual(row(section, "Persistent storage"), ["0 MB", "0 MB", "No customer limit"]);
    assert.deepEqual(row(section, "Replicas per app"), ["2", "3", "3"]);
    assert.deepEqual(row(section, "Concurrent deployments"), ["0", "1", "2"]);
    assert.ok(section.textContent.includes("Project or provider limits may be stricter."));
    assert.ok(section.textContent.includes("not live measurements"));
    assert.equal(section.textContent.includes("Memory used"), false);
    const extensionSection = view.host.querySelector('[aria-label="Extension plan and workload usage"]'); assert.ok(extensionSection);
    assert.ok(extensionSection.textContent.includes("No service usage has been reported yet."));
  } finally { await view.close(); }
});

await test("hosting-disabled installations never request native hosting usage", async () => {
  const view = fixture({ enabled: false });
  try { await view.render(); await until(() => view.host.textContent.includes("No service usage has been reported yet.")); assert.deepEqual(view.calls, []); assert.equal(view.host.querySelector('[aria-label="Application hosting"]'), null); } finally { await view.close(); }
});

await test("denied, unavailable and malformed native reads cannot become zero reservations", async () => {
  for (const options of [{ status: 403 }, { status: 503 }, { body: { ...native, effective: {} } }, { body: { ...native, allocated: { ...native.allocated, memoryBytes: null } } }]) {
    const view = fixture(options);
    try { await view.render(); await until(() => view.host.querySelector('[aria-label="Application hosting"] [role="alert"]') !== null); const section = nativeSection(view.host); assert.equal(section.querySelector("table"), null); assert.ok(view.host.textContent.includes("No service usage has been reported yet.")); } finally { await view.close(); }
  }
});

await test("native hosting remains visible when extension usage is unavailable", async () => {
  const view = fixture({ extensionStatus: 503 });
  try { await view.render(); await until(() => view.host.querySelector('[aria-label="Application hosting"] table') !== null); assert.deepEqual(row(nativeSection(view.host), "Hosting apps"), ["2", "1", "4"]); assert.ok(view.host.querySelector('[aria-label="Extension plan and workload usage"] [role="alert"]')); } finally { await view.close(); }
});

await test("changing workspace hides the previous hosting values while the current scoped read is pending", async () => {
  const view = fixture({ holdSecond: true });
  try { await view.render(); await until(() => view.host.querySelector('[aria-label="Application hosting"] table') !== null); await view.render("102"); await until(() => view.calls.includes("102")); assert.equal(nativeSection(view.host).querySelector("table"), null); assert.ok(nativeSection(view.host).querySelector('[role="status"]')); view.release(); await until(() => view.host.querySelector('[aria-label="Application hosting"] table') !== null); assert.deepEqual(view.calls, ["101", "102"]); } finally { await view.close(); }
});

await test("a denied refresh hides cached hosting values until an explicit fresh retry succeeds", async () => {
  const view = fixture();
  try {
    await view.render(); await until(() => view.host.querySelector('[aria-label="Application hosting"] table') !== null);
    view.options.status = 403;
    await act(async () => { await view.client.invalidateQueries({ queryKey: ["hosting", "usage", "101"] }); });
    await until(() => nativeSection(view.host).querySelector('[role="alert"]') !== null);
    assert.equal(nativeSection(view.host).querySelector("table"), null);
    view.options.status = 200;
    const retry = nativeSection(view.host).querySelector("button"); assert.ok(retry);
    await flush(() => { retry.click(); });
    await until(() => nativeSection(view.host).querySelector("table") !== null);
    assert.deepEqual(view.calls, ["101", "101", "101"]);
    assert.deepEqual(row(nativeSection(view.host), "Hosting apps"), ["2", "1", "4"]);
  } finally { await view.close(); }
});
