import { initializePluginState } from "../src/modules/plugins/bootstrap";
import { test, expect } from "bun:test";
import { randomBytes } from "node:crypto";
import { Pool } from "pg";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "network server preserves Auth host cookies and serves a redacted frontend session",
  async () => {
    const fixtureURL = new URL(process.env.API_TEST_OWNER_URL!);
    if (
      fixtureURL.hostname !== "127.0.0.1" ||
      fixtureURL.pathname !== "/webdock_rls_test"
    )
      throw Error("Isolated restricted fixture required");
    const owner = new Pool({ connectionString: fixtureURL.href, max: 1 });
    const reservation = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: () => new Response(null),
    });
    const port = reservation.port;
    await reservation.stop();
    const apiOrigin = `http://127.0.0.1:${port}`,
      authOrigin = "http://localhost:3125",
      ingressSecret = randomBytes(32).toString("hex");
    const { hashPassword } = await import("better-auth/crypto");
    const email = "http-" + randomBytes(6).toString("hex") + "@example.invalid",
      password = randomBytes(24).toString("hex");
    const user = (
      await owner.query(
        `INSERT INTO webdock_auth."user"(name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword") VALUES($1,$2,true,'user',false,false) RETURNING id`,
        ["HTTP fixture", email],
      )
    ).rows[0];
    await owner.query(
      `INSERT INTO webdock_auth.account("accountId","providerId","userId",password,"updatedAt") VALUES($1,'credential',$1,$2,now())`,
      [user.id, await hashPassword(password)],
    );
    const hadPluginBootstrap =
      (await owner.query("SELECT singleton FROM webdock_auth.plugin_bootstrap"))
        .rows.length > 0;
    await initializePluginState(owner, undefined, "legacy");
    const child = Bun.spawn(
      [
        process.execPath,
        "--no-env-file",
        new URL("../src/index.ts", import.meta.url).pathname,
      ],
      {
        env: {
          PATH: process.env.PATH,
          HOME: process.env.HOME,
          NODE_ENV: "development",
          DATABASE_URL: process.env.DATABASE_URL,
          AUTH_DATABASE_URL: process.env.AUTH_DATABASE_URL,
          DATABASE_WORKER_URL: process.env.DATABASE_WORKER_URL,
          DATABASE_RLS_SECRET: process.env.DATABASE_RLS_SECRET,
          AUTH_TEST_MAIL: "true",
          BETTER_AUTH_SECRET: process.env.BETTER_AUTH_SECRET,
          BETTER_AUTH_URL: authOrigin,
          WEBDOCK_API_ORIGIN: apiOrigin,
          WEBDOCK_STUDIO_ORIGIN: "http://localhost:3120",
          WEBDOCK_API_DOMAINS: "auth",
          WEBDOCK_INGRESS_SECRET: ingressSecret,
          WEBDOCK_TRUSTED_PROXIES: "127.0.0.1/32",
          PORT: String(port),
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    try {
      let ready = false;
      for (let n = 0; n < 100; n++) {
        try {
          ready = (await fetch(apiOrigin + "/readyz")).ok;
          if (ready) break;
        } catch {}
        await Bun.sleep(50);
      }
      expect(ready).toBe(true);
      const headers = {
        "x-forwarded-host": "localhost:3125",
        "x-forwarded-proto": "http",
        "x-forwarded-for": "192.0.2.20",
        "x-webdock-ingress-token": ingressSecret,
        origin: authOrigin,
        "content-type": "application/json",
      };
      const discovery = await fetch(
        apiOrigin + "/.well-known/openid-configuration",
        { headers, redirect: "manual" },
      );
      expect(discovery.status).toBe(200);
      expect((await discovery.json()).issuer).toBe(authOrigin + "/api/auth");
      const login = await fetch(apiOrigin + "/api/auth/sign-in/email", {
        method: "POST",
        headers,
        body: JSON.stringify({ email, password }),
        redirect: "manual",
      });
      expect(login.status).toBe(200);
      const cookies = login.headers.getSetCookie();
      expect(cookies.length).toBeGreaterThan(0);
      expect(cookies.join(";")).not.toContain("Domain=");
      const session = await fetch(apiOrigin + "/v1/auth/session", {
        headers: {
          cookie: cookies.map((value) => value.split(";")[0]).join("; "),
        },
      });
      const body = await session.json();
      expect(body.data.user.id).toBe(user.id);
      expect(body.data.session).not.toHaveProperty("token");
      expect((await fetch(apiOrigin + "/api/auth/get-session")).status).toBe(
        404,
      );
    } finally {
      child.kill("SIGTERM");
      const exit = await child.exited;
      if (exit !== 0) console.error(await new Response(child.stderr).text());
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
        user.id,
      ]);
      if (!hadPluginBootstrap) {
        await owner.query(
          "DELETE FROM webdock_auth.plugin_installation WHERE kind='builtin'",
        );
        await owner.query("DELETE FROM webdock_auth.plugin_bootstrap");
      }
      await owner.end();
    }
  },
  15_000,
);
