import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, defaultScheduler, notifyManager } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { WorkspaceApiTokens } from "../src/ApiTokens";
notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const actor: Session = { user: { id: "1", name: "Owner", email: "owner@example.test", emailVerified: true, role: "user" }, session: { id: "2", userId: "1", expiresAt: "2099-01-01T00:00:00Z" } };
const secret = "private-workspace-token-only-in-current-view";
const made = { id: "600", name: "CI deploy", canWrite: false, createdAt: "2026-11-11T00:00:00Z", expiresAt: "2099-01-01T00:00:00Z", lastUsedAt: null, createdBy: "Owner", token: secret };
async function flush(action: () => void = () => undefined) { await act(async () => { action(); await new Promise(done => setTimeout(done, 0)); }); }
async function until(check: () => boolean) { for (let i = 0; i < 60 && !check(); i++) await flush(); assert.ok(check(), "UI did not settle"); }
function fixture(post?: (init: RequestInit) => Promise<Response>) {
  const browser = new Window({ url: "https://console.example/workspaces/10/api-tokens" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, document: browser.document, HTMLElement: browser.HTMLElement, Event: browser.Event, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false, gcTime: Infinity } } });
  let posts = 0;
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://accounts.example", enabledFeatures: [] }, async (_, init) => {
    if (init?.method === "POST") { posts++; return post ? post(init) : Response.json(made); }
    return Response.json([]);
  });
  const setRole = (role: string) => client.setQueryData(["tenants"], [{ id: "10", name: "First", role }, { id: "11", name: "Second", role }]);
  setRole("owner");
  return { host, client, setRole, posts: () => posts,
    async render(id = "10", session = actor) { await flush(() => { root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><WorkspaceApiTokens id={id} /></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>); }); },
    async close() { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); },
  };
}
const cache = (client: QueryClient) => JSON.stringify([client.getQueryCache().getAll().map(q => q.state), client.getMutationCache().getAll().map(m => m.state)]);
async function create(host: HTMLElement) { await flush(() => { const form = host.querySelector("form"); assert.ok(form); form.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true })); }); }
function click(host: HTMLElement, text: string) { const button = [...host.querySelectorAll("button")].find(item => item.textContent === text); assert.ok(button); button.click(); }
await test("API secrets stay out of shared caches and disappear on Hide, workspace and session changes", async () => {
  const view = fixture(); try {
    await view.render(); await until(() => !!view.host.querySelector("form"));
    await create(view.host); await until(() => view.host.textContent.includes(secret));
    assert.ok(!cache(view.client).includes(secret));
    await flush(() => { click(view.host, "Hide token"); }); assert.ok(!view.host.textContent.includes(secret));
    await create(view.host); await until(() => view.host.textContent.includes(secret));
    await view.render("11"); assert.ok(!view.host.textContent.includes(secret));
    await create(view.host); await until(() => view.host.textContent.includes(secret));
    await view.render("11", { ...actor, session: { ...actor.session, id: "new-session" } });
    assert.ok(!view.host.textContent.includes(secret)); assert.ok(!cache(view.client).includes(secret));
  } finally { await view.close(); }
});
await test("loss of the administrator role aborts creation; late responses cannot restore or cache secrets", async () => {
  let finish: ((value: Response) => void) | undefined; let signal: AbortSignal | null | undefined;
  const view = fixture(init => { signal = init.signal; return new Promise(resolve => { finish = resolve; }); }); try {
    await view.render(); await until(() => !!view.host.querySelector("form")); await create(view.host);
    await flush(() => { view.setRole("member"); });
    await until(() => view.host.textContent.includes("Workspace administrator access required"));
    assert.equal(signal?.aborted, true); assert.ok(finish);
    const complete = finish; await flush(() => { complete(Response.json(made)); });
    await flush(() => { view.setRole("owner"); }); await until(() => !!view.host.querySelector("form"));
    assert.ok(!view.host.textContent.includes(secret)); assert.ok(!cache(view.client).includes(secret)); assert.equal(view.posts(), 1);
  } finally { await view.close(); }
});
await test("customer preview cannot create a workspace automation token", async () => {
  const view = fixture(); try {
    await view.render("10", { ...actor, user: { ...actor.user, preview: true, acting: true } });
    assert.equal(view.host.querySelector("form"), null); assert.equal(view.posts(), 0);
  } finally { await view.close(); }
});
