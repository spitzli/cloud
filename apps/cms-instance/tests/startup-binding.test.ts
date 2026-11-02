import test from "node:test";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

test("failed-init cache binding rejection and transient recovery in an isolated offline process", () => {
  execFileSync(
    process.execPath,
    [
      "test",
      fileURLToPath(new URL("./fixtures/startup-binding.mjs", import.meta.url)),
    ],
    {
      env: { PATH: process.env.PATH, HOME: process.env.HOME, NODE_ENV: "test" },
      stdio: "pipe",
      timeout: 10000,
    },
  );
});
