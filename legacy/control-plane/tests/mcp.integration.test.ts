import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes, createHash } from "node:crypto";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "MCP HTTP authenticates real scoped OAuth tokens and observes session revocation",
  async () => {
    const url = new URL(
      process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL!,
    );
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Use the isolated RLS fixture");
    const db = new Pool({ connectionString: url.href });
    const base = BigInt(
      "96" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
    );
    const [user, sid, native, clientID] = [0, 1, 2, 3].map((n) =>
      (base + BigInt(n)).toString(),
    );
    const keys = [
      "WEBDOCK_STUDIO_ORIGIN",
      "WEBDOCK_AUTH_ISSUER",
      "WEBDOCK_SSO_CLIENT_ID",
      "WEBDOCK_SSO_CLIENT_SECRET",
    ];
    const saved = Object.fromEntries(
      keys.map((key) => [key, process.env[key]]),
    );
    const origin = "http://localhost:3125",
      studio = "https://studio.webdock.dev";
    try {
      Object.assign(process.env, {
        WEBDOCK_STUDIO_ORIGIN: studio,
        WEBDOCK_AUTH_ISSUER: origin + "/api/auth",
      });
      await db.query(
        `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,'MCP fixture',$2,'operator',true,false,true)`,
        [user, user + "@example.invalid"],
      );
      await db.query(
        `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
        [sid, user],
      );
      await db.query(
        `INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES($1,'MCP fixture',$2,'operator',$3)`,
        [native, native + "@example.invalid", user],
      );
      const { auth } = await import("../src/modules/auth/auth");
      const { offlineProvisioning } =
        await import("../src/modules/auth/offline");
      const { serializeSignedCookie } = await import("better-call");
      const context = await auth.$context,
        cookie = await serializeSignedCookie(
          context.authCookies.sessionToken.name,
          sid,
          context.secret,
        ),
        headers = new Headers({ origin, cookie: cookie.split(";")[0] });
      const redirect = "https://fixture.example.invalid/callback";
      const client = await offlineProvisioning.run({ clientID }, () =>
        auth.api.adminCreateOAuthClient({
          headers,
          body: {
            client_name: "MCP fixture",
            redirect_uris: [redirect],
            application_type: "web",
            scope: "webdock:read webdock:write",
            grant_types: ["authorization_code"],
            response_types: ["code"],
            token_endpoint_auth_method: "client_secret_post",
            require_pkce: true,
            skip_consent: true,
            metadata: { webdock_mcp: true },
          },
        }),
      );
      await auth.api.adminLinkClientResource({
        headers,
        params: {
          identifier: studio + "/api/mcp",
          client_id: client.client_id,
        },
      });
      process.env.WEBDOCK_SSO_CLIENT_ID = client.client_id;
      process.env.WEBDOCK_SSO_CLIENT_SECRET = client.client_secret!;
      const verifier = randomBytes(32).toString("base64url"),
        params = new URLSearchParams({
          client_id: client.client_id,
          redirect_uri: redirect,
          response_type: "code",
          scope: "webdock:read",
          resource: studio + "/api/mcp",
          code_challenge: createHash("sha256")
            .update(verifier)
            .digest("base64url"),
          code_challenge_method: "S256",
          state: "fixture",
        });
      const authorize = await auth.handler(
        new Request(origin + "/api/auth/oauth2/authorize?" + params, {
          headers,
        }),
      );
      expect(authorize.status).toBe(302);
      const code = new URL(authorize.headers.get("location")!).searchParams.get(
        "code",
      )!;
      expect(code).toBeTruthy();
      const response = await auth.handler(
        new Request(origin + "/api/auth/oauth2/token", {
          method: "POST",
          headers: { "content-type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({
            grant_type: "authorization_code",
            client_id: client.client_id,
            client_secret: client.client_secret!,
            redirect_uri: redirect,
            code,
            code_verifier: verifier,
            resource: studio + "/api/mcp",
          }),
        }),
      );
      expect(response.status).toBe(200);
      const token = (await response.json()).access_token;
      const { createApp } = await import("../src/app");
      const { readConfig } = await import("../src/config");
      const { mcpProtocol } = await import("../src/protocols/mcp");
      const config = readConfig({
          NODE_ENV: "development",
          WEBDOCK_STUDIO_ORIGIN: studio,
        }),
        app = await createApp(config, { mounts: mcpProtocol(config) });
      const call = (method: string, body: unknown = {}) =>
        app.handle(
          new Request(studio + "/api/mcp", {
            method: "POST",
            headers: {
              authorization: "Bearer " + token,
              "content-type": "application/json",
              accept: "application/json, text/event-stream",
            },
            body: JSON.stringify({
              jsonrpc: "2.0",
              id: 1,
              method,
              params: body,
            }),
          }),
        );
      const list = await call("tools/list");
      expect(list.status).toBe(200);
      const tools = (await list.json()).result.tools;
      expect(
        tools.some(
          (tool: { name: string }) =>
            tool.name === "list_billing_product_versions",
        ),
      ).toBe(false);
      expect(
        tools.some(
          (tool: { name: string }) =>
            tool.name === "publish_billing_product_version",
        ),
      ).toBe(false);
      const removedTool = await call("tools/call", {
        name: "publish_billing_product_version",
        arguments: { environment: "live", versionID: "1" },
      });
      expect(removedTool.status).toBe(200);
      const removedToolBody = await removedTool.json();
      expect(Boolean(removedToolBody.error || removedToolBody.result?.isError)).toBe(true);

      expect(
        tools.some((tool: { name: string }) => tool.name === "list_records"),
      ).toBe(true);
      expect(
        tools.some((tool: { name: string }) => tool.name === "save_customer"),
      ).toBe(false);
      expect(
        (
          await call("tools/call", {
            name: "save_customer",
            arguments: { data: { name: "Denied" } },
          })
        ).status,
      ).toBe(403);
      await db.query("DELETE FROM webdock_auth.session WHERE id=$1", [sid]);
      expect((await call("tools/list")).status).toBe(401);
    } finally {
      await db.query(
        'DELETE FROM webdock_auth."oauthClient" WHERE "clientId"=$1',
        [clientID],
      );
      await db.query("DELETE FROM webdock_admin.users WHERE id=$1", [native]);
      await db.query('DELETE FROM webdock_auth."user" WHERE id=$1', [user]);
      for (const key of keys) {
        if (saved[key] === undefined) delete process.env[key];
        else process.env[key] = saved[key];
      }
      await (await import("../src/modules/registry/context")).closeRegistry();
      await (await import("../src/db/runtime")).closeDatabases();
      await db.end();
    }
  },
  15_000,
);
