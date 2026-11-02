import "./router-test-environment";
import assert from "node:assert/strict";
import { after, test } from "node:test";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider, notifyManager, defaultScheduler } from "@tanstack/react-query";
import { Window } from "happy-dom";
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { ApiClient, type Session } from "../src/api";
import { ApiContext, SessionContext } from "../src/context";
import { ConnectedWebmail } from "../src/Webmail";
import { WebmailConnection } from "../src/webmail-client";
notifyManager.setScheduler(queueMicrotask);
after(() => { notifyManager.setScheduler(defaultScheduler); });
const session: Session = { user: { id: "101", name: "Reader", email: "reader@example.test", role: "user", emailVerified: true }, session: { id: "session", userId: "101", expiresAt: "2099-01-01T00:00:00Z" } };
async function flush(action: () => void = () => undefined) {
  await act(async () => { action(); await new Promise((resolve) => setTimeout(resolve, 0)); });
}
async function until(check: () => boolean) {
  for (let i = 0; i < 80; i++) { if (check()) return; await flush(); }
  assert.ok(check(), "Webmail UI did not settle");
}
function fixture(fetcher: typeof fetch) {
  const browser = new Window({ url: "https://console.example.test/mail/inbox/201?customer=101" });
  Object.assign(globalThis, { window: browser, navigator: browser.navigator, self: browser, scrollTo: browser.scrollTo.bind(browser), location: browser.location, document: browser.document, HTMLElement: browser.HTMLElement, HTMLInputElement: browser.HTMLInputElement, HTMLSelectElement: browser.HTMLSelectElement, Event: browser.Event, FormData: browser.FormData, IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity }, mutations: { retry: false } } });
  const api = new ApiClient({ apiOrigin: "https://console.example.test", authOrigin: "https://console.example.test", enabledFeatures: ["mail"] }, fetcher);
  const connection = new WebmailConnection("https://mail.example.test", "301", Date.now() + 600000, fetcher);
  return {
    host, client, connection,
    render: async () => {
      const parent = createRootRoute({ component: () => <ApiContext.Provider value={api}><SessionContext.Provider value={session}><Outlet /></SessionContext.Provider></ApiContext.Provider> });
      const mailbox = createRoute({ getParentRoute: () => parent, path: "/mail/inbox/$mailboxID", component: () => <ConnectedWebmail connection={connection} customerID="101" mailboxID="201" name="Private mailbox" address="private@example.test" sendingAvailable={false} /> });
      const directory = createRoute({ getParentRoute: () => parent, path: "/mail", component: () => null });
      const router = createRouter({ routeTree: parent.addChildren([mailbox, directory]), history: createMemoryHistory({ initialEntries: ["/mail/inbox/201?customer=101"] }), isServer: false });
      await router.load();
      await flush(() => { root.render(<QueryClientProvider client={client}><RouterProvider router={router} /></QueryClientProvider>); });
    },
    close: async () => { await flush(() => { root.unmount(); }); client.clear(); await browser.happyDOM.abort(); },
  };
}
function response(action: unknown) {
  if (action === "folders") return { data: { list: [{ id: "inbox", name: "Private folder", role: "inbox", unreadEmails: 1 }] } };
  if (action === "identities") return { data: [{ id: "sender", email: "private@example.test", name: "Private sender" }] };
  if (action === "list") return { data: { list: [{ id: "email", subject: "Private message", from: [{ email: "sender@example.test" }], preview: "Private preview" }], total: 1, position: 0 } };
  return { data: { id: "email", subject: "Private message", textBody: [{ partId: "body" }], bodyValues: { body: { value: "Private message body" } } } };
}
await test("gateway denial removes every private folder, header, message and draft from the current render", async () => {
  let denied = false;
  const view = fixture((_url, options) => {
    const input: unknown = JSON.parse(typeof options?.body === "string" ? options.body : "{}");
    const action = input && typeof input === "object" && "action" in input ? input.action : undefined;
    if (action === "read") denied = true;
    return Promise.resolve(Response.json(response(action), { status: denied ? 403 : 200 }));
  });
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Private message"));
    await flush(() => {
      const button = [...view.host.querySelectorAll("button")].find((item) => item.textContent.includes("Private message"));
      assert.ok(button); button.click();
    });
    await until(() => view.host.textContent.includes("expired or was revoked"));
    assert.doesNotMatch(view.host.textContent, /Private folder|Private mailbox|private@example.test|Private message|Private preview/);
    assert.equal(view.connection.isClosed(), true);
  } finally { await view.close(); }
});
await test("an uncertain draft keeps one exact request and never automatically submits another change", async () => {
  const writes: string[] = [];
  const view = fixture((_url, options) => {
    const raw = typeof options?.body === "string" ? options.body : "{}";
    const input: unknown = JSON.parse(raw);
    const action = input && typeof input === "object" && "action" in input ? input.action : undefined;
    if (action === "draft") { writes.push(raw); return Promise.resolve(Response.json({}, { status: 503 })); }
    return Promise.resolve(Response.json(response(action)));
  });
  const click = (text: string) => {
    const button = [...view.host.querySelectorAll("button")].find((item) => item.textContent === text);
    assert.ok(button, text); button.click();
  };
  try {
    await view.render();
    await until(() => view.host.textContent.includes("Private message"));
    await flush(() => { click("New message"); });
    await flush(() => { click("Save draft"); click("Save draft"); });
    await until(() => view.host.textContent.includes("outcome is unknown"));
    assert.equal(writes.length, 1);
    await flush(() => { click("Retry same request"); });
    await until(() => writes.length === 2);
    assert.equal(writes[1], writes[0]);
    assert.equal([...view.host.querySelectorAll("button")].find((item) => item.textContent === "Send message")?.disabled, true);
  } finally { await view.close(); }
});
