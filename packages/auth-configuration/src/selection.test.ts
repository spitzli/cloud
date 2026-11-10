import { test } from "node:test";
import assert from "node:assert/strict";
import { selectAuthPlugin } from "./selection";
import { lazyAuthPlugin } from "@spitzli/core/auth-plugin";
test("legacy default and explicit providers select separate trusted entries", async () => {
  const legacy = selectAuthPlugin(undefined);
  assert.equal(legacy.configuration.provider, "better-auth");
  assert.equal((await legacy.load()).manifest.id, "auth-better-auth");
  const clerk = selectAuthPlugin(JSON.stringify({ provider: "clerk", issuer: "https://identity.example", audience: "webdock", authorizedParties: ["https://console.example"] }));
  assert.equal((await clerk.load()).manifest.id, "auth-clerk");
  assert.throws(() => selectAuthPlugin('{"provider":"unknown"}'));
});
test("trusted entries load once on demand and retry failed loading", async () => {
  let calls = 0;
  const load = lazyAuthPlugin(async () => {
    if (++calls === 1) throw Error("unavailable");
    return { id: "fixture" };
  });
  assert.equal(calls, 0);
  await assert.rejects(load(), /unavailable/);
  assert.deepEqual(await load(), { id: "fixture" });
  await load();
  assert.equal(calls, 2);
});
