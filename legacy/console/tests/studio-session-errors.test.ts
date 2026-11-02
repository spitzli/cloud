import test from "node:test";
import assert from "node:assert/strict";
import { AccessError, studioError } from "../src/lib/studio-errors";
test("a backend 401 after delegation cannot be mistaken for a missing local session", () => {
  const error = studioError("AccessError", "Sign in again", 401);
  assert.equal(error.status, 401);
  assert.equal(error instanceof AccessError, false);
  assert.equal(studioError("AccessError", "Forbidden", 403) instanceof AccessError, true);
});
