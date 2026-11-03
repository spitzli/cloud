import test from "node:test";
import assert from "node:assert/strict";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { Window } from "happy-dom";
import { PluginPanel } from "../src/plugins/Panel";

function dom() {
  const browser = new Window({ url: "https://console.example" });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator, document: browser.document,
    HTMLElement: browser.HTMLElement, Event: browser.Event, MessageEvent: browser.MessageEvent,
    FormData: browser.FormData, IS_REACT_ACT_ENVIRONMENT: true,
  });
  const host = document.createElement("div");
  document.body.append(host);
  return { browser, host, root: createRoot(host) };
}
async function flush(action: () => void) {
  await act(async () => {
    action();
    await Promise.resolve();
  });
}
const submit = (host: HTMLElement) =>
  host.querySelector("form")?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));

await test("secret field is write-only and context merges into action input", async () => {
  const { browser, host, root } = dom();
  const calls: unknown[] = [];
  const value = {
    version: 2,
    title: "Connect",
    blocks: [{
      kind: "form", title: "Account",
      fields: [
        { name: "key", label: "API key", type: "secret", required: true },
        { name: "notify", label: "Notify me", type: "checkbox" },
        { name: "note", label: "Note", type: "textarea" },
      ],
      action: { id: "connect", label: "Connect" },
    }],
  };
  await flush(() => {
    root.render(<PluginPanel value={value} context={{ resource_id: "db-1" }} invoke={(...args) => { calls.push(args); return Promise.resolve({}); }} />);
  });
  const secret = host.querySelector<HTMLInputElement>("input[name=key]");
  assert.ok(secret);
  assert.equal(secret.type, "password");
  assert.equal(secret.value, "");
  assert.equal(secret.getAttribute("autocomplete"), "new-password");
  secret.value = "s3cret";
  host.querySelector<HTMLInputElement>("input[name=notify]")?.click();
  const note = host.querySelector<HTMLTextAreaElement>("textarea[name=note]");
  assert.ok(note);
  note.value = "hi";
  await flush(() => { submit(host); });
  assert.deepEqual(calls, [["connect", { key: "s3cret", notify: true, note: "hi", resource_id: "db-1" }]]);
  assert.equal(secret.value, "", "the secret does not stay in the page after submit");
  assert.doesNotMatch(host.innerHTML, /s3cret/);
  await flush(() => { root.unmount(); });
  await browser.happyDOM.abort();
});

await test("rowLink rows open the resource by its id", async () => {
  const { browser, host, root } = dom();
  const opened: string[] = [];
  const value = {
    version: 2,
    title: "Databases",
    blocks: [{ kind: "table", title: "All", rowLink: "resource", columns: [{ key: "name", label: "Name" }], rows: [{ name: "orders", resource_id: "db-7" }] }],
  };
  await flush(() => {
    root.render(<PluginPanel value={value} invoke={() => Promise.resolve({})} onOpenResource={(id) => { opened.push(id); }} />);
  });
  assert.doesNotMatch(host.textContent, /db-7/, "the id is not a visible column");
  const row = host.querySelector<HTMLAnchorElement>("tbody a");
  assert.ok(row);
  await flush(() => { row.click(); });
  assert.deepEqual(opened, ["db-7"]);
  await flush(() => { root.unmount(); });
  await browser.happyDOM.abort();
});

await test("reveal shows a value only after a click and hides it again", async () => {
  const { browser, host, root } = dom();
  const calls: unknown[] = [];
  const value = { version: 2, title: "Connection", blocks: [{ kind: "reveal", label: "Connection string", action: "credentials" }] };
  await flush(() => {
    root.render(<PluginPanel value={value} context={{ resource_id: "db-1" }} invoke={(...args) => { calls.push(args); return Promise.resolve({ value: "postgres://u:p@h/db" }); }} />);
  });
  assert.doesNotMatch(host.innerHTML, /postgres:/);
  const show = [...host.querySelectorAll("button")].find((b) => b.textContent.includes("Show"));
  assert.ok(show);
  await flush(() => { show.click(); });
  await flush(() => undefined);
  const request = calls[0] as [string, unknown, AbortSignal];
  assert.deepEqual(request.slice(0, 2), ["credentials", { resource_id: "db-1" }]);
  assert.ok(request[2] instanceof AbortSignal);
  assert.match(host.textContent, /postgres:\/\/u:p@h\/db/);
  const hide = [...host.querySelectorAll("button")].find((b) => b.textContent.includes("Hide"));
  assert.ok(hide);
  await flush(() => { hide.click(); });
  assert.doesNotMatch(host.innerHTML, /postgres:/);
  await flush(() => { root.unmount(); });
  await browser.happyDOM.abort();
});

await test("read-only resource panels keep read actions but disable write actions", async () => {
  const { browser, host, root } = dom();
  const value = { version: 2, title: "Resource", blocks: [{ kind: "actions", actions: [
    { id: "resource.read", label: "Refresh" },
    { id: "resource.write", label: "Change settings" },
  ] }] };
  const calls: string[] = [];
  try {
    await flush(() => { root.render(<PluginPanel value={value} allowedActions={["resource.read"]} invoke={action => { calls.push(action); return Promise.resolve({}); }} />); });
    const buttons = [...host.querySelectorAll("button")];
    assert.equal(buttons.find(b => b.textContent === "Refresh")?.disabled, false);
    assert.equal(buttons.find(b => b.textContent === "Change settings")?.disabled, true);
    await flush(() => { buttons.find(b => b.textContent === "Refresh")?.click(); });
    assert.deepEqual(calls, ["resource.read"]);
  } finally { await flush(() => { root.unmount(); }); await browser.happyDOM.close(); }
});

await test("generic panel context changes abort pending private values and discard late replies", async () => {
  const { browser, host, root } = dom();
  let complete: ((value: unknown) => void) | undefined;
  let signal: AbortSignal | undefined;
  const pending = new Promise(resolve => { complete = resolve; });
  const value = { version: 2, title: "Credentials", blocks: [{ kind: "reveal", label: "Password", action: "secret.read" }] };
  const invoke = (_action: string, _input: unknown, requestSignal?: AbortSignal) => { signal = requestSignal; return pending; };
  try {
    await flush(() => { root.render(<PluginPanel value={value} context={{ resource_id: "one" }} invoke={invoke} />); });
    await flush(() => { host.querySelector<HTMLButtonElement>("button")?.click(); });
    await flush(() => { root.render(<PluginPanel value={value} context={{ resource_id: "two" }} invoke={invoke} />); });
    assert.equal(signal?.aborted, true);
    await flush(() => { complete?.({ value: "synthetic-old-resource-secret" }); });
    assert.ok(!host.textContent.includes("synthetic-old-resource-secret"));
  } finally { await flush(() => { root.unmount(); }); await browser.happyDOM.close(); }
});
