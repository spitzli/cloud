import { expect, test } from "bun:test";
import { readAuthProtocolSelection } from "@webdock/auth-configuration/protocols";
import { createAuthProtocolMounts } from "../src/protocols/auth-startup";
import { readConfig } from "../src/config";
const clerk = JSON.stringify({provider:"clerk",issuer:"https://identity.example",authorizedParties:["https://console.example"],audience:"console"});

test("Clerk starts auth foundation without importing Better Auth or requiring its secret", async () => {
  const config = readConfig({WEBDOCK_API_DOMAINS:"auth,portal,registry"});
  let loaded = false;
  const mounts = await createAuthProtocolMounts(config, readAuthProtocolSelection({WEBDOCK_AUTH_PROVIDER:clerk}), async () => { loaded = true; throw Error("must not load"); });
  expect(mounts).toEqual([]);
  expect(loaded).toBe(false);
});
test("OAuth compatibility is explicit for Clerk and legacy installs retain their issuer", () => {
  expect(readAuthProtocolSelection({}).oauthProvider).toBe("better-auth");
  expect(readAuthProtocolSelection({WEBDOCK_AUTH_PROVIDER:clerk}).oauthProvider).toBe("disabled");
  expect(readAuthProtocolSelection({WEBDOCK_AUTH_PROVIDER:clerk,WEBDOCK_OAUTH_PROVIDER:"better-auth"}).compatibility).toBe(true);
  expect(() => readAuthProtocolSelection({WEBDOCK_OAUTH_PROVIDER:"clerk-jwt"})).toThrow("Unsupported OAuth provider");
});
test("requesting MCP without an OAuth capability fails before loading a runtime", async () => {
  const config=readConfig({WEBDOCK_API_DOMAINS:"auth,mcp"});
  await expect(createAuthProtocolMounts(config,readAuthProtocolSelection({WEBDOCK_AUTH_PROVIDER:clerk}))).rejects.toThrow("MCP requires an explicitly selected OAuth provider");
});
test("disabled OAuth denies introspection before importing the legacy runtime", async () => {
  const previous = process.env.WEBDOCK_AUTH_PROVIDER;
  const previousOAuth = process.env.WEBDOCK_OAUTH_PROVIDER;
  process.env.WEBDOCK_AUTH_PROVIDER = clerk;
  delete process.env.WEBDOCK_OAUTH_PROVIDER;
  try {
    const { introspectToken } = await import("../src/modules/auth/introspection");
    await expect(introspectToken("untrusted-token", "client", "secret")).rejects.toThrow("Legacy OAuth capability is disabled");
  } finally {
    if (previous === undefined) delete process.env.WEBDOCK_AUTH_PROVIDER;
    else process.env.WEBDOCK_AUTH_PROVIDER = previous;
    if (previousOAuth === undefined) delete process.env.WEBDOCK_OAUTH_PROVIDER;
    else process.env.WEBDOCK_OAUTH_PROVIDER = previousOAuth;
  }
});
test("Clerk portal, registry and plugin modules never import the unselected runtime", () => {
  const root = new URL("../../../", import.meta.url).pathname;
  const result = Bun.spawnSync([process.execPath, "-e", `
    import { mock } from "bun:test";
    mock.module("./apps/control-plane/src/modules/auth/auth", () => { throw Error("unselected Better Auth loaded"); });
    await import("./apps/control-plane/src/protocols/portal");
    await import("./apps/control-plane/src/protocols/registry");
    await import("./apps/control-plane/src/protocols/plugins");
  `], {
    cwd: root,
    env: { ...process.env, WEBDOCK_AUTH_PROVIDER: clerk, DATABASE_URL: "postgres://fixture:fixture@localhost:5432/webdock_test" },
  });
  expect(result.stderr.toString()).toBe("");
  expect(result.exitCode).toBe(0);
});
test("selected Clerk Console transport revalidates mocked identity and authorization without Better Auth", () => {
  const script = `
    import { mock } from "bun:test";
    mock.module("./apps/control-plane/src/modules/auth/auth", () => { throw Error("unselected runtime"); });
    const provider = await import("./apps/control-plane/src/modules/auth/clerk-provider");
    const { createConsoleVerifier } = await import("./apps/control-plane/src/modules/auth/clerk-identity");
    const { consoleAuthConfiguration } = await import("./apps/control-plane/src/modules/auth/console-configuration");
    const config = consoleAuthConfiguration();
    globalThis.fetch = async () => { throw Error("External network forbidden in transport matrix"); };
    // Provider and SQL are local doubles; the HTTP handler and authorization paths are real.
    const claims = { sub:"external", sid:"session", iss:config.issuer, azp:config.authorizedParties[0], aud:config.audience, exp:Math.floor(Date.now()/1000)+60 };
    const liveSession = { id:"session", userId:"external", status:"active", expireAt:Date.now()+60000 };
    const local = { id:"1001",name:"Fixture",email:"fixture@example.test",role:"operator",banned:false,enabled:true,emailVerified:true };
    const providerMetadata = { webdockRole: "operator" };
    let mapped = true;
    let providerReads = 0;
    const verifier = createConsoleVerifier(config, {
      verify: async () => { providerReads++; return claims; },
      session: async () => liveSession,
      user: async () => ({ id:"external", banned:false, locked:false, privateMetadata:providerMetadata, primaryEmailAddressId:"email", emailAddresses:[{id:"email",verification:{status:"verified"}}] }),
      identity: async () => mapped ? local : null,
      memberships: async () => [], organization: async () => null,
    });
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider", () => ({ ...provider, requireConsolePrincipal:verifier }));
    const { database } = await import("./apps/control-plane/src/db/runtime");
    let projectReads = 0;
    database.query = async (sql, values) => {
      if (sql.includes("FROM webdock_auth.plugin_installation")) throw Error("Foreign project reached plugin controller");
      if (sql.includes("FROM webdock_admin.projects p")) {
        if (values?.[0] !== "9999") throw Error("wrong project lookup");
        projectReads++;
      }
      return { rows:[] };
    };
    const { handleConsoleRequest } = await import("./apps/control-plane/src/modules/portal/studio-api");
    const request = (key = true, operation = "session", args = []) => new Request("http://localhost:3130/v1/console", { method:"POST", headers:{ "content-type":"application/json",authorization:"Bearer verified",cookie:"webdock-auth.session_token=legacy-cookie",...(key?{"x-webdock-service-key":config.serviceKey}:{}) },body:JSON.stringify({operation,args}) });
    const check = async (status, operation = "session", args = [], key = true) => {
      const response = await handleConsoleRequest(request(key, operation, args));
      const body = await response.json();
      if (response.status !== status) throw Error(JSON.stringify({operation,status:response.status,expected:status,body}));
      return body;
    };
    await check(401, "session", [], false);
    if (providerReads !== 0) throw Error("provider read before service credential");
    const body = await check(200);
    if (body.data?.user?.id !== "1001") throw Error(JSON.stringify(body));
    await check(200, "requireAccessOperator");
    providerMetadata.webdockRole = "user";
    await check(403, "requireAccessOperator");
    if ((await check(200)).data.user.role !== "user") throw Error("stale operator role retained");
    // SQL double models a project hidden by RLS; this is the application denial, not a database RLS test.
    const deniedProject = await check(403, "plugins.list", ["9999"]);
    if (projectReads !== 1 || deniedProject.error?.kind !== "PluginError") throw Error("project access guard was not reached");
    providerMetadata.webdockRole = "operator";
    await check(200, "requireAccessOperator");
    liveSession.status = "revoked";
    await check(401);
    liveSession.status = "active";
    for (const field of ["iss", "aud", "azp"]) {
      const previous = claims[field];
      claims[field] = "https://foreign.example";
      await check(401);
      claims[field] = previous;
    }
    mapped = false;
    await check(403);
    mapped = true;
    local.enabled = false;
    await check(403);
    local.enabled = true;
    await check(200);

  `;
  const result = Bun.spawnSync([process.execPath, "-e", script], {
    cwd: new URL("../../../", import.meta.url).pathname,
    env: {...process.env, WEBDOCK_MODULES: JSON.stringify({configVersion:1,modules:{}}), DATABASE_URL:"postgres://fixture:fixture@localhost:5432/webdock_test", WEBDOCK_AUTH_PROVIDER:clerk, WEBDOCK_CONSOLE_SERVICE_KEY:"s".repeat(32), WEBDOCK_API_DOMAINS:"auth,portal,registry", BETTER_AUTH_SECRET:"", CLERK_ISSUER:"" },
  });
  expect(result.stderr.toString()).toBe("");
  expect(result.exitCode).toBe(0);
});
