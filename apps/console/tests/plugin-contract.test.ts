import test from "node:test";
import assert from "node:assert/strict";
import {
  parsePanel,
  parseLaunch,
  validateActionMessage,
} from "../src/plugins/contracts";
const panel = {
  version: 2,
  title: "Storage",
  blocks: [
    {
      kind: "status",
      label: "Connection",
      state: "unknown",
      text: "Not observed",
    },
  ],
};
await test("declarative UI rejects HTML, unknown fields and unbounded collections", () => {
  assert.equal(parsePanel(panel).title, "Storage");
  assert.throws(() => parsePanel({ ...panel, html: "<script />" }));
  assert.throws(() =>
    parsePanel({ ...panel, blocks: Array(51).fill(panel.blocks[0]) }),
  );
  assert.throws(() =>
    parsePanel({ ...panel, blocks: [{ kind: "html", html: "<script />" }] }),
  );
});
await test("custom UI requires a separate HTTPS origin and never accepts embedded credentials", () => {
  const launch = {
    version: 1,
    installationId: "install-1",
    url: "https://plugin.example/ui",
    actions: ["read"],
  };
  assert.equal(
    parseLaunch(launch, [
      "https://console.example",
      "https://api.example",
      "https://auth.example",
    ]).url,
    launch.url,
  );
  for (const url of [
    "https://console.example/ui",
    "https://api.example/ui",
    "https://auth.example/ui",
    "javascript:alert(1)",
    "https://secret@plugin.example/",
  ]) {
    assert.throws(() =>
      parseLaunch({ ...launch, url }, [
        "https://console.example",
        "https://api.example",
        "https://auth.example",
      ]),
    );
  }
});
await test("bridge authenticates exact frame, origin, installation, action and bounded input", () => {
  const frame = {};
  const policy = {
    origin: "https://plugin.example",
    source: frame,
    installationId: "install-1",
    actions: ["read"],
    active: true,
  };
  const data = {
    version: 1,
    type: "webdock.action",
    requestId: "request-1",
    installationId: "install-1",
    action: "read",
    input: { key: "value" },
  };
  assert.equal(
    validateActionMessage(
      { origin: policy.origin, source: frame, data },
      policy,
    )?.action,
    "read",
  );
  for (const event of [
    { origin: "https://evil.example", source: frame, data },
    { origin: policy.origin, source: {}, data },
    {
      origin: policy.origin,
      source: frame,
      data: { ...data, installationId: "install-2" },
    },
    {
      origin: policy.origin,
      source: frame,
      data: { ...data, action: "delete" },
    },
    {
      origin: policy.origin,
      source: frame,
      data: { ...data, input: { huge: "x".repeat(33000) } },
    },
  ]) {
    assert.equal(validateActionMessage(event, policy), null);
  }
  assert.equal(
    validateActionMessage(
      { origin: policy.origin, source: frame, data },
      { ...policy, active: false },
    ),
    null,
  );
});
await test("core origin comparison normalizes trailing slashes and rejects missing policy", () => {
  const launch = {
    version: 1,
    installationId: "install-1",
    url: "https://api.example/ui",
    actions: ["read"],
  };
  assert.throws(() => parseLaunch(launch, ["https://api.example/"]));
  assert.throws(() => parseLaunch(launch, []));
});

await test("panel v2 rejects version 1 and bounds the refresh interval", () => {
  assert.throws(() => parsePanel({ ...panel, version: 1 }));
  assert.equal(parsePanel({ ...panel, refreshSeconds: 30 }).refreshSeconds, 30);
  assert.throws(() => parsePanel({ ...panel, refreshSeconds: 4 }));
  assert.throws(() => parsePanel({ ...panel, refreshSeconds: 301 }));
  assert.throws(() =>
    parsePanel({
      ...panel,
      blocks: [{ kind: "table", title: "Rows", rowLink: "resource", columns: [{ key: "name", label: "Name" }], rows: [{ name: "no id" }] }],
    }),
  );
});
