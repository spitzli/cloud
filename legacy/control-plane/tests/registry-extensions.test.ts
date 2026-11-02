import { test } from "node:test";
import assert from "node:assert/strict";
import { retainedRegistryInventory } from "../src/modules/registry/extensions";
import type { RegistryTransaction } from "@webdock/core/registry/ports";

test("fresh core without historical CMS table needs no product runtime", async () => {
  let queries = 0;
  const tx = {
    execute: async () => {
      queries++;
      return { rows: [{ present: false }] };
    },
  } as unknown as RegistryTransaction;
  await retainedRegistryInventory(tx, "projects", "123");
  assert.equal(queries, 1);
});
test("retained CMS inventory blocks archive without a CMS runtime", async () => {
  let queries = 0;
  const tx = {
    execute: async () => ({
      rows: ++queries === 1 ? [{ present: true }] : [{ id: "456" }],
    }),
  } as unknown as RegistryTransaction;
  await assert.rejects(
    retainedRegistryInventory(tx, "projects", "123"),
    /Retire the linked CMS/,
  );
  assert.equal(queries, 2);
});
test("retired or absent retained inventory permits archive", async () => {
  let queries = 0;
  const tx = {
    execute: async () => ({ rows: ++queries === 1 ? [{ present: true }] : [] }),
  } as unknown as RegistryTransaction;
  await retainedRegistryInventory(tx, "projects", "123");
  assert.equal(queries, 2);
});
