import { test, expect, afterEach } from "bun:test";
import { ViewCache, configureViewCache, readCachedView } from "../src/cache";
const options = {
  kind: "registry" as const,
  scope: { subject: "user-one", sessionID: "session-one" },
  query: { page: 1, filter: { active: true } },
  ttlSeconds: 30,
};
function fixture() {
  const values = new Map<string, { value: string; expires: number }>();
  const writes: { key: string; ttl: number }[] = [];
  const cache = new ViewCache({
    get: async (key) => {
      const v = values.get(key);
      return v && v.expires > Date.now() ? v.value : null;
    },
    set: async (key, value, ttl) => {
      writes.push({ key, ttl });
      values.set(key, { value, expires: Date.now() + ttl * 1000 });
    },
  });
  configureViewCache(cache, { environment: "test-api", defaultTTLSeconds: 60 });
  return { values, writes, cache };
}
afterEach(() => configureViewCache(undefined, { environment: "test-api" }));
test("authorized view keys isolate scopes, queries, environments and revisions while canonicalizing object keys", async () => {
  const { writes, cache } = fixture();
  let loads = 0,
    revision = "1";
  const version = async () => revision,
    load = async () => ({ value: ++loads });
  expect(await readCachedView(options, version, load)).toEqual({ value: 1 });
  expect(
    await readCachedView(
      { ...options, query: { filter: { active: true }, page: 1 } },
      version,
      load,
    ),
  ).toEqual({ value: 1 });
  await readCachedView(
    { ...options, scope: { ...options.scope, sessionID: "other" } },
    version,
    load,
  );
  await readCachedView({ ...options, query: { page: 2 } }, version, load);
  revision = "2";
  expect(await readCachedView(options, version, load)).toEqual({ value: 4 });
  expect(
    writes.every(
      (w) => !w.key.includes("user-one") && !w.key.includes("session-one"),
    ),
  ).toBe(true);
  expect(writes[0].ttl).toBe(30);
  await readCachedView(
    { ...options, scope: { ...options.scope, subject: "other-user" } },
    version,
    load,
  );
  configureViewCache(cache, {
    environment: "another-api",
    defaultTTLSeconds: 60,
  });
  expect(await readCachedView(options, version, load)).toEqual({ value: 6 });
});
test("unconfigured cache avoids version SQL, configured version errors propagate", async () => {
  let calls = 0;
  const version = async () => {
    calls++;
    throw Error("SQL unavailable");
  };
  configureViewCache(undefined, { environment: "test" });
  expect(await readCachedView(options, version, async () => "fresh")).toBe(
    "fresh",
  );
  expect(calls).toBe(0);
  fixture();
  await expect(
    readCachedView(
      options,
      version,
      async () => "must not run in aborted transaction",
    ),
  ).rejects.toThrow("SQL unavailable");
});
test("same-key misses share one loader, failures and sensitive results never enter cache", async () => {
  const { values } = fixture();
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  let loads = 0;
  const load = async () => {
    loads++;
    await gate;
    return { name: "safe" };
  };
  const pending = [
    readCachedView(options, async () => "1", load),
    readCachedView(options, async () => "1", load),
  ];
  await Promise.resolve();
  await Promise.resolve();
  release();
  const [first, second] = await Promise.all(pending);
  expect(loads).toBe(1);
  first.name = "changed";
  expect(second.name).toBe("safe");
  const failed = { ...options, query: { case: "failure" } };
  for (let i = 0; i < 2; i++)
    await expect(
      readCachedView(
        failed,
        async () => "1",
        async () => {
          throw Error("loader failed");
        },
      ),
    ).rejects.toThrow("loader failed");
  expect(values.size).toBe(1);
  for (let i = 0; i < 2; i++)
    await readCachedView(
      { ...options, query: { case: "secret" } },
      async () => "1",
      async () => ({ nested: { accessToken: "private" } }),
    );
  expect(values.size).toBe(1);
});
test("slow old fills cannot replace the current revision", async () => {
  fixture();
  let release!: (v: { name: string }) => void;
  let revision = "old";
  const old = readCachedView(
    options,
    async () => revision,
    () => new Promise((r) => (release = r)),
  );
  while (!release) await Promise.resolve();
  revision = "new";
  expect(
    await readCachedView(
      options,
      async () => revision,
      async () => ({ name: "new" }),
    ),
  ).toEqual({ name: "new" });
  release({ name: "old" });
  await old;
  expect(
    await readCachedView(
      options,
      async () => revision,
      async () => ({ name: "wrong" }),
    ),
  ).toEqual({ name: "new" });
});
test("TTL expiry reloads and Dragonfly failure immediately falls back to fresh data", async () => {
  const { values } = fixture();
  let calls = 0;
  const load = async () => ({ n: ++calls });
  await readCachedView(options, async () => "1", load);
  for (const entry of values.values()) entry.expires = 0;
  expect(await readCachedView(options, async () => "1", load)).toEqual({
    n: 2,
  });
  configureViewCache(
    new ViewCache({
      get: async () => {
        throw Error("offline");
      },
      set: async () => {
        throw Error("offline");
      },
    }),
    { environment: "test" },
  );
  expect(await readCachedView(options, async () => "1", load)).toEqual({
    n: 3,
  });
  expect(await readCachedView(options, async () => "1", load)).toEqual({
    n: 4,
  });
});

test("failed writes and cache timeouts preserve fresh loader results", async () => {
  let loads = 0;
  configureViewCache(
    new ViewCache({
      get: async () => null,
      set: async () => {
        throw Error("write failed");
      },
    }),
    { environment: "test" },
  );
  expect(
    await readCachedView(
      options,
      async () => "1",
      async () => ({ n: ++loads }),
    ),
  ).toEqual({ n: 1 });
  expect(
    await readCachedView(
      options,
      async () => "1",
      async () => ({ n: ++loads }),
    ),
  ).toEqual({ n: 2 });
  configureViewCache(
    new ViewCache({ get: () => new Promise(() => {}), set: async () => {} }),
    { environment: "test" },
  );
  const started = performance.now();
  expect(
    await readCachedView(
      options,
      async () => "1",
      async () => "fresh",
    ),
  ).toBe("fresh");
  expect(performance.now() - started).toBeLessThan(1000);
});

test("write transactions bypass all cache reads and invalid empty revisions fail closed", async () => {
  let gets = 0,
    sets = 0,
    loads = 0;
  configureViewCache(
    new ViewCache({
      get: async () => {
        gets++;
        return null;
      },
      set: async () => {
        sets++;
      },
    }),
    { environment: "test" },
  );
  expect(
    await readCachedView(
      options,
      async () => null,
      async () => ++loads,
    ),
  ).toBe(1);
  expect(
    await readCachedView(
      options,
      async () => null,
      async () => ++loads,
    ),
  ).toBe(2);
  expect(gets).toBe(0);
  expect(sets).toBe(0);
  await expect(
    readCachedView(
      options,
      async () => "",
      async () => "wrong",
    ),
  ).rejects.toThrow("Missing view cache revision");
});

test("Clerk and linked legacy views bypass cached SQL revisions across organization and role changes", async () => {
  const { withDatabaseScope } = await import("@webdock/database-runtime");
  const { writes } = fixture();
  let loads = 0;
  const load = async () => ({ value: ++loads });
  const version = async () => "unchanged-sql-revision";
  // Seed the same legacy key to prove verified Clerk requests cannot hit it.
  expect(await readCachedView(options, version, load)).toEqual({ value: 1 });
  for (const [organizationID, role] of [["org-first", "admin"], ["org-second", "member"], ["org-first", "member"]] as const) {
    const clerk = { operator: false, issuer: "https://fixture.clerk.accounts.dev", userID: "clerk-user", organizationID, clerkOrganizationID: organizationID, customerID: organizationID, role, verifiedAt: Date.now(), expiresAt: Date.now() + 60000 };
    const value = await withDatabaseScope({kind:"session",subject:"user-one",sessionID:"session-one",clerk}, () => readCachedView(options, version, load));
    expect(value).toEqual({value: loads});
  }
  expect(loads).toBe(4);
  const value = await withDatabaseScope({kind:"session",subject:"user-one",sessionID:"session-one",memberships:{operator:false,twoFactorEnabled:false,issuer:"https://fixture.clerk.accounts.dev",userID:"clerk-user",verifiedAt:Date.now(),expiresAt:Date.now()+60000,roles:[]}}, () => readCachedView(options, version, load));
  expect(value).toEqual({value:5});
  expect(writes).toHaveLength(1);
});
