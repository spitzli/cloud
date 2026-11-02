import { afterEach, expect, mock, test } from "bun:test";
import React, { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Window } from "happy-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

let view = { bucket: "bucket", endpoint: "http://localhost", environment: "staging", revision: 1, state: "active", credentials: [] as string[], observed: null, inventory: null };
const calls: { action: string; input: Record<string, unknown>; core: string | undefined }[] = [];
let fail = true;
let revealWait: Promise<unknown> | undefined;
let revealSignal: AbortSignal | undefined;
let createWait: Promise<unknown> | undefined;
let createSignal: AbortSignal | undefined;

mock.module("../../../apps/console/src/plugins/views", () => ({
  invokePlugin: async (_api: unknown, _plugin: string, _project: string, action: string, input: Record<string, unknown>, _signal?: unknown, core?: string) => {
    if (action === "buckets.view") return view;
    if (action === "credentials.reveal") {
      revealSignal = _signal as AbortSignal;
      calls.push({ action, input: structuredClone(input), core });
      return revealWait ?? { access_key: input.access_key, secret_key: "ui-private-secret", endpoint: "http://localhost", bucket: "bucket" };
    }
    calls.push({ action, input: structuredClone(input), core });
    if (action === "credentials.create") view = { ...view, revision: 2, credentials: ["known-key"] };
    if (action === "buckets.create" && createWait) { createSignal = _signal as AbortSignal; return createWait; }
    if (fail) throw Error("lost response");
    return { resource: { id: "created-bucket" } };
  },
}));
const { ApiClient } = await import("../../../apps/console/src/api");
const { ApiContext, SessionContext } = await import("../../../apps/console/src/context");
const { default: plugin } = await import("../ui/index");
const window = new Window();
Object.assign(globalThis, { window, document: window.document, HTMLElement: window.HTMLElement, FormData: window.FormData, IS_REACT_ACT_ENVIRONMENT: true });
Object.defineProperty(globalThis, "navigator", { configurable: true, value: { clipboard: { writeText: async () => {} } } });
let root: Root;
const context = { plugin: "object-storage", projectID: "project", customer: "customer", resource: { id: "id", offering: "bucket", name: "files", createdVia: "self-service" as const }, features: ["manage-buckets"], writable: true, onDeleted() {} };
const settle = async () => { for (let i = 0; i < 8; i++) await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)); }); };
function mount(child: React.ReactNode) {
  calls.length = 0; fail = true;
  const container = document.createElement("div"); document.body.append(container);
  root = createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://account.example", enabledFeatures: [] });
  const session = { user: { id: "unit-user", name: "Unit operator", email: "unit@example.test", role: "operator", emailVerified: true }, session: { id: "unit-session", userId: "unit-user", expiresAt: "2099-01-01T00:00:00Z" } };
  return { client, container, session, render: (nextChild = child, nextSession = session) => act(async () => root.render(<ApiContext.Provider value={api}><SessionContext.Provider value={nextSession}><QueryClientProvider client={client}>{nextChild}</QueryClientProvider></SessionContext.Provider></ApiContext.Provider>)) };
}
const button = (container: Element, text: string) => Array.from(container.querySelectorAll("button")).find(item => item.textContent?.includes(text))!;
afterEach(async () => { if (root) await act(async () => root.unmount()); document.body.innerHTML = ""; createWait = undefined; createSignal = undefined; });

test("lost create-key response retries the original action, body and both IDs after refreshed rotation controls", async () => {
  view = { ...view, credentials: [], revision: 1 };
  const { container, render } = mount(<plugin.Resource {...context} />);
  await render(); await settle();
  await act(async () => button(container, "Create application key").click()); await settle();
  expect(calls.length).toBe(1);
  const retry = Array.from(container.querySelectorAll("button")).find(item => item.textContent?.startsWith("Retry"))!;
  await act(async () => retry.click()); await settle();
  expect(calls.length).toBe(2);
  expect(calls[1]).toEqual(calls[0]);
  expect(calls[0]!.core).toBeTruthy(); expect(calls[0]!.input.request_id).toBeTruthy();
});

test("native create form freezes inputs and IDs until retry is discarded", async () => {
  const Create = (plugin as any).Create?.["buckets.create"];
  expect(Create).toBeDefined();
  let created = "";
  const { container, render } = mount(<Create {...context} action="buckets.create" title="Create bucket" resources={[]} onCancel={() => {}} onCreated={(id: string) => { created = id; }} />);
  await render();
  function input(name: string, value: string) {
    const field = container.querySelector(`input[name="${name}"]`) as HTMLInputElement;
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!;
    setter.call(field, value); field.dispatchEvent(new window.Event("input", { bubbles: true }) as never);
  }
  await act(async () => { input("name", "Files"); input("environment", "staging"); });
  const submit = async () => { await act(async () => container.querySelector("form")!.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }) as never)); await settle(); };
  await submit();
  const first = calls[0]!;
  await act(async () => input("name", "Changed"));
  await submit();
  expect(calls[1]).toEqual(first);
  await act(async () => button(container, "Discard retry").click());
  await submit();
  expect(calls[2]!.core).not.toBe(first.core);
  expect(calls[2]!.input.request_id).not.toBe(first.input.request_id);
  fail = false; await submit();
  expect(calls[3]).toEqual(calls[2]); expect(created).toBe("created-bucket");
});


test.each(["principal", "project"])("pending Create aborts on %s scope change and discards a late successful response", async change => {
  const Create = (plugin as any).Create["buckets.create"];
  const created: string[] = [];
  const props = { ...context, action: "buckets.create", title: "Create bucket", resources: [], onCancel() {}, onCreated: (id: string) => created.push(id) };
  const fixture = mount(<Create {...props} />);
  await fixture.render();
  let finish!: (value: unknown) => void;
  createWait = new Promise(resolve => { finish = resolve; });
  await act(async () => {
    for (const [name, value] of [["name", "Files"], ["environment", "test"]]) {
      const field = fixture.container.querySelector(`input[name="${name}"]`) as HTMLInputElement;
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")!.set!.call(field, value);
      field.dispatchEvent(new window.Event("input", { bubbles: true }) as never);
    }
    fixture.container.querySelector("form")!.dispatchEvent(new window.Event("submit", { bubbles: true, cancelable: true }) as never);
  });
  await settle();
  expect(calls).toHaveLength(1);
  const frozen = structuredClone(calls[0]);
  expect(createSignal?.aborted).toBe(false);
  expect(fixture.container.querySelector("fieldset")!.disabled).toBe(true);
  const session = change === "principal" ? { user: { ...fixture.session.user, id: "other-user" }, session: { ...fixture.session.session, id: "other-session", userId: "other-user" } } : fixture.session;
  await fixture.render(<Create {...props} projectID={change === "project" ? "other-project" : props.projectID} />, session);
  expect(createSignal?.aborted).toBe(true);
  // The transport deliberately ignores cancellation to exercise the stale completion guard.
  await act(async () => finish({ resource: { id: "old-scope-bucket" } }));
  await settle();
  expect(created).toEqual([]);
  expect(calls).toEqual([frozen]);
  expect(fixture.container.querySelector("fieldset")!.disabled).toBe(false);
});

test("credential reveal hides, aborts pending requests and never enters query or mutation caches", async () => {
  view = { ...view, state: "active", credentials: ["key-a"], revision: 1 };
  revealWait = undefined;
  const { client, container, render } = mount(<plugin.Resource {...context} />);
  await render(); await settle();
  const reveal = button(container, "Reveal");
  expect(reveal).toBeDefined();
  await act(async () => reveal.click()); await settle();
  expect(container.textContent).toContain("ui-private-secret");
  expect(JSON.stringify(client.getQueryCache().getAll().map(q => q.state))).not.toContain("ui-private-secret");
  expect(JSON.stringify(client.getMutationCache().getAll().map(m => m.state))).not.toContain("ui-private-secret");
  await act(async () => button(container, "Copy secret key").click());
  expect(container.textContent).toContain("Copy requested");
  await act(async () => button(container, "Hide").click());
  expect(container.textContent).not.toContain("ui-private-secret");
  expect(container.textContent).not.toContain("Copy requested");
  let finish!: (value: unknown) => void;
  revealWait = new Promise(resolve => { finish = resolve; });
  await act(async () => button(container, "Reveal").click()); await settle();
  await act(async () => button(container, "Hide").click());
  expect(revealSignal?.aborted).toBe(true);
  await act(async () => finish({ access_key: "key-a", secret_key: "late-secret", endpoint: "http://localhost", bucket: "bucket" }));
  await settle(); expect(container.textContent).not.toContain("late-secret");
  revealWait = undefined;
});

test("resource, credential and writable-feature changes clear mounted reveals", async () => {
  view = { ...view, state: "active", credentials: ["key-a"], revision: 1 };
  const { container, render } = mount(<plugin.Resource {...context} />);
  await render(); await settle();
  expect(button(container, "Reveal")).toBeDefined();
  await act(async () => button(container, "Reveal").click()); await settle();
  view = { ...view, credentials: ["key-b"] };
  await act(async () => button(container, "Refresh status").click()); await settle();
  expect(container.textContent).not.toContain("ui-private-secret");
  await act(async () => button(container, "Reveal").click()); await settle();
  expect(container.textContent).toContain("ui-private-secret");
  // Resource identity changes must clear before painting the next bucket.
  await act(async () => root.render(<ApiContext.Provider value={{} as never}><QueryClientProvider client={new QueryClient()}><plugin.Resource {...context} resource={{ ...context.resource, id: "other" }} /></QueryClientProvider></ApiContext.Provider>));
  expect(container.textContent).not.toContain("ui-private-secret");
  await act(async () => root.render(<ApiContext.Provider value={{} as never}><QueryClientProvider client={new QueryClient()}><plugin.Resource {...context} features={[]} /></QueryClientProvider></ApiContext.Provider>));
  await settle(); expect(button(container, "Reveal")).toBeUndefined();
});
