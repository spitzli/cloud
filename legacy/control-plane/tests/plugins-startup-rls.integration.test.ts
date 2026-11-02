import { test, expect } from "bun:test";
import { Pool } from "pg";
import { initializePluginState } from "../src/modules/plugins/bootstrap";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "API refuses missing plugin initialization and boots after explicit offline import",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated fixture required");
    const owner = new Pool({ connectionString: url.href, max: 1 });
    expect(
      (await owner.query("SELECT * FROM webdock_auth.plugin_bootstrap")).rows,
    ).toEqual([]);
    const reserved = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: () => new Response(null),
    });
    const port = reserved.port;
    await reserved.stop();
    const origin = `http://127.0.0.1:${port}`;
    const spawn = () =>
      Bun.spawn([process.execPath, "--no-env-file", "src/index.ts"], {
        env: {
          PATH: process.env.PATH,
          HOME: process.env.HOME,
          NODE_ENV: "development",
          DATABASE_URL: process.env.DATABASE_URL,
          AUTH_DATABASE_URL: process.env.AUTH_DATABASE_URL,
          DATABASE_WORKER_URL: process.env.DATABASE_WORKER_URL,
          DATABASE_RLS_SECRET: process.env.DATABASE_RLS_SECRET,
          DATABASE_POOL_MAX: "1",
          BETTER_AUTH_SECRET: process.env.BETTER_AUTH_SECRET,
          AUTH_TEST_MAIL: "true",
          BETTER_AUTH_URL: "http://localhost:3125",
          WEBDOCK_API_ORIGIN: origin,
          WEBDOCK_STUDIO_ORIGIN: "http://localhost:3120",
          WEBDOCK_API_DOMAINS: "",
          PORT: String(port),
        },
        stdout: "pipe",
        stderr: "pipe",
      });
    let child = spawn();
    let errors = new Response(child.stderr).text();
    try {
      for (let n = 0; n < 60 && child.exitCode === null; n++)
        await Bun.sleep(50);
      expect(child.exitCode).not.toBeNull();
      expect(child.exitCode).not.toBe(0);
      expect(await errors).toContain("Initialize plugin installation state");
      await initializePluginState(owner, undefined, "legacy");
      child = spawn();
      errors = new Response(child.stderr).text();
      let ready = false;
      for (let n = 0; n < 100; n++) {
        try {
          ready = (await fetch(origin + "/readyz")).ok;
          if (ready) break;
        } catch {}
        if (child.exitCode !== null) break;
        await Bun.sleep(50);
      }
      expect(ready).toBe(true);
      await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
      expect((await fetch(origin + "/readyz")).status).toBe(503);
    } finally {
      if (child.exitCode === null) child.kill("SIGTERM");
      await child.exited;
      await errors;
      await owner.query(
        "DELETE FROM webdock_auth.plugin_installation WHERE kind='builtin'",
      );
      await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
      await owner.end();
    }
  },
  15000,
);
