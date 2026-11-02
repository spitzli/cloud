import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, defaultScheduler, notifyManager } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { ApiClient } from "../src/api";
import { ApiContext } from "../src/context";
import { usePluginReveal } from "../src/plugins/sdk";

notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const secret = "private-fixture-app-key-never-in-shared-cache";
async function flush(run: () => void) {
  await act(async () => { run(); await Promise.resolve(); });
  await act(async () => { await new Promise((done) => setTimeout(done, 0)); });
}
function View({ project, resource }: { project: string; resource: string }) {
  const reveal = usePluginReveal({ plugin: "object-storage", projectID: project, resource: { id: resource } }, "credentials.reveal");
  return <>
    <button type="button" onClick={() => { reveal.mutate({ resource_id: resource }); }}>Reveal</button>
    <button type="button" onClick={() => { reveal.reset(); }}>Hide</button>
    <output>{reveal.data ? JSON.stringify(reveal.data) : "hidden"}</output>
  </>;
}
function setup(fetcher: typeof fetch) {
  const browser = new Window({ url: "https://console.example" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, document: browser.document, HTMLElement: browser.HTMLElement, Event: browser.Event, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false, gcTime: Infinity } } });
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://console.example", enabledFeatures: [] }, fetcher);
  return {
    host, client,
    render: (project: string, resource = "resource") => flush(() => { root.render(<QueryClientProvider client={client}><ApiContext.Provider value={api}><View project={project} resource={resource} /></ApiContext.Provider></QueryClientProvider>); }),
    close: async () => { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); },
  };
}
function click(host: HTMLElement, text: string) {
  const button = [...host.querySelectorAll("button")].find((item) => item.textContent === text);
  assert.ok(button);
  button.click();
}
function sharedCache(client: QueryClient) {
  return JSON.stringify([client.getQueryCache().getAll().map((item) => item.state), client.getMutationCache().getAll().map((item) => item.state)]);
}
function deferredResponse() {
  let complete: ((value: Response) => void) | undefined;
  const promise = new Promise<Response>((done) => { complete = done; });
  return { promise, resolve: (value: Response) => { assert.ok(complete); complete(value); } };
}

await test("revealed credentials stay out of both shared caches and disappear on Hide and project changes", async () => {
  const fixture = setup(() => Promise.resolve(Response.json({ value: secret })));
  try {
    await fixture.render("100");
    await flush(() => { click(fixture.host, "Reveal"); });
    assert.match(fixture.host.textContent, new RegExp(secret));
    assert.ok(!sharedCache(fixture.client).includes(secret), "credential persisted in shared query/mutation cache");
    await flush(() => { click(fixture.host, "Hide"); });
    assert.ok(!fixture.host.textContent.includes(secret));
    assert.ok(!sharedCache(fixture.client).includes(secret));
    await flush(() => { click(fixture.host, "Reveal"); });
    await fixture.render("101");
    assert.ok(!fixture.host.textContent.includes(secret), "previous project credential leaked into next project");
  } finally { await fixture.close(); }
});

await test("Hide aborts an in-flight reveal and a late response cannot restore or cache its secret", async () => {
  const pending = deferredResponse();
  let signal: AbortSignal | null | undefined;
  const fixture = setup((_, input) => { signal = input?.signal; return pending.promise; });
  try {
    await fixture.render("100");
    await flush(() => { click(fixture.host, "Reveal"); });
    await flush(() => { click(fixture.host, "Hide"); });
    assert.equal(signal?.aborted, true);
    await flush(() => { pending.resolve(Response.json({ value: secret })); });
    assert.ok(!fixture.host.textContent.includes(secret));
    assert.ok(!sharedCache(fixture.client).includes(secret));
  } finally { await fixture.close(); }
});

await test("a resource change cancels the prior reveal before its late result can reach the next resource", async () => {
  const pending = deferredResponse();
  let signal: AbortSignal | null | undefined;
  const fixture = setup((_, input) => { signal = input?.signal; return pending.promise; });
  try {
    await fixture.render("100", "resource-a");
    await flush(() => { click(fixture.host, "Reveal"); });
    await fixture.render("100", "resource-b");
    assert.equal(signal?.aborted, true);
    await flush(() => { pending.resolve(Response.json({ value: secret })); });
    assert.ok(!fixture.host.textContent.includes(secret));
    assert.ok(!sharedCache(fixture.client).includes(secret));
  } finally { await fixture.close(); }
});
