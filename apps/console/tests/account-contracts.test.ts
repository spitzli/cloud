import assert from "node:assert/strict";
import { test } from "node:test";
import { authorizationContinuation } from "../src/account-contracts";
await test("account continuation stays on the canonical authorization endpoint", () => {
  assert.equal(
    authorizationContinuation(
      "/api/auth/oauth2/authorize?state=abc",
      "https://auth.example.invalid",
    ),
    "/api/auth/oauth2/authorize?state=abc",
  );
  for (const path of [
    "//evil.invalid",
    "/api/auth/sign-out",
    "/api/auth/oauth2/authorize\\evil",
    "https://evil.invalid",
    "/api/auth/oauth2/%61uthorize",
  ])
    assert.equal(
      authorizationContinuation(path, "https://auth.example.invalid"),
      null,
    );
});
