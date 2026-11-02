import { initializePluginState } from "../src/modules/plugins/bootstrap";
import { test, expect } from "bun:test";
import { Pool } from "pg";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "worker boots, finishes an empty storage batch and drains without bootstrap credentials",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated fixture required");
    const db = new Pool({ connectionString: url.href, max: 1 });
    if (
      Number(
        (
          await db.query(
            "SELECT count(*) AS n FROM webdock_auth.tenant_storage",
          )
        ).rows[0].n,
      ) !== 0
    ) {
      await db.end();
      throw Error("Worker smoke test requires an empty storage fixture");
    }
    const hadPluginBootstrap =
      (await db.query("SELECT singleton FROM webdock_auth.plugin_bootstrap"))
        .rows.length > 0;
    await initializePluginState(db, undefined, "legacy");
    const slot = new Date(Math.floor(Date.now() / 3600000) * 3600000);
    const existing = (
      await db.query(
        "SELECT id FROM webdock_auth.api_job WHERE name='storage.refresh' AND slot=$1",
        [slot],
      )
    ).rows[0];
    if (existing) {
      await db.end();
      throw Error(
        "This fixture hour already has a storage job; preserve it and use a fresh fixture",
      );
    }
    const worker = Bun.spawn(
      [
        process.execPath,
        "--no-env-file",
        new URL("../src/worker.ts", import.meta.url).pathname,
      ],
      {
        env: {
          PATH: process.env.PATH,
          HOME: process.env.HOME,
          NODE_ENV: "development",
          DATABASE_WORKER_URL: process.env.DATABASE_WORKER_URL,
          DATABASE_RLS_SECRET: process.env.DATABASE_RLS_SECRET,
          DATABASE_POOL_MAX: "1",
          BETTER_AUTH_SECRET: process.env.BETTER_AUTH_SECRET,
          BETTER_AUTH_URL: "http://localhost:3125",
          AUTH_TEST_MAIL: "true",
          WEBDOCK_WORKER_DOMAINS: "storage",
          WEBDOCK_JOBS_REDIS_URL: process.env.WEBDOCK_JOBS_REDIS_URL,
          WEBDOCK_JOBS_PREFIX: "worker-profile-" + Date.now(),
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    const output = new Response(worker.stderr).text();
    try {
      let state: string | undefined;
      for (let n = 0; n < 100; n++) {
        state = (
          await db.query(
            "SELECT status FROM webdock_auth.api_job WHERE name='storage.refresh' AND slot=$1",
            [slot],
          )
        ).rows[0]?.status;
        if (state === "succeeded" || worker.exitCode !== null) break;
        await Bun.sleep(50);
      }
      expect(state).toBe("succeeded");
      worker.kill("SIGTERM");
      expect(await worker.exited).toBe(0);
      expect(await output).not.toContain("job_failed");
    } finally {
      if (worker.exitCode === null) worker.kill("SIGKILL");
      await worker.exited;
      await db.query(
        "DELETE FROM webdock_auth.api_job WHERE name='storage.refresh' AND slot=$1",
        [slot],
      );
      if (!hadPluginBootstrap) {
        await db.query(
          "DELETE FROM webdock_auth.plugin_installation WHERE kind='builtin'",
        );
        await db.query("DELETE FROM webdock_auth.plugin_bootstrap");
      }
      await db.end();
    }
  },
);
