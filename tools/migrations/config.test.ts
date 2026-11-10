import { test } from "node:test";
import assert from "node:assert/strict";
import { migrationConfig, runtimeRoles } from "./config.ts";
const base = {
  DATABASE_URL_UNPOOLED:
    "postgresql://webdock_offline_migrate:secret@127.0.0.1:5544/webdock",
  DATABASE_CONTEXT_SECRET: "a".repeat(48),
};
test("offline migration requires an exact explicit target and dedicated role", () => {
  assert.equal(
    migrationConfig(base, "127.0.0.1:5544/webdock").bootstrap,
    false,
  );
  assert.throws(() => migrationConfig(base, "other:5544/webdock"));
  assert.throws(() =>
    migrationConfig(
      {
        ...base,
        DATABASE_URL_UNPOOLED:
          "postgresql://postgres:secret@127.0.0.1:5544/webdock",
      },
      "127.0.0.1:5544/webdock",
    ),
  );
  assert.throws(() =>
    migrationConfig(
      { ...base, DATABASE_CONTEXT_SECRET: "" },
      "127.0.0.1:5544/webdock",
    ),
  );
});
test("fresh role bootstrap is explicit and requires all strong private passwords", () => {
  const env = {
    ...base,
    SPITZLI_BOOTSTRAP_EMPTY_DATABASE: "true",
    MIGRATION_ROLE_PASSWORDS: JSON.stringify(
      Object.fromEntries(
        [...runtimeRoles, "webdock_offline_migrate"].map((role) => [
          role,
          "private".repeat(8) + role,
        ]),
      ),
    ),
  };
  assert.equal(migrationConfig(env, "127.0.0.1:5544/webdock").bootstrap, true);
  assert.throws(() =>
    migrationConfig(
      { ...env, MIGRATION_ROLE_PASSWORDS: "{}" },
      "127.0.0.1:5544/webdock",
    ),
  );
  assert.throws(() =>
    migrationConfig(
      {
        ...base,
        DATABASE_URL_UNPOOLED:
          "postgresql://webdock_offline_migrate:secret@remote.example/webdock",
      },
      "remote.example:5432/webdock",
    ),
  );
});

test("bootstrap rejects shared runtime and offline passwords", () => {
  assert.throws(
    () =>
      migrationConfig(
        {
          ...base,
          SPITZLI_BOOTSTRAP_EMPTY_DATABASE: "true",
          MIGRATION_ROLE_PASSWORDS: JSON.stringify(
            Object.fromEntries(
              [...runtimeRoles, "webdock_offline_migrate"].map((role) => [
                role,
                "shared-password-".repeat(3),
              ]),
            ),
          ),
        },
        "127.0.0.1:5544/webdock",
      ),
    /distinct passwords/,
  );
});
