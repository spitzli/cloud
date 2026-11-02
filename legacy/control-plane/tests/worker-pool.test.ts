import { test, expect } from "bun:test";
import { createWorkerDatabases } from "../src/db/client";
test("worker composition owns only one restricted pool and never needs bootstrap credentials", async () => {
  const pools = createWorkerDatabases({
    environment: "test",
    workerURL:
      "postgresql://webdock_worker_runtime:fixture@127.0.0.1:1/webdock_api_test",
    signingSecret: "fixture-context-key-at-least-32-characters",
    max: 1,
  });
  try {
    expect(pools.authentication).toBe(pools.worker);
    expect(pools.business).toBe(pools.worker);
    expect(pools.orm).toBe(pools.workerOrm);
  } finally {
    await pools.close();
  }
  expect(() =>
    createWorkerDatabases({
      environment: "production",
      workerURL:
        "postgresql://webdock_auth_bootstrap:fixture@db.example.invalid/platform",
      signingSecret: "fixture-context-key-at-least-32-characters",
      max: 1,
    }),
  ).toThrow();
});
