import test from "node:test";
import assert from "node:assert/strict";
import { searchFormURL } from "../src/lib/live-search-query";
test("live query preserves filters and exact IDs but resets pagination", () => {
  const form = new FormData();
  form.set("q", "Müller & Sohn");
  form.set("customer", "9223372036854775806");
  form.set("status", "archived");
  form.set("page", "7");
  const u = new URL(
    searchFormURL("/customers", form),
    "https://studio.invalid",
  );
  assert.equal(u.searchParams.get("q"), "Müller & Sohn");
  assert.equal(u.searchParams.get("customer"), "9223372036854775806");
  assert.equal(u.searchParams.get("status"), "archived");
  assert.equal(u.searchParams.has("page"), false);
});
test("custom mail pagination resets while empty search clears the query", () => {
  const form = new FormData();
  form.set("mailSearch", "");
  form.set("mailPage", "3");
  assert.equal(searchFormURL("/mail", form, "mailPage"), "/mail");
});
import { fieldValueFromURL } from "@webdock/search/url";
test("older results cannot overwrite newer edits after focus leaves the field", () => {
  assert.equal(fieldValueFromURL("abc", "", "abcd"), undefined);
  assert.equal(fieldValueFromURL("active", "active", "archived"), undefined);
  assert.equal(fieldValueFromURL("abcd", "", "abcd"), "abcd");
});
test("explicit reset and back navigation clear pending edits and optional filters", () => {
  assert.equal(fieldValueFromURL(null, "", "unsent text", true), "");
  assert.equal(fieldValueFromURL(null, "", undefined), "");
  assert.equal(fieldValueFromURL(null, "active", "archived", true), "active");
});
