import test from "node:test";
import { execFileSync } from "node:child_process";

test("Console reads overlap independent work and remain isolated to each render", () => {
  execFileSync(process.execPath, [
    "--conditions=react-server",
    new URL("./fixtures/console-data.ts", import.meta.url).pathname,
  ], { stdio: "pipe" });
});
