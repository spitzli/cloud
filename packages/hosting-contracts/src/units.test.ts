import test from "node:test";
import assert from "node:assert/strict";
import { resourceInput, parseResourceInput, formatResource } from "./units";
test("resource units round-trip exact existing values and German decimals", () => {
  for (const key of [
    "memoryBytes",
    "volumeBytes",
    "ephemeralBytes",
    "cpuMillicores",
    "apps",
  ])
    for (const n of [0, 1, 125, 67108864, 8000000000, Number.MAX_SAFE_INTEGER])
      assert.equal(parseResourceInput(key, resourceInput(key, n)), n);
  assert.equal(parseResourceInput("cpuMillicores", "0,5 cores"), 500);
  assert.equal(parseResourceInput("memoryBytes", "1.5 GB"), 1500000000);
  assert.equal(formatResource("memoryBytes", 8000000000), "8 GB");
  assert.equal(formatResource("cpuMillicores", 500), "0.5");
});
test("unit mismatch, negative values, overflow and fractional bytes are rejected", () => {
  for (const [key, value] of [
    ["apps", "1 GB"],
    ["memoryBytes", "1 cores"],
    ["cpuMillicores", "-1"],
    ["memoryBytes", "9007199254740992"],
    ["memoryBytes", "0.0000001 MB"],
    ["cpuMillicores", "0.0001 cores"],
  ])
    assert.throws(() => parseResourceInput(key, value));
});
import { resourceInUnit } from "./units";
test("unit switching preserves the exact resource allocation", () => {
  assert.equal(resourceInUnit("memoryBytes", 67108864, "GB"), "0.067108864");
  assert.equal(
    parseResourceInput(
      "memoryBytes",
      resourceInUnit("memoryBytes", 67108864, "GB") + " GB",
    ),
    67108864,
  );
});
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nProvider } from "@spitzli/i18n/react";
import { ResourceField } from "./resource-field";
test("German resource fields submit exact values while showing a separate unit selector", () => {
  const html = renderToStaticMarkup(
    createElement(
      I18nProvider,
      { locale: "de", preference: "de" },
      createElement(ResourceField, {
        name: "hosting.memoryBytes",
        dimension: "memoryBytes",
        value: 67108864,
        label: "Arbeitsspeicher",
      }),
    ),
  );
  assert.match(html, /value="67,108864"/);
  assert.match(html, /name="hosting.memoryBytes" value="67,108864 MB"/);
  assert.match(html, /<select/);
  assert.equal(parseResourceInput("memoryBytes", "67,108864 MB"), 67108864);
});
