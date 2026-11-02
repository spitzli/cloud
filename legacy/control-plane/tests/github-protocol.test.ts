import { expect, test } from "bun:test";

test("Core and full mount one GitHub protocol, enforce live identity and preserve callback refresh without importing Better Auth", () => {
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "-e",
      `
    import assert from "node:assert/strict";
    import { mock } from "bun:test";
    mock.module("./apps/control-plane/src/modules/auth/auth", () => { throw Error("unselected Better Auth loaded"); });
    const { ConsoleAuthError } = await import("./apps/control-plane/src/modules/auth/clerk-identity");
    const provider = await import("./apps/control-plane/src/modules/auth/clerk-provider");
    let authorized = true, identityReads = 0, actorReads = 0;
    const principal = async () => {
      identityReads++;
      if (!authorized) throw new ConsoleAuthError(401, "Revoked");
      return { provider:"clerk", issuer:"https://identity.example", userID:"1001", clerkUserID:"external", sessionID:"session", operator:true, organizationID:null, clerkOrganizationID:null, customerID:null, role:null, verifiedAt:Date.now(), expiresAt:Date.now()+60000, name:"Test", email:"test@example.test" };
    };
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider", () => ({...provider,
      requireConsoleBrowserPrincipal: async (request) => {
        if (request.headers.has("authorization")) throw new ConsoleAuthError(401, "Browser cookies required");
        return principal();
      },
      requireConsolePrincipal: async (request) => {
        assert.equal(request.headers.get("x-webdock-service-key"), "test-service-key-32-characters-long");
        assert.ok(request.headers.get("authorization")?.startsWith("Bearer "));
        return principal();
      },
    }));
    const { currentDatabaseScope } = await import("@webdock/database-runtime");
    mock.module("./apps/control-plane/src/modules/integrations/operator", () => ({ currentRegistryActor: async () => {
      actorReads++;
      assert.equal(currentDatabaseScope()?.subject, "1001");
      return { user:{id:"1001"} };
    }}));
    const { readConfig } = await import("./apps/control-plane/src/config");
    const { createApp } = await import("./apps/control-plane/src/app");
    const { githubProtocols } = await import("./apps/control-plane/src/protocols/github");
    const config = readConfig(process.env);
    const mounts = githubProtocols(config);
    assert.equal(mounts.flatMap(m => m.app.routes).filter(r => r.path === "/api/github/callback").length, 1);
    const app = await createApp(config, { mounts });
    const connect = (suffix="", origin=config.studioOrigin) => new Request(config.studioOrigin + "/api/github/connect" + suffix, { method:"POST", headers:{origin} });
    assert.equal((await app.handle(connect("?returnTo=//evil.test"))).status, 400);
    assert.equal((await app.handle(connect("?returnTo=/plugins/123/install&returnTo=/plugins"))).status, 400);
    assert.equal(actorReads, 0);
    const previousReads = identityReads;
    assert.equal((await app.handle(connect("", "https://evil.test"))).status, 403);
    assert.equal(identityReads, previousReads);
    const started = await app.handle(connect("?returnTo=/plugins/123/install"));
    assert.equal(started.status, 303);
    assert.equal(new URL(started.headers.get("location")).origin, "https://github.com");
    authorized = false;
    assert.equal((await app.handle(connect())).status, 401);
    const callback = await app.handle(new Request(config.studioOrigin + "/api/github/callback?code=private-code&state=state"));
    assert.equal(callback.status, 303);
    assert.equal(callback.headers.get("location"), config.studioOrigin + "/auth/integration-resume");
    assert.ok(!callback.headers.get("set-cookie").includes("private-code"));
    const resume = () => app.handle(new Request(config.apiOrigin + "/v1/integrations/resume", { method:"POST", headers:{ origin:config.studioOrigin, authorization:"Bearer signed.proof", "x-webdock-service-key":"test-service-key-32-characters-long", cookie:callback.headers.get("set-cookie").split(";")[0] } }));
    const resumed = await resume();
    assert.equal(resumed.status, 401);
    assert.equal(resumed.headers.get("location"), null);
    assert.ok(resumed.headers.get("set-cookie").includes("Max-Age=0"));
    authorized = true;
    const beforeFresh = actorReads;
    assert.equal((await resume()).status, 303);
    assert.equal(actorReads, beforeFresh + 1);
    const { configureFullInstallation } = await import("./apps/control-plane/src/full-installation");
    configureFullInstallation();
    const { integrationProtocols } = await import("./apps/control-plane/src/protocols/integrations");
    const full = await createApp(config, { mounts:[...mounts, ...integrationProtocols(config)] });
    const beforeSelect = identityReads;
    assert.equal((await full.handle(new Request(config.studioOrigin + "/api/github/select", {method:"POST", headers:{origin:config.studioOrigin}}))).status, 503);
    assert.equal(identityReads, beforeSelect);
    delete process.env.WEBDOCK_SSO_COOKIE_SECRET;
    assert.equal(githubProtocols(config).flatMap(m => m.app.routes).some(r => r.path === "/v1/integrations/resume"), false);
  `,
    ],
    {
      cwd: new URL("../../../", import.meta.url).pathname,
      env: {
        ...process.env,
        WEBDOCK_AUTH_PROVIDER: JSON.stringify({
          provider: "clerk",
          issuer: "https://identity.example",
          authorizedParties: ["https://studio.example.test"],
          audience: "console",
        }),
        WEBDOCK_CONSOLE_SERVICE_KEY: "test-service-key-32-characters-long",
        WEBDOCK_MODULES: JSON.stringify({ configVersion: 1, modules: {} }),
        WEBDOCK_API_DOMAINS: "auth,portal,registry",
        WEBDOCK_STUDIO_ORIGIN: "https://studio.example.test",
        WEBDOCK_SSO_APP_ORIGIN: "https://studio.example.test",
        WEBDOCK_GITHUB_CLIENT_ID: "Iv1.test-client",
        WEBDOCK_GITHUB_CLIENT_SECRET: "test-client-secret",
        WEBDOCK_GITHUB_APP_SLUG: "test-app",
        WEBDOCK_SSO_COOKIE_SECRET: "test-sso-cookie-secret-32-characters-long",
        DATABASE_URL: "postgres://fixture:fixture@localhost:5432/webdock_test",
        BETTER_AUTH_SECRET: "",
        CLERK_ISSUER: "",
      },
    },
  );
  expect(result.stderr.toString()).toBe("");
  expect(result.exitCode).toBe(0);
});
