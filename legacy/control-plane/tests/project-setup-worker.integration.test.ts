import { initializePluginState } from "../src/modules/plugins/bootstrap";
import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomUUID } from "node:crypto";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createSetupFixture } from "./helpers/setup-fixture";
import { executeHosting } from "../src/modules/hosting/service";
import { executeSetupStep } from "../src/modules/hosting/setup-execution";
import { workerDatabase } from "../src/db/runtime";
import {
  claimOperation,
  completeOperation,
} from "../src/modules/hosting/operations";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "setup worker uses restricted credentials, fences stale claims and resumes to observed readiness",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Restricted local fixture required");
    const db = new Pool({ connectionString: url.href, max: 1 });
    const hadPluginBootstrap =
      (await db.query("SELECT singleton FROM webdock_auth.plugin_bootstrap"))
        .rows.length > 0;
    const f = await createSetupFixture(db);
    const old = process.env.WEBDOCK_SETUP_ENABLED;
    process.env.WEBDOCK_SETUP_ENABLED = "true";
    let worker: import("bun").Subprocess<"pipe", "pipe", "pipe"> | undefined;
    try {
      const saved = await executeHosting(f.actor, {
        action: "setup.save",
        projectID: f.projectID,
        revision: 0,
        configuration: {
          name: "restricted-portal",
          source: { kind: "template", template: "healthcheck" },
          spec: {
            args: [],
            port: 8080,
            healthPath: "/ping",
            cpuMillicores: 100,
            memoryBytes: 67108864,
            ephemeralBytes: 67108864,
            replicas: 1,
          },
          database: { mode: "none" },
          mail: null,
          autoPublish: false,
        },
        idempotencyKey: randomUUID(),
      });
      const review = await executeHosting(f.actor, {
        action: "setup.preview",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
      });
      expect(review.preview.blockers).toEqual([]);
      await executeHosting(f.actor, {
        action: "setup.confirm",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
        previewID: review.preview.id,
        acceptedOfferIDs: [],
        idempotencyKey: randomUUID(),
      });
      const row = (
        await db.query(
          "UPDATE webdock_auth.hosting_setup SET generation=generation+1,lease_until=now()+interval '60 seconds' WHERE id=$1 RETURNING *",
          [saved.id],
        )
      ).rows[0];
      const scope = {
        kind: "system" as const,
        purpose: "hosting-setup",
        customerID: f.customerID,
        setupID: saved.id,
        generation: row.generation,
      };
      const flags = await withDatabaseScope(scope, () =>
        workerDatabase.query(
          "SELECT webdock_rls.setup_allowed($1,$2) AS allowed,webdock_rls.setup_identity($3,$4) AS identity",
          [f.customerID, f.projectID, f.actor.subject, f.actor.sessionID],
        ),
      );
      expect(flags.rows[0]).toEqual({ allowed: true, identity: true });
      await expect(
        withDatabaseScope(scope, () =>
          workerDatabase.query(
            "UPDATE webdock_auth.hosting_setup SET actor='{}' WHERE id=$1",
            [saved.id],
          ),
        ),
      ).rejects.toThrow();
      const visible = await withDatabaseScope(scope, () =>
        workerDatabase.query(
          'SELECT u.id FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id WHERE u.id=$1 AND s.id=$2 FOR SHARE OF u,s',
          [f.actor.subject, f.actor.sessionID],
        ),
      );
      expect(visible.rowCount).toBe(1);
      const first = await withDatabaseScope(scope, () => executeSetupStep(row));
      expect(first.resources.appID).toBeTruthy();
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET resources=$2,steps=$3,generation=generation+1,lease_until=NULL WHERE id=$1",
        [
          saved.id,
          JSON.stringify(first.resources),
          JSON.stringify(first.steps),
        ],
      );
      await expect(
        withDatabaseScope(scope, () => executeSetupStep(row)),
      ).rejects.toThrow();
      await initializePluginState(db, undefined, "legacy");
      worker = Bun.spawn([process.execPath, "--no-env-file", "src/worker.ts"], {
        cwd: new URL("../", import.meta.url).pathname,
        env: {
          PATH: process.env.PATH,
          NODE_ENV: "development",
          DATABASE_WORKER_URL: process.env.DATABASE_WORKER_URL,
          DATABASE_RLS_SECRET: process.env.DATABASE_RLS_SECRET,
          DATABASE_POOL_MAX: "2",
          BETTER_AUTH_SECRET: process.env.BETTER_AUTH_SECRET,
          BETTER_AUTH_URL: "http://localhost:3125",
          AUTH_TEST_MAIL: "true",
          WEBDOCK_WORKER_DOMAINS: "hosting",
          WEBDOCK_SETUP_ENABLED: "true",
        },
        stdout: "pipe",
        stderr: "pipe",
      });
      const errors = new Response(worker.stderr).text();
      let setup;
      for (let n = 0; n < 600; n++) {
        setup = (
          await db.query(
            "SELECT state,reason,resources FROM webdock_auth.hosting_setup WHERE id=$1",
            [saved.id],
          )
        ).rows[0];
        if (
          setup.resources.operationID ||
          setup.state === "blocked" ||
          worker.exitCode !== null
        )
          break;
        await Bun.sleep(50);
      }
      expect(String(setup.state)).toBe("running");
      expect(setup.resources.operationID).toBeTruthy();
      const job = await withDatabaseScope(
        { kind: "system", purpose: "agent", clusterID: f.clusterID },
        () => claimOperation(f.agent),
      );
      expect(job).toBeTruthy();
      await withDatabaseScope(
        { kind: "system", purpose: "agent", clusterID: f.clusterID },
        () =>
          completeOperation(f.agent, {
            id: job.id,
            generation: job.generation,
            outcome: "succeeded",
            proof: {
              appID: first.resources.appID,
              revision: job.target_revision,
              namespace: "wd-" + f.projectID,
              uid: "restricted-fixture-app",
              ready: true,
              deleted: false,
              replicas: 1,
            },
          }),
      );
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET next_attempt_at=now() WHERE id=$1",
        [saved.id],
      );
      for (let n = 0; n < 600; n++) {
        setup = (
          await db.query(
            "SELECT state,reason FROM webdock_auth.hosting_setup WHERE id=$1",
            [saved.id],
          )
        ).rows[0];
        if (setup.state !== "running") break;
        await Bun.sleep(50);
      }
      expect(String(setup.state)).toBe("ready");
      expect(
        (
          await db.query(
            "SELECT count(*)::int AS n FROM webdock_auth.hosting_app WHERE project_id=$1",
            [f.projectID],
          )
        ).rows[0].n,
      ).toBe(1);
      const currentApp = await executeHosting(f.actor, {
        action: "apps.get",
        appID: first.resources.appID!,
      });
      await executeHosting(f.actor, {
        action: "apps.update",
        appID: first.resources.appID!,
        revision: currentApp.revision,
        spec: currentApp.spec,
        environment: [{ name: "TOKEN", value: "new-fixture-value" }],
        subscriptionRevision: currentApp.subscriptionRevision,
        idempotencyKey: randomUUID(),
      });
      worker.kill("SIGTERM");
      expect(await worker.exited).toBe(0);
      expect(await errors).not.toContain("job_failed");
    } finally {
      if (worker && worker.exitCode === null) {
        worker.kill("SIGKILL");
        await worker.exited;
      }
      if (old === undefined) delete process.env.WEBDOCK_SETUP_ENABLED;
      else process.env.WEBDOCK_SETUP_ENABLED = old;
      await f.cleanup();
      if (!hadPluginBootstrap) {
        await db.query(
          "DELETE FROM webdock_auth.plugin_installation WHERE kind='builtin'",
        );
        await db.query("DELETE FROM webdock_auth.plugin_bootstrap");
      }
      await db.end();
    }
  },
  90000,
);
(enabled ? test : test.skip)(
  "blocked draft environment repair runs under the tenant role and preserves resources",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Restricted local fixture required");
    const db = new Pool({ connectionString: url.href, max: 1 }),
      f = await createSetupFixture(db);
    try {
      const configuration = {
        name: "Repair fixture",
        source: { kind: "template", template: "healthcheck" },
        spec: {
          args: [],
          port: 8080,
          healthPath: "/ping",
          cpuMillicores: 100,
          memoryBytes: 67108864,
          ephemeralBytes: 67108864,
          replicas: 1,
        },
        database: { mode: "none" },
        mail: null,
        autoPublish: false,
      };
      const saved = await executeHosting(f.actor, {
        action: "setup.save",
        projectID: f.projectID,
        revision: 0,
        configuration,
        environment: [{ name: "CONFLICT", value: "private" }],
        idempotencyKey: randomUUID(),
      });
      const app = await executeHosting(f.actor, {
        action: "apps.prepare",
        projectID: f.projectID,
        name: configuration.name,
        spec: configuration.spec,
        environment: [{ name: "CONFLICT", value: "private" }],
        subscriptionRevision: 1,
        idempotencyKey: randomUUID(),
      });
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET state='blocked',resources=$2,steps=$3 WHERE id=$1",
        [
          saved.id,
          JSON.stringify({ appID: app.appID }),
          JSON.stringify([
            {
              name: "bindings",
              state: "blocked",
              operationID: null,
              reason: "conflict",
            },
          ]),
        ],
      );
      const repair = {
        action: "setup.save",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
        configuration,
        environment: [{ name: "CONFLICT", value: null }],
        idempotencyKey: randomUUID(),
      };
      await expect(
        executeHosting({ ...f.actor, scopes: ["hosting:read"] }, repair),
      ).rejects.toThrow();
      const fixed = await executeHosting(f.actor, repair);
      expect(fixed.environmentNames).toEqual([]);
      expect(fixed.resources.appID).toBe(app.appID);
      expect(fixed.state).toBe("blocked");
      expect(
        (
          await executeHosting(f.actor, {
            action: "apps.get",
            appID: app.appID,
          })
        ).environmentNames,
      ).toEqual([]);
    } finally {
      await f.cleanup();
      await db.end();
    }
  },
);
