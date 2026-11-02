import { expect, test } from "bun:test";

for (const selected of ["clerk", "better-auth"]) {
  test(`Studio safe session reflects ${selected} authority through native and delegated transports`, () => {
    const result = Bun.spawnSync(
      [
        process.execPath,
        "--no-env-file",
        "-e",
        `
      import { mock } from "bun:test";
      import assert from "node:assert/strict";
      let effective = { subject: "123", sessionID: "session", role: "user", banned: false, emailVerified: true, twoFactorEnabled: true, mustChangePassword: false, preview: false };
      const native = { user: { id: "123", name: "User", email: "user@example.test", role: "operator", banned: false, emailVerified: true, twoFactorEnabled: true, mustChangePassword: false }, session: { id: "session" } };
      let mapped = true;
      mock.module("./apps/control-plane/src/modules/auth/auth", () => ({ auth: { $context: Promise.resolve({ authCookies: { sessionToken: { name: "fixture_session" } }, secret: "s".repeat(32) }), api: { getSession: async () => native } } }));
      const principal = await import("./apps/control-plane/src/modules/auth/principal");
      mock.module("./apps/control-plane/src/modules/auth/principal", () => ({ ...principal, trustedLocalSessionKind: () => "clerk", selectedIdentityDirectory: { session: async (_db, reference) => { assert.deepEqual(reference, { subject: "123", sessionID: "session" }); return mapped ? effective : null; } } }));
      const provider = await import("./apps/control-plane/src/modules/auth/clerk-provider");
      mock.module("./apps/control-plane/src/modules/auth/clerk-provider", () => ({ ...provider, legacyClerkMemberships: async () => ({ subject: "123", issuer: "https://identity.example", userID: "user_one", operator: effective.role === "operator", twoFactorEnabled: true, verifiedAt: Date.now(), expiresAt: Date.now()+60000, roles: [] }) }));
      mock.module("./apps/control-plane/src/modules/auth/introspection", () => ({ introspectToken: async () => Response.json({ active: true, client_id: "studio", sub: "123", sid: "session", exp: Date.now()/1000+60 }) }));
      const db = await import("./apps/control-plane/src/db/runtime");
      db.authDatabase.query = async sql => ({ rows: sql.includes("SELECT token") ? [{ token: "local-private" }] : [], rowCount: 0 });
      db.database.query = db.authDatabase.query;
      const { handleStudioRequest, handleNativeStudioRequest } = await import("./apps/control-plane/src/modules/portal/studio-api");
      const run = async delegated => {
        const response = await (delegated ? handleStudioRequest : handleNativeStudioRequest)(new Request("http://localhost:3125/api/studio", { method: "POST", headers: { origin: "http://localhost:3125", "content-type": "application/json", authorization: "Basic " + Buffer.from("studio:secret").toString("base64") }, body: JSON.stringify({ operation: "session", args: [], ...(delegated ? { accessToken: "verified-token" } : {}) }) }));
        return { status: response.status, body: await response.json() };
      };
      for (const delegated of [false, true]) {
        for (const role of ["user", "operator", "user"]) {
          effective.role = role;
          native.user.role = role === "operator" ? "user" : "operator";
          if (${JSON.stringify(selected)} === "clerk") { native.user.emailVerified = false; native.user.twoFactorEnabled = false; }
          const result = await run(delegated);
          assert.equal(result.status, 200, JSON.stringify({ delegated, role, body: result.body }));
          const expected = ${JSON.stringify(selected)} === "clerk" ? role : native.user.role;
          assert.equal(result.body.data.user.role, expected);
          assert.equal(result.body.data.operator, expected === "operator");
          assert.equal(result.body.data.user.emailVerified, true);
          assert.equal(result.body.data.user.twoFactorEnabled, true);
        }
        if (${JSON.stringify(selected)} === "clerk") {
          mapped = false;
          assert.equal((await run(delegated)).status, 401);
          mapped = true;
          effective.subject = "wrong";
          assert.equal((await run(delegated)).status, 401);
          effective.subject = "123";
          effective.role = "operator"; effective.twoFactorEnabled = false;
          assert.equal((await run(delegated)).status, 401);
          effective.twoFactorEnabled = true;
        }
        if (delegated) {
          native.session.id = "wrong-session";
          assert.equal((await run(delegated)).status, 401);
          native.session.id = "session";
        }
      }
    `,
      ],
      {
        cwd: new URL("../../../", import.meta.url).pathname,
        env: {
          ...process.env,
          WEBDOCK_AUTH_PROVIDER: JSON.stringify(
            selected === "clerk"
              ? {
                  provider: "clerk",
                  issuer: "https://identity.example",
                  authorizedParties: ["https://console.example"],
                  audience: "webdock",
                }
              : { provider: "better-auth" },
          ),
          WEBDOCK_OAUTH_PROVIDER: "better-auth",
          WEBDOCK_STUDIO_CLIENT_ID: "studio",
          BETTER_AUTH_URL: "http://localhost:3125",
          WEBDOCK_MODULES: '{"configVersion":1,"modules":{}}',
          WEBDOCK_API_DOMAINS: "auth,portal",
          DATABASE_URL:
            "postgres://fixture:fixture@localhost:5432/webdock_test",
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    );
    expect(result.exitCode, result.stderr.toString()).toBe(0);
  });
}
