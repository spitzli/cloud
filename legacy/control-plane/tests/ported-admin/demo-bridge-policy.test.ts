import test from "node:test";
import assert from "node:assert/strict";
import {
  demoBridges,
  authorizedDemoSite,
} from "../../src/modules/cms-bridge/demo-bridge-policy";
const bridge = { origin: "https://sample.webdock.dev", secret: "s".repeat(48) };
const site = {
  id: "123",
  name: "Sample",
  url: "https://sample.webdock.dev/admin",
  role: "reader",
};
test("bridge configuration accepts only fixed HTTPS demo origins and long secrets", () => {
  assert.deepEqual(demoBridges(JSON.stringify({ "123": bridge })), {
    "123": bridge,
  });
  for (const origin of [
    "http://sample.webdock.dev",
    "https://evil.test",
    "https://sample.webdock.dev/path",
    "https://sample.webdock.dev:443",
    "https://x.webdock.dev.evil.test",
    "https://user@sample.webdock.dev",
  ])
    assert.throws(() =>
      demoBridges(JSON.stringify({ "123": { ...bridge, origin } })),
    );
  assert.throws(() =>
    demoBridges(JSON.stringify({ "123": { ...bridge, secret: "short" } })),
  );
});
test("authorization requires live exact website grant, bound origin and write role", () => {
  assert.equal(authorizedDemoSite([site], "123", bridge)?.id, "123");
  assert.equal(authorizedDemoSite([site], "123", bridge, true), undefined);
  assert.equal(
    authorizedDemoSite([{ ...site, role: "editor" }], "123", bridge, true)?.id,
    "123",
  );
  assert.equal(
    authorizedDemoSite([{ ...site, role: "admin" }], "123", bridge, true)?.id,
    "123",
  );
  assert.equal(
    authorizedDemoSite([{ ...site, role: "operator" }], "123", bridge, true)
      ?.id,
    "123",
  );
  assert.equal(authorizedDemoSite([site], "456", bridge), undefined);
  assert.equal(authorizedDemoSite([], "123", bridge), undefined);
  assert.equal(
    authorizedDemoSite(
      [{ ...site, url: "https://foreign.webdock.dev/admin" }],
      "123",
      bridge,
    ),
    undefined,
  );
  assert.equal(
    authorizedDemoSite([{ ...site, role: "member" }], "123", bridge),
    undefined,
  );
});

test("tenant-admin preview retains management reads but never permits writes", () => {
  const site = {
    id: "123",
    name: "Customer site",
    url: "https://example.webdock.dev/cms",
    role: "admin",
  };
  const bridge = {
    origin: "https://example.webdock.dev",
    secret: "s".repeat(48),
  };
  assert.equal(
    authorizedDemoSite([site], "123", bridge, false, true)?.role,
    "admin",
  );
  assert.equal(
    authorizedDemoSite([site], "123", bridge, true, true),
    undefined,
  );
  assert.equal(
    authorizedDemoSite([site], "999", bridge, false, true),
    undefined,
  );
  assert.equal(
    authorizedDemoSite([site], "123", bridge, true, false)?.role,
    "admin",
  );
});

test("additive shop capability is explicit and typed", () => {
  assert.equal(
    demoBridges(JSON.stringify({ "123": { ...bridge, shop: true } }))["123"]
      .shop,
    true,
  );
  assert.throws(() =>
    demoBridges(JSON.stringify({ "123": { ...bridge, shop: "true" } })),
  );
});
