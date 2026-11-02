import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";
import { initializePluginState } from "../src/modules/plugins/bootstrap";
import { requestPluginState } from "../src/modules/plugins/store";
import { createModuleAdmission } from "../src/modules/plugins/admission";
import { readPublicInstallation } from "../src/modules/plugins/public-view";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "module admission fences state changes, shares nested transactions and denies stale snapshots",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated fixture required");
    const fixture = JSON.parse(await readFile(enabled!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const api = new ScopedPool(
      { connectionString: fixture.roles.webdock_auth_runtime, max: 2 },
      { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
    );
    const subject = randomUUID(),
      sessionID = randomUUID();
    const run = <T>(fn: () => T) =>
      withDatabaseScope({ kind: "session", subject, sessionID }, fn);
    const admit = createModuleAdmission(api);
    let admittedID: string | undefined;
    let release: () => void = () => {};
    let entered: () => void = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const acquired = new Promise<void>((resolve) => {
      entered = resolve;
    });
    try {
      await initializePluginState(owner, undefined, "legacy");
      await owner.query(
        'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,\'Admission fixture\',$2,\'operator\',true,false,true)',
        [subject, subject + "@example.invalid"],
      );
      await owner.query(
        'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
        [sessionID, subject],
      );
      const snapshot = await run(() => readPublicInstallation(api));
      expect(snapshot.modules.storage).toBe("enabled");
      const work = run(() =>
        admit("storage", "provision", async (ctx) => {
          admittedID = ctx.operationID!;
          await ctx.db.query(
            "INSERT INTO webdock_auth.access_event(id,actor_id,action,outcome) VALUES($1,$2,'plugin.admission.fixture','accepted')",
            [ctx.operationID, subject],
          );
          await admit("mail", "read", async (nested) => {
            expect(nested.db).toBe(ctx.db);
            expect(nested.operationID).toBe(ctx.operationID);
          });
          entered();
          await held;
        }),
      );
      await acquired;
      const transition = run(() =>
        requestPluginState(api, { subject, sessionID }, "storage", "disabled", {
          expectedRevision: "0",
          idempotencyKey: "drain",
        }),
      );
      // Observe the blocked exclusive advisory lock on a separate real connection.
      let waiting = false;
      for (let n = 0; n < 50; n++) {
        waiting = (
          await owner.query(
            "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND database=(SELECT oid FROM pg_database WHERE datname=current_database())) AS waiting",
          )
        ).rows[0].waiting;
        if (waiting) break;
        await Bun.sleep(10);
      }
      expect(waiting).toBe(true);
      release();
      await work;
      await transition;
      expect(snapshot.modules.storage).toBe("enabled");
      const current = await run(() => readPublicInstallation(api));
      expect(current.modules.storage).toBe("draining");
      expect(current.revision).not.toBe(snapshot.revision);
      let called = false;
      await expect(
        run(() =>
          admit("storage", "provision", async () => {
            called = true;
          }),
        ),
      ).rejects.toThrow();
      expect(called).toBe(false);
      await expect(
        run(() => admit("storage", "continue", async () => {})),
      ).rejects.toThrow();
      const continuation = {
        operationID: admittedID!,
        authorize: async (db: Pick<Pool, "query">) =>
          (
            await db.query(
              "SELECT id FROM webdock_auth.access_event WHERE id=$1 AND actor_id=$2 AND outcome='accepted'",
              [admittedID, subject],
            )
          ).rows.length === 1,
      };
      await run(() =>
        admit(
          "storage",
          "continue",
          async (ctx) => {
            expect(ctx.operationID).toBe(admittedID!);
          },
          { continuation },
        ),
      );
      await expect(
        run(() =>
          admit("storage", "continue", async () => {}, {
            continuation: { ...continuation, authorize: async () => false },
          }),
        ),
      ).rejects.toThrow();
      await expect(
        run(() =>
          admit("mail", "provision", async (ctx) => {
            try {
              await admit("storage", "provision", async () => {});
            } catch {}
            await ctx.db.query(
              "INSERT INTO webdock_auth.access_event(id,actor_id,action,outcome) VALUES($1,$2,'plugin.admission.rollback','accepted')",
              [ctx.operationID, subject],
            );
          }),
        ),
      ).rejects.toThrow();
      await expect(
        run(() =>
          admit("mail", "read", async (ctx) => {
            await ctx.db.query(
              "INSERT INTO webdock_auth.access_event(actor_id,action,outcome) VALUES($1,'plugin.admission.read-write','accepted')",
              [subject],
            );
          }),
        ),
      ).rejects.toThrow();
      await expect(
        run(() =>
          admit("mail", "read", () =>
            admit("mail", "provision", async () => {}),
          ),
        ),
      ).rejects.toThrow();
      await expect(
        run(() =>
          admit("mail", "provision", async (ctx) => {
            await ctx.db.query(
              "INSERT INTO webdock_auth.access_event(id,actor_id,action,outcome) VALUES($1,$2,'plugin.admission.rollback','accepted')",
              [ctx.operationID, subject],
            );
            try {
              await ctx.db.query("SELECT 1/0");
            } catch {}
            return "must not succeed";
          }),
        ),
      ).rejects.toThrow();
      for (const failure of [undefined, null, false, 0]) {
        const rejected = await run(() =>
          admit("mail", "provision", async (ctx) => {
            await ctx.db.query(
              "INSERT INTO webdock_auth.access_event(id,actor_id,action,outcome) VALUES($1,$2,'plugin.admission.rollback','accepted')",
              [ctx.operationID, subject],
            );
            try {
              await admit("mail", "read", async () => {
                throw failure;
              });
            } catch {}
          }),
        ).then(
          () => false,
          () => true,
        );
        expect(rejected).toBe(true);
      }
      await expect(
        run(() =>
          admit("mail", "provision", async (ctx) => {
            try {
              await ctx.db.query(
                "/* nested /* comment */ transaction */ COMMIT",
              );
            } catch {}
          }),
        ),
      ).rejects.toThrow();
      const stopped = new ScopedPool(
        { connectionString: fixture.roles.webdock_auth_runtime, max: 1 },
        { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
      );
      await stopped.end();
      await expect(
        run(() =>
          createModuleAdmission(stopped)("mail", "provision", async () => {
            throw Error("must not run");
          }),
        ),
      ).rejects.toThrow("pool");
      await expect(
        run(() =>
          admit("mail", "provision", async (ctx) => {
            await ctx.db.query(
              "INSERT INTO webdock_auth.access_event(id,actor_id,action,outcome) VALUES($1,$2,'plugin.admission.rollback','accepted')",
              [ctx.operationID, subject],
            );
            throw Error("rollback");
          }),
        ),
      ).rejects.toThrow("rollback");
      expect(
        (
          await owner.query(
            "SELECT id FROM webdock_auth.access_event WHERE actor_id=$1 AND action='plugin.admission.rollback'",
            [subject],
          )
        ).rows,
      ).toEqual([]);
    } finally {
      release();
      await owner.query(
        "DELETE FROM webdock_auth.plugin_operation WHERE actor_id=$1",
        [subject],
      );
      await owner.query(
        "DELETE FROM webdock_auth.plugin_installation WHERE kind='builtin'",
      );
      await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
      await owner.query(
        "DELETE FROM webdock_auth.access_event WHERE actor_id=$1",
        [subject],
      );
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
        subject,
      ]);
      await Promise.all([api.end(), owner.end()]);
    }
  },
);
