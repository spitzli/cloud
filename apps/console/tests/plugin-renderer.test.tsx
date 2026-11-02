import test from "node:test";
import assert from "node:assert/strict";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { Window } from "happy-dom";
import { PluginPanel } from "../src/plugins/Panel";
import { CustomPluginUI } from "../src/plugins/CustomUI";
const noop = () => Promise.resolve({});
await test("renderer escapes supplied content and exposes table and status semantics", () => {
  const html = renderToStaticMarkup(
    <PluginPanel
      value={{
        version: 2,
        title: "Panel",
        blocks: [
          { kind: "text", text: "<script>alert(1)</script>" },
          {
            kind: "table",
            title: "Resources",
            columns: [{ key: "name", label: "Name" }],
            rows: [{ name: "<img src=x>" }],
          },
          {
            kind: "status",
            label: "Health",
            state: "unknown",
            text: "Not observed",
          },
        ],
      }}
      invoke={noop}
    />,
  );
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /<caption>Resources/);
  assert.match(html, /scope="col"/);
  assert.match(html, /Not observed/);
  assert.match(
    renderToStaticMarkup(<PluginPanel value={{ version: 99 }} invoke={noop} />),
    /role="alert"/,
  );
});
await test("forms submit typed input, require destructive confirmation and disable revoked actions", async () => {
  const browser = new Window({ url: "https://console.example" });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator,
    document: browser.document,
    HTMLElement: browser.HTMLElement,
    Event: browser.Event,
    MessageEvent: browser.MessageEvent,
    FormData: browser.FormData,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const calls: unknown[] = [];
  const value = {
    version: 2,
    title: "Configure",
    blocks: [
      {
        kind: "form",
        title: "Capacity",
        fields: [
          { name: "size", label: "Size", type: "number", required: true },
        ],
        action: { id: "resize", label: "Resize", destructive: true },
      },
    ],
  };
  const invoke = (...args: unknown[]) => {
    calls.push(args);
    return Promise.resolve();
  };
  await flushAct(() => {
    root.render(<PluginPanel value={value} invoke={invoke} />);
  });
  const button = host.querySelector("button");
  assert.ok(button);
  assert.equal(button.disabled, true);
  const input = host.querySelector<HTMLInputElement>("input[type=number]");
  assert.ok(input);
  input.value = "4";
  const checkbox = host.querySelector<HTMLInputElement>("input[type=checkbox]");
  assert.ok(checkbox);
  await flushAct(() => {
    checkbox.click();
  });
  assert.equal(button.disabled, false);
  await flushAct(() => {
    host
      .querySelector("form")
      ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  assert.deepEqual(calls, [["resize", { size: 4 }]]);
  assert.match(host.textContent, /Action completed/);
  await flushAct(() => {
    root.render(<PluginPanel value={value} invoke={invoke} active={false} />);
  });
  assert.equal(button.disabled, true);
  await flushAct(() => {
    root.unmount();
  });
  await browser.happyDOM.abort();
});
await test("custom frame blocks hostile messages and aborts pending work on revocation", async () => {
  const browser = new Window({
    url: "https://console.example",
    settings: { disableIframePageLoading: true },
  });
  Object.assign(globalThis, {
    window: browser, navigator: browser.navigator,
    document: browser.document,
    HTMLElement: browser.HTMLElement,
    Event: browser.Event,
    MessageEvent: browser.MessageEvent,
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const launch = {
    version: 1,
    installationId: "install-1",
    url: "https://plugin.example/ui",
    actions: ["read"],
  };
  const signals: AbortSignal[] = [];
  const invoke = async (
    _action: string,
    _input: unknown,
    signal?: AbortSignal,
  ) => {
    assert.ok(signal);
    signals.push(signal);
    return new Promise(() => {
      /* Pending until the component aborts its request. */
    });
  };
  const props = {
    launch,
    coreOrigins: ["https://console.example", "https://api.example"],
    invoke,
  };
  await flushAct(() => {
    root.render(<CustomPluginUI {...props} active />);
  });
  const frame = host.querySelector("iframe");
  assert.ok(frame);
  Object.defineProperty(frame, "contentWindow", {
    value: {
      postMessage() {
        /* Capture is unnecessary; this test checks revocation. */
      },
    },
  });
  assert.equal(
    frame.getAttribute("sandbox"),
    "allow-scripts allow-same-origin",
  );
  assert.equal(frame.getAttribute("referrerpolicy"), "no-referrer");
  const data = {
    version: 1,
    type: "webdock.action",
    installationId: "install-1",
    requestId: "request-1",
    action: "read",
    input: {},
  };
  const message = (origin: string, source: unknown) => {
    const event = new MessageEvent("message", { origin, data });
    Object.defineProperty(event, "source", { value: source });
    window.dispatchEvent(event);
  };
  await flushAct(() => {
    message("https://evil.example", frame.contentWindow);
    message("https://plugin.example", browser);
  });
  assert.equal(signals.length, 0);
  await flushAct(() => {
    message("https://plugin.example", frame.contentWindow);
    message("https://plugin.example", frame.contentWindow);
  });
  assert.equal(signals.length, 1, "duplicate request was rejected");
  await flushAct(() => {
    root.render(<CustomPluginUI {...props} active={false} />);
  });
  assert.equal(signals[0]?.aborted, true);
  assert.equal(host.querySelector("iframe"), null);
  await flushAct(() => {
    root.unmount();
  });
  await browser.happyDOM.abort();
});

async function flushAct(action: () => void) {
  await act(async () => {
    action();
    await Promise.resolve();
  });
}
