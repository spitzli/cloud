import { expect, test } from "bun:test";
import { testEnvironment, rlsEnvironment } from "../scripts/test-environment";
test("test children discard inherited provider credentials and use a fixed local issuer", () => {
  const env = testEnvironment({
    PATH: "/fixture/bin",
    API_TEST_DATABASE_URL:
      "postgresql://fixture:test@127.0.0.1:5432/webdock_api_test",
    BETTER_AUTH_URL: "https://auth.webdock.dev",
    DATABASE_URL: "postgresql://production/db",
    POLAR_ACCESS_TOKEN: "production",
    NODE_OPTIONS: "--require=untrusted",
  });
  expect(env.PATH).toBe("/fixture/bin");
  expect(env.POLAR_ACCESS_TOKEN).toBeUndefined();
  expect(env.NODE_OPTIONS).toBeUndefined();
  expect(env.DATABASE_URL).toContain("/webdock_api_test");
  expect(env.BETTER_AUTH_URL).toBe("http://localhost:3125");
});
test("restricted test roles must share the explicit disposable owner endpoint", () => {
  const owner = "postgresql://postgres:fixture@127.0.0.1:5432/webdock_rls_test";
  const roles = Object.fromEntries(
    [
      "webdock_auth_runtime",
      "webdock_auth_bootstrap",
      "webdock_worker_runtime",
      "webdock_registry_runtime",
    ].map((role) => [
      role,
      owner.replace("postgres:fixture", role + ":fixture"),
    ]),
  );
  const fixture = { contextSecret: "a".repeat(64), roles };
  expect(
    rlsEnvironment(
      { BETTER_AUTH_SECRET: "fixture-auth-key-at-least-32-characters" },
      fixture,
      owner,
    ).DATABASE_URL,
  ).toBe(roles.webdock_auth_runtime);
  expect(() =>
    rlsEnvironment(
      { BETTER_AUTH_SECRET: "fixture-auth-key-at-least-32-characters" },
      fixture,
      owner.replace("127.0.0.1", "db.example.com"),
    ),
  ).toThrow();
  expect(() =>
    rlsEnvironment(
      { BETTER_AUTH_SECRET: "fixture-auth-key-at-least-32-characters" },
      {
        ...fixture,
        roles: {
          ...roles,
          webdock_registry_runtime:
            roles.webdock_registry_runtime + "?host=production",
        },
      },
      owner,
    ),
  ).toThrow();
  expect(() =>
    rlsEnvironment(
      { BETTER_AUTH_SECRET: "fixture-auth-key-at-least-32-characters" },
      { ...fixture, roles: { ...roles, webdock_auth_runtime: owner } },
      owner,
    ),
  ).toThrow();
});
