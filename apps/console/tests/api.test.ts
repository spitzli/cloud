import test from "node:test";
import assert from "node:assert/strict";
import {
  ApiClient,
  ApiError,
  runtimeConfigSchema,
  sessionSchema,
  recordSchema,
  listSchema,
} from "../src/api";
const config = {
  apiOrigin: "https://api.example",
  authOrigin: "https://auth.example",
  enabledFeatures: [],
};
await test("rate limits tell people to wait without exposing the server response", async () => {
  const api = new ApiClient(config, () => Promise.resolve(new Response("internal limiter details", { status: 429 })));
  await assert.rejects(api.get("/api/console/waitlist", sessionSchema), error =>
    error instanceof ApiError && error.status === 429 && error.message === "Too many requests. Please wait before trying again.");
});
await test("runtime configuration rejects credentials and non-origin paths", () => {
  assert.deepEqual(runtimeConfigSchema.parse(config), config);
  for (const apiOrigin of [
    "https://user:pass@api.example",
    "https://api.example/path",
    "javascript:alert(1)",
  ])
    assert.throws(() => runtimeConfigSchema.parse({ ...config, apiOrigin }));
});
await test("runtime configuration carries explicit database order retirement without disabling database access", () => {
  for (const databaseOrderingEnabled of [false, true]) {
    const value = { ...config, enabledFeatures: ["databases"], databaseOrderingEnabled };
    assert.deepEqual(runtimeConfigSchema.parse(value), value);
  }
  assert.throws(() => runtimeConfigSchema.parse({ ...config, databaseOrderingEnabled: "false" }));
});
await test("registry direct envelope and session data envelope remain distinct", async () => {
  const calls: [string, RequestInit | undefined][] = [];
  const fetcher: typeof fetch = (url, options) => {
    calls.push([url instanceof Request ? url.url : String(url), options]);
    return Promise.resolve(
      Response.json(
        (url instanceof Request ? url.url : String(url)).endsWith("/session")
          ? { data: null }
          : {
              docs: [{ id: "123", name: "Customer", status: "active" }],
              page: 1,
              totalPages: 1,
              totalDocs: 1,
              hasNextPage: false,
              hasPrevPage: false,
            },
      ),
    );
  };
  const api = new ApiClient(config, fetcher);
  assert.equal((await api.get("/v1/auth/session", sessionSchema)).data, null);
  assert.equal(
    (await api.get("/v1/operator/registry/customers", listSchema)).docs[0]?.id,
    "123",
  );
  assert.equal(calls[0]?.[1]?.credentials, "include");
  assert.equal(calls[0][1].cache, "no-store");
});
await test("writes use JSON and propagate denied requests without invented success", async () => {
  const api = new ApiClient(config, (_url, options) => {
    assert.equal(options?.method, "PUT");
    assert.equal(options.body, JSON.stringify({ name: "Updated" }));
    return Promise.resolve(
      Response.json({ error: { message: "Denied" } }, { status: 403 }),
    );
  });
  await assert.rejects(
    api.write(
      "/v1/operator/registry/customers/1",
      "PUT",
      { name: "Updated" },
      recordSchema,
    ),
    /access/,
  );
});
await test("browser fetch is invoked without an ApiClient receiver", async () => {
  const fetcher: typeof fetch = function (this: unknown) {
    assert.equal(this, undefined);
    return Promise.resolve(Response.json({ data: null }));
  };
  await new ApiClient(config, fetcher).get("/v1/auth/session", sessionSchema);
});
await test("same-origin browser facade maps session, registry and auth without token storage", async () => {
  const paths: string[] = [];
  const api = new ApiClient(
    {
      apiOrigin: "https://console.example",
      authOrigin: "https://console.example",
      enabledFeatures: [],
      browserFacade: true,
    },
    (url) => {
      paths.push(url instanceof Request ? url.url : String(url));
      return Promise.resolve(Response.json({ data: null }));
    },
  );
  await api.get("/v1/auth/session", sessionSchema);
  await api.get("/v1/operator/registry/customers?limit=20", sessionSchema);
  await api.write("/api/auth/sign-out", "POST", {}, sessionSchema, true);
  assert.deepEqual(paths, [
    "https://console.example/api/console/session",
    "https://console.example/api/console/registry/customers?limit=20",
    "https://console.example/api/console/auth/sign-out",
  ]);
});
await test("API paths cannot escape the configured origin with slash or backslash authority", async () => {
  let called = false;
  const api = new ApiClient(config, () => {
    called = true;
    return Promise.resolve(Response.json({ data: null }));
  });
  for (const path of [
    "//evil.example/x",
    "/\\evil.example/x",
    "https://evil.example/x",
  ])
    await assert.rejects(api.get(path, sessionSchema), /Invalid API path/);
  assert.equal(called, false);
});

await test("caller Headers and tuple headers survive defaults", async () => {
  for (const headers of [
    new Headers({ "X-Request-ID": "test", Accept: "application/problem+json" }),
    [
      ["X-Request-ID", "test"],
      ["Accept", "application/problem+json"],
    ] as [string, string][],
  ]) {
    const fetcher: typeof fetch = (_url, options) => {
      const actual = new Headers(options?.headers);
      assert.equal(actual.get("X-Request-ID"), "test");
      assert.equal(actual.get("Accept"), "application/problem+json");
      assert.equal(actual.get("Content-Type"), "application/json");
      return Promise.resolve(Response.json({ data: null }));
    };
    await new ApiClient(config, fetcher).request(
      "/v1/auth/session",
      sessionSchema,
      { headers, body: "{}", method: "POST" },
    );
  }
});
await test("conflicts show the server's reason, other failures keep generic text", async () => {
  const replies = [
    Response.json({ error: { code: "registry_error", message: "Archive this customer before deleting it." } }, { status: 409 }),
    new Response("not json", { status: 409 }),
    Response.json({ error: { code: "x", message: "internal detail" } }, { status: 500 }),
  ];
  const client = new ApiClient(config, () => Promise.resolve(replies.shift() ?? new Response(null, { status: 500 })));
  const messages = [];
  for (let i = 0; i < 3; i++)
    messages.push(await client.get("/v1/operator/registry/customers/1", recordSchema).then(() => "", (error: unknown) => (error instanceof Error ? error.message : "")));
  assert.equal(messages[0], "Archive this customer before deleting it.");
  assert.match(messages[1] ?? "", /changed or has linked resources/);
  assert.doesNotMatch(messages[2] ?? "", /internal detail/);
});

await test("session preserves explicit owner acting while keeping the preview marker", () => {
  const result = sessionSchema.parse({ data: {
    user: { id: "1", name: "Operator", email: "operator@example.invalid", role: "user", emailVerified: true, preview: true, acting: true, customerScope: "scope-epoch" },
    session: { id: "s", userId: "1", expiresAt: "2099-01-01T00:00:00Z" },
  } });
  assert.ok(result.data);
  assert.equal(result.data.user.acting, true);
  assert.equal(result.data.user.customerScope, "scope-epoch");
});
