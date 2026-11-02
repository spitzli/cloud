import assert from "node:assert/strict";
import test from "node:test";
import { accountContinuation } from "../src/lib/account-continuation";

test("offer sign-in continuation accepts only a local opaque token and never overrides signed OAuth", () => {
  const token = "a".repeat(43);
  assert.equal(accountContinuation(`?offer=${token}`), `/offers/${token}`);
  assert.equal(accountContinuation(`?offer=${token}&sig=provider-signature`), null);
  for (const value of ["https://evil.example", "//evil.example", "../account", "a".repeat(44), "", `${token}/x`]) {
    assert.equal(accountContinuation(`?offer=${encodeURIComponent(value)}`), null);
  }
  assert.equal(accountContinuation("?invitation=123"), "/invitation?id=123");
  assert.equal(accountContinuation("?returnTo=sites"), "/sites");
});
