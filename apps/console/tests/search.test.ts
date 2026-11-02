import { test } from "node:test";
import assert from "node:assert/strict";
import { parseConsoleSearch } from "../src/search";
await test("search preserves decimal resource IDs and router-encoded strings without numeric coercion", () => {
  assert.deepEqual(parseConsoleSearch("?customer=9223372036854775807"), {
    customer: "9223372036854775807",
  });
  assert.deepEqual(
    parseConsoleSearch("?customer=%229223372036854775807%22&managed=true"),
    { customer: "9223372036854775807", managed: "true" },
  );
  assert.deepEqual(parseConsoleSearch("?customer=1&customer=2"), {});
});
