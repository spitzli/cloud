import { test } from "node:test";
import { execFileSync } from "node:child_process";
for (const mode of ["legacy", "adapter", "console"]) {
  test(`${mode} authentication uses its own transport and fresh credentials`, () => {
    execFileSync(
      process.execPath,
      [
        "--conditions=react-server",
        new URL("./fixtures/browser-transport.ts", import.meta.url).pathname,
        mode,
      ],
      { stdio: "pipe" },
    );
  });
}
