import assert from "node:assert/strict";
import test from "node:test";
import { listQuery, listURL } from "../src/lib/list-query";
test("list filters tolerate duplicate and invalid parameters without losing string IDs", () => {
  assert.deepEqual(
    listQuery({
      page: "-2",
      status: "unknown",
      q: ["  hello  ", "ignored"],
      sort: "nope",
    }),
    { page: 1, status: "active", q: "hello", sort: "-updatedAt" },
  );
  assert.equal(listQuery({ page: "2bad" }).page, 1);
  assert.equal(listQuery({ page: "99999999999999999999" }).page, 100000);
  assert.equal(listQuery({ q: "a".repeat(200) }).q.length, 160);
  const url = listURL("/", {
    q: "A & B",
    customer: "9223372036854775807",
    page: 2,
  });
  assert.equal(
    new URL(url, "https://example.com").searchParams.get("customer"),
    "9223372036854775807",
  );
  assert.equal(
    new URL(url, "https://example.com").searchParams.get("q"),
    "A & B",
  );
  assert.equal(listURL("/customers", { page: 1, q: "" }), "/customers");
});
