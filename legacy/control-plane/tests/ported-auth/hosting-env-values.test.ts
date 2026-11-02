import test from "node:test";
import assert from "node:assert/strict";
import { environmentPatchSchema } from "@webdock/hosting-contracts";
import {
  patchEnvironment,
  openEnvironment,
  sealEnvironment,
  redactValues,
} from "../../src/modules/hosting/environment";

test("environment patches preserve, replace, empty and remove values without duplicate names", () => {
  const initial = { TOKEN: "private-token", EMPTY: "old", KEEP: "preserved" };
  const next = patchEnvironment(initial, [
    { name: "TOKEN", value: "rotated-token" },
    { name: "EMPTY", value: "" },
    { name: "KEEP", value: null },
  ]);
  assert.deepEqual(next, { TOKEN: "rotated-token", EMPTY: "" });
  assert.equal(initial.TOKEN, "private-token");
  for (const patch of [
    [{ name: "bad-name", value: "x" }],
    [
      { name: "A", value: "a" },
      { name: "A", value: null },
    ],
    [{ name: "__proto__", value: "x" }],
    [{ name: "A", value: "a\0b" }],
    [{ name: "A", value: "é".repeat(4096) }],
  ])
    assert.equal(environmentPatchSchema.safeParse(patch).success, false);
  assert.throws(() =>
    patchEnvironment(
      Object.fromEntries(Array.from({ length: 32 }, (_, i) => ["V" + i, "x"])),
      [{ name: "MORE", value: "y" }],
    ),
  );
});

test("environment encryption is bound to one app and plaintext never becomes metadata", () => {
  const values = {
    TOKEN: "not-a-real-token-for-test",
    MULTILINE: "first\nsecond",
  };
  const sealed = sealEnvironment("123", values)!;
  assert.ok(!sealed.includes(values.TOKEN));
  assert.deepEqual(openEnvironment("123", sealed), values);
  assert.throws(() => openEnvironment("124", sealed));
  assert.equal(sealEnvironment("123", {}), null);
  assert.equal(
    redactValues(
      "token=" + values.TOKEN + " " + values.MULTILINE,
      Object.values(values),
    ),
    "token=[redacted] [redacted]",
  );
  assert.equal(
    redactValues("old=abcdef new=abc", ["abc", "abcdef"]),
    "old=[redacted] new=[redacted]",
  );
  assert.equal(
    redactValues("x".repeat(5998) + "se", ["secret"], true),
    "x".repeat(5998) + "[redacted]",
  );
  assert.equal(
    redactValues("cret\nready", ["secret"], true),
    "[redacted]\nready",
  );
});
