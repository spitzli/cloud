import { expect, test } from "bun:test";
import { readConfig } from "../src/config";
test("production rejects unsafe origins and incomplete configuration", () => {
  expect(() => readConfig({ NODE_ENV: "production" })).toThrow();
  expect(() =>
    readConfig({
      NODE_ENV: "production",
      WEBDOCK_API_ORIGIN: "http://api.webdock.dev",
    }),
  ).toThrow();
});
test("origin validation rejects credentials, paths, and non-loopback insecure URLs", () => {
  for (const origin of [
    "https://user:pass@api.webdock.dev",
    "https://api.webdock.dev/subpath",
    "http://api.webdock.dev",
  ]) {
    expect(() =>
      readConfig({ NODE_ENV: "test", WEBDOCK_API_ORIGIN: origin }),
    ).toThrow();
  }
});
test("production rejects database owner credentials and wrong RLS audiences before connecting", async () => {
  const { createDatabases } = await import("../src/db/client");
  expect(() =>
    createDatabases({
      environment: "production",
      businessURL: "postgres://owner:x@db.invalid/db",
      authURL: "postgres://auth:x@db.invalid/db",
      workerURL: "postgres://worker:x@db.invalid/db",
      signingSecret: "fixture-key-that-is-long-enough-to-sign",
      max: 1,
    }),
  ).toThrow();
});
test("connection query parameters cannot override the validated runtime role or host", async () => {
  const { createDatabases } = await import("../src/db/client");
  for (const query of [
    "user=postgres",
    "host=other.invalid",
    "database=other",
    "options=-crole=postgres",
  ]) {
    expect(() =>
      createDatabases({
        environment: "production",
        businessURL:
          "postgresql://webdock_auth_runtime:fixture@db.invalid/db?" + query,
        authURL: "postgresql://webdock_auth_bootstrap:fixture@db.invalid/db",
        workerURL: "postgresql://webdock_worker_runtime:fixture@db.invalid/db",
        signingSecret: "fixture-signing-secret-at-least-32-characters",
        max: 1,
      }),
    ).toThrow();
  }
});
