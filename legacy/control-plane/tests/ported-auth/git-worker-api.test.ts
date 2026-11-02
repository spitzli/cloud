import test from "node:test";
import assert from "node:assert/strict";
import { gitWorkerAPI } from "../../src/modules/hosting/git-worker-api";
import { gitRegistryRepository } from "../../src/modules/hosting/git-publication";
test("Worker routes reject missing or malformed credentials before any lease or source access", async () => {
  for (const authorization of [
    "",
    "Bearer operator",
    "Bearer " + "x".repeat(200),
  ]) {
    const response = await gitWorkerAPI(
      new Request("https://auth.example/api/hosting/git/worker/claim", {
        method: "POST",
        headers: { authorization, "Content-Type": "application/json" },
        body: "{}",
      }),
      ["claim"],
    );
    assert.equal(response.status, 401);
    assert.equal(response.headers.get("cache-control"), "no-store");
  }
});
test("Registry publication paths are derived from customer and project, never supplied image namespaces", () => {
  assert.equal(
    gitRegistryRepository("123", "456", "registry.example"),
    "registry.example/customers/123/projects/456",
  );
  for (const host of [
    "https://registry.example",
    "registry.example/another-tenant",
    "registry.example@evil.example",
  ])
    assert.throws(() => gitRegistryRepository("123", "456", host));
  assert.throws(() =>
    gitRegistryRepository("../123", "456", "registry.example"),
  );
});
