import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";

test("hosting authorization capability regressions run with an isolated runtime", () => {
  // clerk-provider initializes the DB pools on import. Give the child a local,
  // deliberately unreachable URL; every query in the fixture uses its fake DB.
  const result = spawnSync(process.execPath, [
    "--no-env-file", "test", "--preload", "./tests/helpers/no-network.ts",
    "./tests/helpers/hosting-permissions/authorization.test.ts",
  ], {
    cwd: new URL("..", import.meta.url),
    env: { NODE_ENV: "test", DATABASE_URL: "postgresql://fixture:fixture@127.0.0.1:1/webdock_test" },
    encoding: "utf8",
  });
  expect(result.error).toBeUndefined();
  expect(result.status, result.stdout + result.stderr).toBe(0);
});
