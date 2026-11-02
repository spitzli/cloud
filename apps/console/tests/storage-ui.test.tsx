import assert from "node:assert/strict";
import { test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { createHash } from "node:crypto";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { usePluginStorage } from "../src/plugins/storage";
const content = "scoped binary content", hash = createHash("sha256").update(content).digest("hex"), id = "cf1e1be0-3ab9-47e8-8e6c-cc9c94e81e24";
const context = { plugin: "object-storage", customer: "100", projectID: "200", resource: { id: "resource", name: "Files", offering: "bucket", createdVia: "operator" as const }, writable: true, features: ["manage-buckets"], actions: ["objects.observe", "objects.upload", "objects.download"], onDeleted() { assert.fail("Transfers must not delete the resource"); } };
const session: Session = { user: { id: "unit", name: "Unit operator", email: "unit@example.test", role: "operator", emailVerified: true }, session: { id: "unit-session", userId: "unit", expiresAt: "2099-01-01T00:00:00Z" } };
function View({ project, writable = true }: { project: string; writable?: boolean }) {
  const storage = usePluginStorage({ ...context, projectID: project, writable, features: writable ? ["manage-buckets"] : [] });
  return <><button onClick={() => { void storage.observe("proof.txt"); }}>Inspect</button><button onClick={() => { void storage.download(); }}>Download</button><button onClick={() => { void storage.upload(new File([content], "proof.txt", { type: "text/plain" }), "proof.txt"); }}>Upload</button><button onClick={() => { storage.reset(); }}>Hide</button><output>{storage.status}</output>{storage.url ? <a href={storage.url}>Save</a> : null}</>;
}
async function flush() { await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)); }); }
await test("binary state never enters query/mutation caches; object URLs revoke on hide and principal/project changes", async () => {
  const browser = new Window(); Object.assign(globalThis, { window: browser, document: browser.document, HTMLElement: browser.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
  const originalCreate = Object.getOwnPropertyDescriptor(URL, "createObjectURL"), originalRevoke = Object.getOwnPropertyDescriptor(URL, "revokeObjectURL");
  assert(originalCreate); assert(originalRevoke);
  const revoked: string[] = []; let serial = 0;
  URL.createObjectURL = () => "blob:owned-" + String(++serial); URL.revokeObjectURL = value => { revoked.push(value); };
  const client = new QueryClient(), host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://account.example", enabledFeatures: [] }, (url, init) => {
    if ((url instanceof Request ? url.url : url.toString()).includes("/storage/")) return Promise.resolve(new Response(content, { headers: { "content-type": "text/plain" } }));
    assert(typeof init?.body === "string");
    const body = JSON.parse(init.body) as { action: string };
    if (body.action === "objects.observe") return Promise.resolve(Response.json({ key: "proof.txt", exact_bytes: content.length, filename: "proof.txt", content_type: "text/plain", version_id: "null", if_match: '"abc"', expected_sha256: hash }));
    return Promise.resolve(Response.json({ version: 1, id, path: "/api/console/plugins/storage/" + id, method: "GET", expiresAt: Math.floor(Date.now() / 1000) + 60, exactBytes: content.length, expectedSha256: hash, attachment: { filename: "proof.txt", contentType: "text/plain" } }));
  });
  const render = async (project: string, actor = session, writable = true) => { await act(() => { root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={actor}><View project={project} writable={writable} /></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>); return Promise.resolve(); }); };
  const click = async (name: string) => { const button = [...host.querySelectorAll("button")].find(node => node.textContent === name); assert(button); await act(() => { button.click(); return Promise.resolve(); }); for (let n = 0; n < 5; n++) await flush(); };
  const cache = () => JSON.stringify([client.getQueryCache().getAll().map(q => q.state), client.getMutationCache().getAll().map(m => m.state)]);
  try {
    await render("200"); await click("Inspect"); await click("Download"); assert.equal(host.querySelector("a")?.href, "blob:owned-1");
    assert(!cache().includes(id)); assert(!cache().includes(content));
    await click("Hide"); assert(revoked.includes("blob:owned-1")); assert.equal(host.querySelector("a"), null);
    await click("Inspect"); await click("Download"); await render("201"); assert.equal(host.querySelector("a"), null); assert(revoked.includes("blob:owned-2"));
    await click("Inspect"); await click("Download"); await render("201", { ...session, session: { ...session.session, id: "new-principal-session" } });
    assert.equal(host.querySelector("a"), null); assert(revoked.includes("blob:owned-3"));
    await render("201", session, false); await click("Inspect"); await click("Download"); assert.equal(host.querySelector("a")?.href, "blob:owned-4", "project readers do not receive create-feature discovery flags");
  } finally { await act(() => { root.unmount(); return Promise.resolve(); }); client.clear(); Object.defineProperty(URL, "createObjectURL", originalCreate); Object.defineProperty(URL, "revokeObjectURL", originalRevoke); await browser.happyDOM.abort(); }
});
await test("upload admission binds the file hash and raw bytes once; uncertain upload and readonly scopes never auto-retry", async () => {
  const browser = new Window(); Object.assign(globalThis, { window: browser, document: browser.document, HTMLElement: browser.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
  let admissions = 0, puts = 0;
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://account.example", enabledFeatures: [] }, async (_url, init) => {
    if (init?.method === "PUT") { puts++; assert(init.body instanceof Blob); assert.equal(await init.body.text(), content); throw Error("lost commit response"); }
    admissions++; assert(typeof init?.body === "string"); const body = JSON.parse(init.body) as { input: { expected_sha256: string; exact_bytes: number }; action: string; idempotencyKey: string };
    assert.equal(body.action, "objects.upload"); assert.equal(body.input.expected_sha256, hash); assert.equal(body.input.exact_bytes, content.length); assert(body.idempotencyKey);
    return Response.json({ version: 1, id, path: "/api/console/plugins/storage/" + id, method: "PUT", expiresAt: Math.floor(Date.now() / 1000) + 60, exactBytes: content.length, expectedSha256: hash, attachment: { filename: "proof.txt", contentType: "text/plain" } });
  });
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host), client = new QueryClient();
  const render = async (writable: boolean) => { await act(() => { root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><View project="200" writable={writable} /></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>); return Promise.resolve(); }); };
  const upload = async () => { const button = [...host.querySelectorAll("button")].find(b => b.textContent === "Upload"); assert(button); await act(() => { button.click(); return Promise.resolve(); }); for (let n = 0; n < 6; n++) await flush(); };
  try { await render(true); await upload(); assert(host.textContent.includes("uncertain")); assert.equal(admissions, 1); assert.equal(puts, 1); await flush(); assert.equal(puts, 1); await render(false); await upload(); assert.equal(admissions, 1); assert.equal(puts, 1); assert(!JSON.stringify(client.getMutationCache().getAll()).includes(id)); }
  finally { await act(() => { root.unmount(); return Promise.resolve(); }); client.clear(); await browser.happyDOM.abort(); }
});
await test("scope retirement aborts admission and discards a late opaque descriptor", async () => {
  const browser = new Window(); Object.assign(globalThis, { window: browser, document: browser.document, HTMLElement: browser.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
  let finish!: (value: Response) => void, signal: AbortSignal | null | undefined; let calls = 0;
  const pending = new Promise<Response>(resolve => { finish = resolve; });
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://account.example", enabledFeatures: [] }, (_url, init) => { calls++; signal = init?.signal; return pending; });
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host), client = new QueryClient();
  const render = async (project: string) => { await act(() => { root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><SessionContext.Provider value={session}><View project={project} /></SessionContext.Provider></ApiContext.Provider></QueryClientProvider>); return Promise.resolve(); }); };
  try { await render("200"); const inspect = host.querySelector("button"); assert(inspect); await act(() => { inspect.click(); return Promise.resolve(); }); await render("201"); assert.equal(signal?.aborted, true); finish(Response.json({ private: "late" })); await flush(); assert.equal(calls, 1); assert.equal(host.querySelector("a"), null); assert(!host.textContent.includes("late")); }
  finally { await act(() => { root.unmount(); return Promise.resolve(); }); client.clear(); await browser.happyDOM.abort(); }
});
