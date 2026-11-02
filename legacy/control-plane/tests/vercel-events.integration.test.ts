import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes, createHmac } from "node:crypto";
import { withDatabaseScope } from "@webdock/database-runtime";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "Vercel events verify bytes, use worker authority and deduplicate completed effects",
  async () => {
    const url = new URL(
      process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL!,
    );
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated fixture required");
    const db = new Pool({ connectionString: url.href });
    const base = BigInt(
      "97" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
    );
    const [subject, sid, native] = [0, 1, 2].map((n) =>
      (base + BigInt(n)).toString(),
    );
    const team = "team_event" + base,
      project = "prj_event" + base,
      eventID = "event-" + base;
    const values = {
      WEBDOCK_VERCEL_CLIENT_ID: "fixture",
      WEBDOCK_VERCEL_CLIENT_SECRET: "fixture-event-signature-key",
      WEBDOCK_VERCEL_INTEGRATION_SLUG: "fixture",
      WEBDOCK_VERCEL_TEAM_ID: team,
      WEBDOCK_VERCEL_TEAM_SLUG: "fixture",
      WEBDOCK_SSO_COOKIE_SECRET:
        "fixture-vercel-cookie-key-at-least-32-characters",
    };
    const previous = Object.fromEntries(
      Object.keys(values).map((key) => [key, process.env[key]]),
    );
    try {
      Object.assign(process.env, values);
      await db.query(
        `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,'Event fixture',$2,'operator',true,false,true)`,
        [subject, subject + "@example.invalid"],
      );
      await db.query(
        `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
        [sid, subject],
      );
      await db.query(
        `INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'Event fixture',$2,'operator',$3)`,
        [native, native + "@example.invalid", subject],
      );
      const { currentRegistryActor } =
        await import("../src/modules/integrations/operator");
      const { saveVercelConnection } =
        await import("../src/modules/integrations/vercel/vercel-store");
      await withDatabaseScope(
        { kind: "session", subject, sessionID: sid },
        async () =>
          saveVercelConnection(
            await currentRegistryActor(),
            {
              teamID: team,
              configurationID: "icfg_fixture",
              accessToken: "fixture-provider-token",
            },
            values.WEBDOCK_SSO_COOKIE_SECRET,
          ),
      );
      const { vercelRegionEvent } =
        await import("../src/modules/integrations/vercel/events");
      const payload = JSON.stringify({
        id: eventID,
        type: "project.created",
        payload: { team: { id: team }, project: { id: project } },
      });
      const request = (
        signature = createHmac("sha1", values.WEBDOCK_VERCEL_CLIENT_SECRET)
          .update(payload)
          .digest("hex"),
      ) =>
        new Request("https://studio.webdock.dev/api/vercel/events", {
          method: "POST",
          headers: { "x-vercel-signature": signature },
          body: payload,
        });
      let patched = false,
        calls = 0;
      const provider = async (
        _input: RequestInfo | URL,
        init?: RequestInit,
      ) => {
        calls++;
        if (init?.method === "PATCH") patched = true;
        return Response.json({
          id: project,
          accountId: team,
          resourceConfig: {
            functionDefaultRegions: [patched ? "fra1" : "iad1"],
            functionZeroConfigFailover: !patched,
          },
        });
      };
      expect(
        (await vercelRegionEvent(request("0".repeat(40)), provider)).status,
      ).toBe(403);
      expect(calls).toBe(0);
      const first = await vercelRegionEvent(request(), provider);
      expect(first.status).toBe(200);
      expect(await first.json()).toEqual({
        projectID: project,
        region: "fra1",
        changed: true,
      });
      expect(calls).toBe(3);
      expect((await vercelRegionEvent(request(), provider)).status).toBe(200);
      expect(calls).toBe(3);
      expect(
        (
          await db.query(
            "SELECT count(*)::int AS n FROM webdock_admin.audit_events WHERE target_i_d=$1",
            [project],
          )
        ).rows[0].n,
      ).toBe(1);
    } finally {
      for (const [key, value] of Object.entries(previous)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      await db.query(
        "DELETE FROM webdock_auth.provider_event_receipt WHERE event_id=$1",
        [eventID],
      );
      await db.query(
        "DELETE FROM webdock_admin.audit_events WHERE actor_id=$1",
        [native],
      );
      await db.query(
        "DELETE FROM webdock_admin.vercel_connection WHERE team_id=$1",
        [team],
      );
      await db.query("DELETE FROM webdock_admin.users WHERE id=$1", [native]);
      await db.query('DELETE FROM webdock_auth."user" WHERE id=$1', [subject]);
      await (await import("../src/modules/registry/context")).closeRegistry();
      await (await import("../src/db/runtime")).closeDatabases();
      await db.end();
    }
  },
);
