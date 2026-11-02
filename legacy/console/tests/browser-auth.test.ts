import test from "node:test";
import assert from "node:assert/strict";
import { safeBrowserReturn } from "../src/lib/browser-auth-config";
test("sign-in return path cannot leave the configured Console", () => {
  for (const input of [
    "https://evil.invalid",
    "//evil.invalid",
    "/\\evil.invalid",
    "/\nevil",
    undefined,
  ])
    assert.equal(safeBrowserReturn(input), "/tenants");
  assert.equal(
    safeBrowserReturn("/tenants/123/mail?connection=456"),
    "/tenants/123/mail?connection=456",
  );
});
