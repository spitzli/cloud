import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { betterAuth } from "better-auth";
import { hashPassword } from "better-auth/crypto";
import { decodeJwt } from "jose";
import { memoryAdapter } from "better-auth/adapters/memory";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { mcpResource, currentMCPClaims } from "../../src/modules/auth/mcp";
import { offlineProvisioning } from "../../src/modules/auth/offline";
import { POST as introspectMCP } from "../../src/protocols/legacy-auth/api/mcp/introspect/route";
afterEach(restoreMocks);

const origin = "http://localhost:3125";
const databaseURL = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(databaseURL.hostname) ||
  databaseURL.pathname !== "/webdock_api_test"
)
  throw Error(
    "Use the disposable local database (only provider initialization uses it; fixtures remain in memory)",
  );
test.after(async () => {
  await auth.$context;
  await database.end();
});

test("MCP OAuth: PKCE, consent, resource audience and live authorization/revocation", async (t) => {
  await auth.$context;
  const data: Record<string, Record<string, unknown>[]> = {
    user: [],
    session: [],
    account: [],
    verification: [],
  };
  for (const plugin of auth.options.plugins || [])
    for (const model of Object.keys(plugin.schema || {})) data[model] ||= [];
  const fixture = betterAuth({
    ...auth.options,
    baseURL: origin,
    trustedOrigins: [origin],
    secret: randomBytes(32).toString("hex"),
    database: memoryAdapter(data),
    advanced: {
      database: { generateId: () => randomBytes(16).toString("hex") },
    },
    rateLimit: { enabled: false },
    logger: { disabled: true },
  });
  mockMethod(database, "query", async (sql: string, values: unknown[]) => {
    // This fixture models an unmigrated legacy identity store.
    if (sql.includes("to_regclass('webdock_auth.clerk_identity')"))
      return { rows: [{ present: false }] };
    if (sql.includes('FROM webdock_auth."user" WHERE id=$1'))
      return { rows: data.user.filter((user) => user.id === values[0]) };
    if (sql.includes('SELECT metadata FROM webdock_auth."oauthClient"'))
      return {
        rows: data.oauthClient.filter(
          (client) => client.clientId === values[0] && !client.disabled,
        ),
      };
    throw new Error(`Unexpected SQL in isolated test: ${sql}`);
  });
  for (const path of [
    "/api/auth/.well-known/openid-configuration",
    "/api/auth/.well-known/oauth-authorization-server",
    "/.well-known/oauth-authorization-server/api/auth",
  ]) {
    const response = await fixture.handler(new Request(origin + path));
    assert.equal(response.status, 200);
    const metadata = await response.json();
    const confidentialMethods = [
      "client_secret_basic",
      "client_secret_post",
      "private_key_jwt",
    ];
    assert.deepEqual(metadata.token_endpoint_auth_methods_supported, [
      "none",
      ...confidentialMethods,
    ]);
    assert.deepEqual(
      metadata.introspection_endpoint_auth_methods_supported,
      confidentialMethods,
    );
    assert.deepEqual(
      metadata.revocation_endpoint_auth_methods_supported,
      confidentialMethods,
    );
    assert.deepEqual(metadata.grant_types_supported, [
      "authorization_code",
      "refresh_token",
    ]);
    assert.equal(metadata.registration_endpoint, undefined);
  }
  const registration = await fixture.handler(
    new Request(origin + "/api/auth/oauth2/register", {
      method: "POST",
      headers: { Origin: origin, "Content-Type": "application/json" },
      body: JSON.stringify({
        redirect_uris: ["https://client.example/callback"],
        token_endpoint_auth_method: "none",
      }),
    }),
  );
  assert.equal(
    registration.status,
    403,
    "Discovery must not enable dynamic client registration",
  );
  mockMethod(auth, "handler", fixture.handler);
  const ctx = await fixture.$context;
  const user = await ctx.internalAdapter.createUser(
    {
      email: "mcp-test@example.invalid",
      name: "MCP operator",
      emailVerified: true,
      role: "operator",
      mustChangePassword: false,
      twoFactorEnabled: false,
    },
    { method: "admin" },
  );
  const password = randomBytes(24).toString("base64url");
  await ctx.internalAdapter.createAccount({
    userId: user.id,
    accountId: user.id,
    providerId: "credential",
    password: await hashPassword(password),
  });
  let cookies = "";
  const call = async (path: string, body?: unknown) => {
    const response = await fixture.handler(
      new Request(origin + "/api/auth" + path, {
        method: body ? "POST" : "GET",
        headers: {
          Origin: origin,
          Cookie: cookies,
          "Content-Type": "application/json",
        },
        body: body ? JSON.stringify(body) : undefined,
      }),
    );
    if (response.headers.getSetCookie().length)
      cookies = response.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; ");
    return response;
  };
  assert.equal(
    (await call("/sign-in/email", { email: user.email, password })).status,
    200,
  );
  // The dedicated auth/passkey suites exercise MFA ceremony. This fixture starts
  // from a completed setup state and checks its revocation on every MCP request.
  data.user[0].twoFactorEnabled = true;
  const headers = new Headers({ Cookie: cookies });
  let nextId = 1000;
  const createClient = async (
    metadata: Record<string, unknown>,
    scope = "webdock:read webdock:write",
    confidential = true,
  ) =>
    offlineProvisioning.run({ clientID: String(++nextId) }, () =>
      fixture.api.adminCreateOAuthClient({
        headers,
        body: {
          client_name: "MCP test",
          redirect_uris: ["https://client.example/callback"],
          application_type: "web",
          scope,
          grant_types: ["authorization_code", "refresh_token"],
          response_types: ["code"],
          token_endpoint_auth_method: confidential
            ? "client_secret_post"
            : "none",
          require_pkce: true,
          skip_consent: false,
          metadata,
        },
      }),
    );
  const client = await createClient(
    { webdock_mcp: true },
    "webdock:read webdock:write",
    false,
  );
  assert.equal(client.client_secret, undefined);
  const resourceServer = await createClient(
    { webdock_binding: "test-studio" },
    "openid profile email",
  );
  for (const clientId of [client.client_id, resourceServer.client_id])
    await fixture.api.adminLinkClientResource({
      headers,
      params: { identifier: mcpResource, client_id: clientId },
    });
  const verifier = randomBytes(32).toString("base64url");
  const query = () =>
    new URLSearchParams({
      client_id: client.client_id,
      redirect_uri: "https://client.example/callback",
      response_type: "code",
      scope: "webdock:read",
      resource: mcpResource,
      state: "test-state",
      code_challenge: createHash("sha256").update(verifier).digest("base64url"),
      code_challenge_method: "S256",
    });
  const authorize = async (params = query()) => {
    const response = await call("/oauth2/authorize?" + params);
    assert.equal(response.status, 302);
    return new URL(response.headers.get("location")!, origin);
  };
  const badRedirect = query();
  badRedirect.set("redirect_uri", "https://client.example/callback/extra");
  assert.notEqual(
    (await authorize(badRedirect)).origin,
    "https://client.example",
  );
  const noPKCE = query();
  noPKCE.delete("code_challenge");
  noPKCE.delete("code_challenge_method");
  assert.ok((await authorize(noPKCE)).searchParams.has("error"));
  const wrongResource = query();
  wrongResource.set("resource", "https://wrong.example/mcp");
  assert.ok((await authorize(wrongResource)).searchParams.has("error"));
  const unlinkedClient = await createClient(
    { webdock_mcp: true },
    "webdock:read",
  );
  const unlinked = query();
  unlinked.set("client_id", unlinkedClient.client_id);
  assert.ok((await authorize(unlinked)).searchParams.has("error"));
  await fixture.api.adminLinkClientResource({
    headers,
    params: { identifier: mcpResource, client_id: unlinkedClient.client_id },
  });
  unlinked.set("scope", "webdock:write");
  assert.ok((await authorize(unlinked)).searchParams.has("error"));
  const consentURL = await authorize();
  assert.equal(consentURL.pathname, "/consent");
  const tamperedConsent = new URLSearchParams(consentURL.search);
  tamperedConsent.set("scope", "webdock:read webdock:write");
  assert.equal(
    (
      await call("/oauth2/consent", {
        accept: true,
        oauth_query: tamperedConsent.toString(),
      })
    ).status,
    400,
  );
  const consent = await call("/oauth2/consent", {
    accept: true,
    oauth_query: consentURL.search.slice(1),
  });
  assert.equal(consent.status, 200);
  const destination = new URL((await consent.json()).url);
  assert.equal(destination.origin, "https://client.example");
  assert.equal(destination.searchParams.get("state"), "test-state");
  const tokenRequest = async (code: string, codeVerifier = verifier) =>
    fixture.handler(
      new Request(origin + "/api/auth/oauth2/token", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "authorization_code",
          client_id: client.client_id,
          ...(client.client_secret
            ? { client_secret: client.client_secret }
            : {}),
          redirect_uri: "https://client.example/callback",
          code,
          code_verifier: codeVerifier,
          resource: mcpResource,
        }),
      }),
    );
  const tokenResponse = await tokenRequest(
    destination.searchParams.get("code")!,
  );
  assert.equal(tokenResponse.status, 200);
  const tokens = await tokenResponse.json();
  assert.equal(tokens.expires_in, 300);
  assert.equal(tokens.scope, "webdock:read");
  assert.equal(tokens.refresh_token, undefined);
  assert.equal(tokens.id_token, undefined);
  assert.equal(tokens.access_token.split(".").length, 3);
  assert.equal(
    (await tokenRequest(destination.searchParams.get("code")!)).status,
    400,
  );
  const nextCode = (await authorize()).searchParams.get("code");
  assert.ok(nextCode, "Prior consent permits a later authorization");
  assert.equal(
    (await tokenRequest(nextCode, randomBytes(32).toString("base64url")))
      .status,
    401,
    "Wrong verifier is rejected",
  );
  const introspect = async (
    token = tokens.access_token,
    clientId = resourceServer.client_id,
    secret = resourceServer.client_secret!,
  ) =>
    introspectMCP(
      new Request(origin + "/api/mcp/introspect", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          token,
          client_id: clientId,
          client_secret: secret,
        }),
      }),
    );
  let response = await introspect();
  assert.equal(response.status, 200);
  let claims = await response.json();
  assert.equal(claims.active, true);
  assert.equal(claims.aud, mcpResource);
  assert.equal(claims.webdock_role, "operator");
  assert.equal(claims.scope, "webdock:read");
  assert.equal(claims.token_type, "Bearer");
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.ok(
    [400, 401].includes(
      (
        await introspect(
          tokens.access_token,
          resourceServer.client_id,
          "incorrect",
        )
      ).status,
    ),
  );
  const wrongAudience = await fixture.api.signJWT({
    body: {
      payload: {
        ...decodeJwt(tokens.access_token),
        aud: "https://wrong.example/mcp",
      },
    },
  });
  assert.equal(
    (await (await introspect(wrongAudience.token)).json()).active,
    false,
  );
  const sessionlessClaims = { ...decodeJwt(tokens.access_token) };
  delete sessionlessClaims.sid;
  const sessionless = await fixture.api.signJWT({
    body: { payload: sessionlessClaims },
  });
  assert.equal(
    (await (await introspect(sessionless.token)).json()).active,
    false,
    "MCP never accepts a token without a session binding",
  );
  const expired = await fixture.api.signJWT({
    body: {
      payload: {
        ...decodeJwt(tokens.access_token),
        exp: Math.floor(Date.now() / 1000) - 1,
      },
    },
  });
  assert.equal((await (await introspect(expired.token)).json()).active, false);
  response = await introspect("invalid-token");
  assert.ok(response.status >= 400 || !(await response.json()).active);
  for (const [field, value] of [
    ["role", "user"],
    ["twoFactorEnabled", false],
    ["mustChangePassword", true],
    ["banned", true],
    ["emailVerified", false],
  ] as const) {
    const previous = data.user[0][field];
    data.user[0][field] = value;
    assert.equal((await currentMCPClaims(user.id)).disabled, true);
    assert.equal((await (await introspect()).json()).active, false, field);
    data.user[0][field] = previous;
  }
  const unlinkedIntrospector = await createClient({
    webdock_binding: "unlinked",
  });
  assert.equal(
    (
      await (
        await introspect(
          tokens.access_token,
          unlinkedIntrospector.client_id,
          unlinkedIntrospector.client_secret!,
        )
      ).json()
    ).active,
    false,
  );
  const issuingClient = data.oauthClient.find(
    (row) => row.clientId === client.client_id,
  )!;
  const metadata = issuingClient.metadata;
  issuingClient.metadata = { webdock_mcp: false };
  assert.equal((await (await introspect()).json()).active, false);
  issuingClient.metadata = metadata;
  issuingClient.disabled = true;
  assert.equal((await (await introspect()).json()).active, false);
  issuingClient.disabled = false;
  const remote = await createClient(
    { webdock_mcp: true },
    "webdock:read offline_access",
    false,
  );
  await fixture.api.adminLinkClientResource({
    headers,
    params: { identifier: mcpResource, client_id: remote.client_id },
  });
  const remoteQuery = query();
  remoteQuery.set("client_id", remote.client_id);
  remoteQuery.set("scope", "webdock:read offline_access");
  const remoteConsent = await authorize(remoteQuery);
  const approved = await call("/oauth2/consent", {
    accept: true,
    oauth_query: remoteConsent.search.slice(1),
  });
  const remoteCode = new URL((await approved.json()).url).searchParams.get(
    "code",
  )!;
  const exchange = async (body: Record<string, string>) =>
    fixture.handler(
      new Request(origin + "/api/auth/oauth2/token", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          client_id: remote.client_id,
          resource: mcpResource,
          ...body,
        }),
      }),
    );
  const firstResponse = await exchange({
    grant_type: "authorization_code",
    code: remoteCode,
    code_verifier: verifier,
    redirect_uri: "https://client.example/callback",
  });
  assert.equal(firstResponse.status, 200);
  const first = await firstResponse.json();
  assert.ok(first.refresh_token);
  assert.equal(
    (
      await exchange({
        grant_type: "refresh_token",
        refresh_token: first.refresh_token,
        scope: "webdock:read webdock:write",
      })
    ).status,
    400,
  );
  const renewedResponse = await exchange({
    grant_type: "refresh_token",
    refresh_token: first.refresh_token,
  });
  assert.equal(renewedResponse.status, 200);
  const renewed = await renewedResponse.json();
  assert.ok(
    renewed.refresh_token && renewed.refresh_token !== first.refresh_token,
  );
  assert.equal(renewed.expires_in, 300);
  assert.equal(
    (await (await introspect(renewed.access_token)).json()).active,
    true,
  );
  data.user[0].banned = true;
  assert.equal(
    (
      await exchange({
        grant_type: "refresh_token",
        refresh_token: renewed.refresh_token,
      })
    ).status,
    403,
  );
  data.user[0].banned = false;
  const remoteClient = data.oauthClient.find(
    (row) => row.clientId === remote.client_id,
  )!;
  remoteClient.disabled = true;
  assert.equal(
    (await (await introspect(renewed.access_token)).json()).active,
    false,
  );
  assert.ok(
    (
      await exchange({
        grant_type: "refresh_token",
        refresh_token: renewed.refresh_token,
      })
    ).status >= 400,
  );
  remoteClient.disabled = false;
  await call("/sign-out", {});
  assert.equal(
    (await (await introspect(renewed.access_token)).json()).active,
    false,
  );
  // PostgreSQL ON DELETE SET NULL detaches offline refresh tokens on logout.
  // The memory adapter does not emulate foreign-key actions.
  for (const row of data.oauthRefreshToken) row.sessionId = null;
  const afterLogout = await exchange({
    grant_type: "refresh_token",
    refresh_token: renewed.refresh_token,
  });
  assert.equal(
    afterLogout.status,
    403,
    "Renewal must reject a refresh token detached from its deleted session",
  );
  const replay = await exchange({
    grant_type: "refresh_token",
    refresh_token: first.refresh_token,
  });
  assert.equal(replay.status, 400, "Rotated refresh tokens cannot be replayed");
  claims = await (await introspect()).json();
  assert.equal(
    claims.active,
    false,
    "Deleting the central session revokes the JWT",
  );
});
