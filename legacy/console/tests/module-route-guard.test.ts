import { test } from "node:test";
import assert from "node:assert/strict";
import { guardModuleRoute } from "../src/lib/module-route-guard.ts";
const request = (path: string) => new Request("https://console.example" + path);
test("direct routes for all products fail closed independently from core registry routes", async () => {
  const disabled = async () => ({
    status: 200,
    body: { data: { installation: { revision: "1", modules: {} } } },
  });
  for (const path of [
    "/tenants/12/mail",
    "/tenants/12/hosting",
    "/tenants/12/databases",
    "/tenants/12/managed-databases",
    "/tenants/12/storage",
  ])
    assert.equal(
      (await guardModuleRoute(request(path), disabled))?.status,
      404,
      path,
    );
  assert.equal(
    await guardModuleRoute(request("/projects/12"), async () => {
      throw Error("Core must not request product state");
    }),
    undefined,
  );
});
test("draining serves existing pages while stale or unreachable session configuration denies access", async () => {
  assert.equal(
    await guardModuleRoute(request("/hosting/projects/12"), async () => ({
      status: 200,
      body: {
        data: { installation: { revision: "1", modules: { hosting: "draining" } } },
      },
    })),
    undefined,
  );
  assert.equal(
    (
      await guardModuleRoute(request("/hosting/projects/12"), async () => ({
        status: 401,
        body: {},
      }))
    )?.status,
    307,
  );
  assert.equal(
    (
      await guardModuleRoute(request("/hosting/projects/12"), async () => {
        throw Error("offline");
      })
    )?.status,
    503,
  );
});

test("Clerk users without tenant access reach the pending page instead of an outage", async () => {
  const denied = async () => ({ status: 403, body: {} });
  const result = await guardModuleRoute(request("/hosting/projects/12"), denied, true);
  assert.equal(result?.status, 307);
  assert.equal(result?.headers.get("location"), "/access-pending");
  assert.equal(result?.headers.get("cache-control"), "no-store");
  assert.equal((await guardModuleRoute(request("/hosting/projects/12"), denied))?.status, 503);
});
