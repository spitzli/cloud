import { verifyPluginState } from "../src/modules/plugins/readiness";
import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { Pool } from "pg";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";
import { initializePluginState } from "../src/modules/plugins/bootstrap";
import {
  requestPluginState,
  readInstalledPlugins,
} from "../src/modules/plugins/store";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "plugin state bootstrap, CAS and idempotency use restricted live authority",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated RLS fixture required");
    const fixture = JSON.parse(await readFile(enabled!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 2 });
    const api = new ScopedPool(
      { connectionString: fixture.roles.webdock_auth_runtime, max: 3 },
      { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
    );
    const raw = new Pool({
      connectionString: fixture.roles.webdock_worker_runtime,
      max: 1,
    });
    const subject = randomUUID(),
      sessionID = randomUUID();
    const run = <T>(fn: () => T) =>
      withDatabaseScope({ kind: "session", subject, sessionID }, fn);
    try {
      await expect(verifyPluginState(owner)).rejects.toThrow(
        "Initialize plugin installation state",
      );
      await initializePluginState(owner, undefined, "legacy");
      await verifyPluginState(owner);
      const policyCheck = await owner.connect();
      try {
        await policyCheck.query("BEGIN");
        await policyCheck.query(
          "DROP POLICY webdock_auth_runtime_read ON webdock_auth.plugin_installation",
        );
        await expect(verifyPluginState(policyCheck)).rejects.toThrow(
          "Required plugin isolation policies",
        );
      } finally {
        await policyCheck.query("ROLLBACK");
        policyCheck.release();
      }

      expect(
        (await readInstalledPlugins(owner)).filter(
          (p) => p.state === "enabled",
        ),
      ).toHaveLength(4);
      await initializePluginState(
        owner,
        { configVersion: 1, modules: {} },
        "fresh",
      );
      expect(
        (await readInstalledPlugins(owner)).filter(
          (p) => p.state === "enabled",
        ),
      ).toHaveLength(4);
      await owner.query(
        'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,\'Plugin operator fixture\',$2,\'operator\',true,false,true)',
        [subject, subject + "@example.invalid"],
      );
      await owner.query(
        'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
        [sessionID, subject],
      );
      const results = await Promise.allSettled(
        ["a", "b"].map((key) =>
          run(() =>
            requestPluginState(
              api,
              { subject, sessionID },
              "storage",
              "disabled",
              { expectedRevision: "0", idempotencyKey: key },
            ),
          ),
        ),
      );
      if (results.every((r) => r.status === "rejected"))
        throw new AggregateError(
          results.map((r) => (r as PromiseRejectedResult).reason),
          "Both CAS requests failed",
        );
      expect(results.filter((r) => r.status === "fulfilled")).toHaveLength(1);
      const index = results.findIndex((r) => r.status === "fulfilled");
      const key = index === 0 ? "a" : "b";
      const first = (
        results[index] as PromiseFulfilledResult<
          Awaited<ReturnType<typeof requestPluginState>>
        >
      ).value;
      expect(
        await run(() =>
          requestPluginState(
            api,
            { subject, sessionID },
            "storage",
            "disabled",
            { expectedRevision: "0", idempotencyKey: key },
          ),
        ),
      ).toEqual(first);
      await expect(
        run(() =>
          requestPluginState(
            api,
            { subject, sessionID },
            "storage",
            "enabled",
            { expectedRevision: "0", idempotencyKey: key },
          ),
        ),
      ).rejects.toThrow();
      expect(
        (await readInstalledPlugins(owner)).find((p) => p.id === "storage")
          ?.state,
      ).toBe("draining");
      await expect(
        run(() =>
          requestPluginState(
            api,
            { subject, sessionID },
            "storage",
            "enabled",
            { expectedRevision: "1", idempotencyKey: "overlap" },
          ),
        ),
      ).rejects.toThrow();
      expect(
        (await readInstalledPlugins(owner)).find((p) => p.id === "storage")
          ?.revision,
      ).toBe("1");
      await owner.query(
        "UPDATE webdock_auth.plugin_operation SET status='succeeded' WHERE id=$1",
        [first.id],
      );
      expect(
        (
          await run(() =>
            requestPluginState(
              api,
              { subject, sessionID },
              "storage",
              "enabled",
              { expectedRevision: "1", idempotencyKey: "after-completion" },
            ),
          )
        ).status,
      ).toBe("pending");
      await raw.query("SELECT set_config('webdock.rls_context',$1,false)", [
        JSON.stringify({
          scope: { kind: "system", purpose: "plugin-controller" },
        }),
      ]);
      expect(
        (await raw.query("SELECT * FROM webdock_auth.plugin_operation")).rows,
      ).toEqual([]);
      await owner.query(
        "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
        [subject],
      );
      await expect(
        run(() =>
          requestPluginState(api, { subject, sessionID }, "mail", "disabled", {
            expectedRevision: "0",
            idempotencyKey: "denied",
          }),
        ),
      ).rejects.toThrow();
      await owner.query("DELETE FROM webdock_auth.plugin_operation");
      await owner.query("DELETE FROM webdock_auth.plugin_installation");
      await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
      await expect(
        initializePluginState(owner, undefined, "fresh"),
      ).rejects.toThrow();
      await initializePluginState(
        owner,
        { configVersion: 1, modules: { storage: { enabled: true } } },
        "fresh",
      );
      expect(
        (await readInstalledPlugins(owner))
          .filter((p) => p.state === "enabled")
          .map((p) => p.id),
      ).toEqual(["storage"]);
      await owner.query("DELETE FROM webdock_auth.plugin_installation");
      await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
    } finally {
      await owner.query("DELETE FROM webdock_auth.plugin_operation");
      await owner.query("DELETE FROM webdock_auth.plugin_installation");
      await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
      await owner.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        sessionID,
      ]);
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
        subject,
      ]);
      await Promise.all([owner.end(), api.end(), raw.end()]);
    }
  },
);
