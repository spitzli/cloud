import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, notifyManager, defaultScheduler } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { I18nProvider } from "@webdock/i18n/react";
import { ApiClient } from "../src/api";
import { ApiContext } from "../src/context";
import { PublicWaitlist, WaitlistReview } from "../src/Waitlist";

notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
async function flush(action: () => void = () => undefined) {
  await act(async () => { action(); await new Promise(resolve => setTimeout(resolve, 0)); });
}
async function until(check: () => boolean) {
  for (let n = 0; n < 60; n++) { if (check()) return; await flush(); }
  assert.ok(check(), "Waitlist did not settle");
}
function button(host: ParentNode, label: string) {
  const value = [...host.querySelectorAll<HTMLButtonElement>("button")].find(item => item.textContent === label);
  assert.ok(value, `Missing button ${label}`); return value;
}
function select(host: ParentNode, selector: string) {
  const value = host.querySelector<HTMLElement>(selector); assert.ok(value, `Missing ${selector}`); return value;
}
async function fixture(fetcher: typeof fetch, publicForm = false, locale: "en" | "de" = "en") {
  const browser = new Window({ url: "https://console.example/waitlist" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, document: browser.document, location: browser.location, HTMLElement: browser.HTMLElement, FormData: browser.FormData, Event: browser.Event, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host), client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity }, mutations: { retry: false } } });
  const api = new ApiClient({ apiOrigin: "https://console.example", authOrigin: "https://console.example", enabledFeatures: [] }, fetcher);
  await flush(() => { root.render(<QueryClientProvider client={client}><ApiContext value={api}><I18nProvider locale={locale} preference={locale}>{publicForm ? <PublicWaitlist /> : <WaitlistReview />}</I18nProvider></ApiContext></QueryClientProvider>); });
  return { host, client, close: async () => { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); } };
}
const entry = { id: "10", email: "contact@example.test", name: null, status: "pending", createdAt: "2026-10-31T00:00:00Z", reviewedAt: null, version: 1 };

await test("German waitlist chrome and dates translate without changing contact names", async () => {
  const f = await fixture(() => Promise.resolve(Response.json({ entries: [{ ...entry, name: "Waitlist" }], nextCursor: null })), false, "de");
  try {
    await until(() => f.host.querySelector("li") !== null);
    assert.equal(select(f.host, "h1").textContent, "Warteliste");
    assert.equal(button(f.host, "Freigeben").disabled, false);
    assert.equal(select(f.host, "li p").textContent, "Waitlist");
    assert.equal(select(f.host, "time").textContent, "31.10.2026");
  } finally { await f.close(); }
});

await test("German waitlist errors retain understandable recovery controls", async () => {
  const f = await fixture(() => Promise.resolve(new Response(null, { status: 429 })), false, "de");
  try {
    await until(() => f.host.querySelector('[role="alert"]') !== null);
    assert.match(select(f.host, '[role="alert"]').textContent, /Laden nicht möglich/);
    assert.match(select(f.host, '[role="alert"]').textContent, /Zu viele Anfragen/);
    assert.equal(button(f.host, "Erneut versuchen").disabled, false);
  } finally { await f.close(); }
});

await test("a failed public submission retains contact fields and explains connection recovery", async () => {
  let fail: (error: Error) => void = () => undefined;
  const f = await fixture(() => new Promise<Response>((_, reject) => { fail = reject; }), true);
  try {
    const email = f.host.querySelector<HTMLInputElement>('input[name="email"]'); assert.ok(email);
    email.value = "contact@example.test";
    const privacy = f.host.querySelector<HTMLInputElement>('input[name="privacy"]'); assert.ok(privacy); privacy.checked = true;
    await flush(() => { select(f.host, "form").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    assert.equal(f.host.querySelector("fieldset")?.disabled, true, "Form should not change while saving");
    await flush(() => { fail(new TypeError("Failed to fetch internal transport")); });
    await until(() => f.host.querySelector('[role="alert"]') !== null);
    assert.match(select(f.host, '[role="alert"]').textContent, /Check your connection and try again/);
    assert.doesNotMatch(f.host.textContent, /internal transport/);
    assert.equal(email.value, "contact@example.test"); assert.equal(privacy.checked, true);
    assert.equal(button(f.host, "Join the waitlist").disabled, false);
  } finally { await f.close(); }
});

await test("repeated public submit events share one pending attempt and allow a deliberate retry", async () => {
  let attempts = 0, fail: (error: Error) => void = () => undefined;
  const f = await fixture(() => { attempts++; return new Promise<Response>((_, reject) => { fail = reject; }); }, true);
  try {
    const email = f.host.querySelector<HTMLInputElement>('input[name="email"]'); assert.ok(email); email.value = "contact@example.test";
    const privacy = f.host.querySelector<HTMLInputElement>('input[name="privacy"]'); assert.ok(privacy); privacy.checked = true;
    const submit = () => { select(f.host, "form").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); };
    await flush(() => { submit(); submit(); });
    assert.equal(attempts, 1, "Repeated submit events must not spend the contact's abuse budget twice");
    await flush(() => { fail(new TypeError("Connection lost")); });
    await until(() => !button(f.host, "Join the waitlist").disabled);
    await flush(submit);
    assert.equal(attempts, 2, "A failed request must remain retryable");
    await flush(() => { fail(new TypeError("Connection lost")); });
  } finally { await f.close(); }
});

await test("review action groups identify the contact when requests share a display name", async () => {
  const f = await fixture(() => Promise.resolve(Response.json({ entries: [{ ...entry, name: "Same name" }, { ...entry, id: "11", name: "Same name", email: "other@example.test" }], nextCursor: null })));
  try {
    await until(() => f.host.querySelectorAll("li").length === 2);
    const groups = [...f.host.querySelectorAll('[role="group"]')];
    assert.deepEqual(groups.map(group => group.getAttribute("aria-label")), [entry.email, "other@example.test"]);
    for (const group of groups) assert.equal(group.querySelectorAll("button").length, 2);
  } finally { await f.close(); }
});

await test("review completion explains that approval still needs a workspace invitation", async () => {
  let approved = false;
  const f = await fixture((url, init) => {
    assert.ok(url instanceof URL); assert.ok(init); assert.equal(init.credentials, "include");
    if (init.method === "POST") {
      assert.equal(url.pathname, "/api/console/waitlist/10");
      assert.ok(typeof init.body === "string");
      assert.deepEqual(JSON.parse(init.body), { action: "approve", version: 1 });
      approved = true; return Promise.resolve(Response.json({ ...entry, status: "approved", version: 2, reviewedAt: "2026-10-31T01:00:00Z" }));
    }
    return Promise.resolve(Response.json({ entries: [{ ...entry, status: approved ? "approved" : "pending", version: approved ? 2 : 1 }], nextCursor: null }));
  });
  try {
    await until(() => f.host.textContent.includes("Approve"));
    await flush(() => { button(f.host, "Approve").click(); });
    await until(() => f.host.querySelector('[role="status"]')?.textContent.includes("Request approved") === true);
    assert.match(select(f.host, '[role="status"]').textContent, /Invite this contact from workspace member management/);
    assert.equal([...f.host.querySelectorAll("button")].some(value => value.textContent === "Approve"), false);
  } finally { await f.close(); }
});

await test("repeated review clicks cannot race conflicting terminal decisions", async () => {
  const decisions: unknown[] = [];
  let finish: (response: Response) => void = () => undefined;
  const f = await fixture((_url, init) => {
    if (init?.method === "POST") {
      assert.ok(typeof init.body === "string"); decisions.push(JSON.parse(init.body));
      return new Promise<Response>(resolve => { finish = resolve; });
    }
    return Promise.resolve(Response.json({ entries: [entry], nextCursor: null }));
  });
  try {
    await until(() => f.host.querySelector("li") !== null);
    await flush(() => { button(f.host, "Approve").click(); button(f.host, "Reject").click(); });
    assert.deepEqual(decisions, [{ action: "approve", version: 1 }]);
    await flush(() => { finish(Response.json({ ...entry, status: "approved", version: 2, reviewedAt: "2026-10-31T01:00:00Z" })); });
    await until(() => f.host.querySelector('[role="status"]') !== null);
  } finally { await f.close(); }
});

await test("a lost review response refreshes the committed decision before allowing another action", async () => {
  let committed = false, writes = 0;
  const f = await fixture((_url, init) => {
    if (init?.method === "POST") {
      writes++; committed = true;
      return Promise.reject(new TypeError("Response lost after commit"));
    }
    return Promise.resolve(Response.json({ entries: [{ ...entry, status: committed ? "approved" : "pending", version: committed ? 2 : 1, reviewedAt: committed ? "2026-10-31T01:00:00Z" : null }], nextCursor: null }));
  });
  try {
    await until(() => f.host.querySelector("li") !== null);
    await flush(() => { button(f.host, "Approve").click(); });
    await until(() => f.host.querySelector('[role="alert"]') !== null);
    assert.match(select(f.host, "li").textContent, /Approved/);
    assert.equal(f.host.querySelectorAll('li button').length, 0, "The committed decision must not expose stale retry controls");
    assert.equal(writes, 1, "An uncertain decision must never be retried automatically");
    assert.equal(f.host.querySelector('[role="status"]'), null, "A lost response must not be reported as a successful response");
  } finally { await f.close(); }
});

await test("a review in flight holds pagination and failure reports the review action", async () => {
  let finish: (value: Response) => void = () => undefined;
  const f = await fixture((url, init) => {
    assert.ok(url instanceof URL);
    if (init?.method === "POST") return new Promise<Response>(resolve => { finish = resolve; });
    return Promise.resolve(Response.json({ entries: [entry], nextCursor: "older-page" }));
  });
  try {
    await until(() => f.host.textContent.includes("Approve"));
    await flush(() => { button(f.host, "Reject").click(); });
    assert.equal(button(f.host, "Older requests").disabled, true);
    await flush(() => { finish(Response.json({ error: { message: "Already reviewed. Refresh before trying again." } }, { status: 409 })); });
    await until(() => f.host.querySelector('[role="alert"]') !== null);
    assert.match(select(f.host, '[role="alert"]').textContent, /Unable to review request/);
    assert.match(select(f.host, '[role="alert"]').textContent, /Already reviewed/);
    await until(() => !button(f.host, "Older requests").disabled);
  } finally { await f.close(); }
});
