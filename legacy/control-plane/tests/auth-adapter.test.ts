import { expect, test } from "bun:test";
import { Pool } from "pg";
import { assertTestDatabase } from "./helpers/database";
const url = process.env.API_TEST_DATABASE_URL;
const integration = url ? test : test.skip;
integration(
  "Drizzle auth reads an existing session without replacing identities or granting cross-origin writes",
  async () => {
    assertTestDatabase(url!);
    const { createAuthentication } = await import("../src/modules/auth/auth");
    const db = new Pool({
      connectionString: url,
      options: "-c search_path=webdock_auth,pg_catalog",
      max: 1,
    });
    try {
      await db.query(
        "INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true) ON CONFLICT DO NOTHING",
      );
      const { rows } = await db.query(
        `INSERT INTO webdock_auth."user"(name,email,"emailVerified","mustChangePassword",role) VALUES('Adapter fixture','adapter@example.invalid',true,false,'user') RETURNING id`,
      );
      const id = rows[0].id;
      const auth = createAuthentication({
        database: db,
        secret: "adapter-fixture-secret-at-least-32-characters",
        baseURL: "http://localhost:3125",
      });
      await auth.$context;
      const stored = await auth.$context.then((ctx) =>
        ctx.internalAdapter.createSession(id),
      );
      expect(stored.userId).toBe(id);
      const session = await auth.$context.then((ctx) =>
        ctx.internalAdapter.findSession(stored.token),
      );
      expect(session?.user.id).toBe(id);
      expect(session?.session.id).toBe(stored.id);
      const { serializeSignedCookie } = await import("better-call");
      const context = await auth.$context;
      const cookie = await serializeSignedCookie(
        context.authCookies.sessionToken.name,
        stored.token,
        context.secret,
      );
      const denied = await auth.handler(
        new Request("http://localhost:3125/api/auth/change-password", {
          method: "POST",
          headers: {
            origin: "https://customer.webdock.dev",
            cookie: cookie.split(";")[0],
            "content-type": "application/json",
          },
          body: JSON.stringify({
            currentPassword: "old-password",
            newPassword: "new-password-long-enough",
          }),
        }),
      );
      expect(denied.status).toBe(403);
      expect(
        (
          await db.query(
            'SELECT count(*)::int AS n FROM webdock_auth."user" WHERE email=$1',
            ["adapter@example.invalid"],
          )
        ).rows[0].n,
      ).toBe(1);
      await db.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        stored.id,
      ]);
      expect(
        await auth.$context.then((ctx) =>
          ctx.internalAdapter.findSession(stored.token),
        ),
      ).toBeNull();
      await db.query('DELETE FROM webdock_auth."user" WHERE id=$1', [id]);
    } finally {
      await db.end();
    }
  },
);
