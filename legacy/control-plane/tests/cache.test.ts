import { expect, test } from "bun:test";
import { ViewCache } from "../src/cache";
test("cache outage preserves authoritative reads and does not repeatedly wait on a broken cache", async () => {
  const cache = new ViewCache({
    get: async () => {
      throw Error("offline");
    },
    set: async () => {
      throw Error("offline");
    },
  });
  let revision = 0;
  expect(
    await cache.read(
      {
        kind: "usage",
        environment: "test",
        customerID: "1",
        resourceID: "storage",
        revision: "1",
      },
      async () => ({ revision: ++revision }),
    ),
  ).toEqual({ revision: 1 });
  expect(
    await cache.read(
      {
        kind: "usage",
        environment: "test",
        customerID: "1",
        resourceID: "storage",
        revision: "1",
      },
      async () => ({ revision: ++revision }),
    ),
  ).toEqual({ revision: 2 });
});
test("tenant and revision changes cannot retrieve another cached view", async () => {
  const store = new Map<string, string>();
  const cache = new ViewCache({
    get: async (key) => store.get(key) ?? null,
    set: async (key, value) => {
      store.set(key, value);
    },
  });
  const key = {
    kind: "usage" as const,
    environment: "test",
    customerID: "1",
    resourceID: "storage",
    revision: "1",
  };
  expect(await cache.read(key, async () => ({ value: "one" }))).toEqual({
    value: "one",
  });
  expect(
    await cache.read({ ...key, customerID: "2" }, async () => ({
      value: "two",
    })),
  ).toEqual({ value: "two" });
  expect(
    await cache.read({ ...key, revision: "2" }, async () => ({ value: "new" })),
  ).toEqual({ value: "new" });
  expect(await cache.read(key, async () => ({ value: "wrong" }))).toEqual({
    value: "one",
  });
});
