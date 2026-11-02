import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

function allowedOrigins(origin?: string) {
  const env = { ...process.env };
  delete env.NEXT_PUBLIC_SERVER_URL;
  delete env.WEBDOCK_CONSOLE_ORIGIN;
  if (origin) env.NEXT_PUBLIC_SERVER_URL = origin;
  return JSON.parse(execFileSync(process.execPath, ["-e",
    "import config from './next.config.ts'; console.log(JSON.stringify(config.experimental.serverActions.allowedOrigins ?? []))",
  ], { cwd: new URL("..", import.meta.url), env, encoding: "utf8" }));
}

test("Server Actions trust only the configured public Console host behind Vercel", () => {
  assert.deepEqual(allowedOrigins("https://console.webdock.dev"), ["console.webdock.dev"]);
});

test("unconfigured deployments do not add cross-origin Server Actions trust", () => {
  assert.deepEqual(allowedOrigins(), []);
});


test("Auth Server Actions trust only the configured public Better Auth host", () => {
  const env = { ...process.env, BETTER_AUTH_URL: "https://auth.webdock.dev" };
  const result = execFileSync(process.execPath, ["-e",
    "import config from './next.config.ts'; console.log(JSON.stringify(config.experimental.serverActions.allowedOrigins))",
  ], { cwd: new URL("../../auth/", import.meta.url), env, encoding: "utf8" });
  assert.deepEqual(JSON.parse(result), ["auth.webdock.dev"]);
});
