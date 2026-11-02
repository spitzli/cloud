/** Disposable operator.integration.test fixture only; exercises real Auth HTTP handlers. */
import assert from "node:assert/strict";
import { createHash, createHmac, randomBytes } from "node:crypto";
const target = new URL(process.env.DATABASE_URL || "");
if (
  process.env.WEBDOCK_TEST_OPERATOR !== "true" ||
  target.hostname !== "127.0.0.1" ||
  target.pathname !== "/webdock_operator_test"
)
  throw Error("Only the isolated operator fixture is allowed");
const { auth } = await import("../../legacy/control-plane/src/modules/auth/auth");
const { databasePools } = await import("../../legacy/control-plane/src/db/runtime");
const { currentClaims } =
  await import("../../legacy/control-plane/src/modules/auth/authorization");
const origin = process.env.BETTER_AUTH_URL!,
  clientID = process.env.WEBDOCK_STUDIO_CLIENT_ID!,
  clientSecret = process.env.WEBDOCK_SSO_CLIENT_SECRET!,
  binding = process.env.OPERATOR_TEST_BINDING!,
  subject = process.env.OPERATOR_TEST_SUBJECT!;
const originalFetch = globalThis.fetch;
let externalHTTPAttempts = 0;
globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
  const request = new Request(input, init);
  if (new URL(request.url).origin !== origin) {
    externalHTTPAttempts++;
    throw Error("Operator fixture forbids external HTTP");
  }
  return auth.handler(request);
}) as typeof globalThis.fetch;
const { readConfig } = await import("../../legacy/control-plane/src/config");
const { createApp } = await import("../../legacy/control-plane/src/app");
const { authProtocol } = await import("../../legacy/control-plane/src/protocols/auth");
const { portalProtocols } = await import("../../legacy/control-plane/src/protocols/portal");
const { registryProtocol } =
  await import("../../legacy/control-plane/src/protocols/registry");
const { registryService, closeRegistry } =
  await import("../../legacy/control-plane/src/modules/registry/context");
const { verifyDatabaseReadiness } =
  await import("../../legacy/control-plane/src/db/readiness");
const { verifyModuleShutdown } =
  await import("../../legacy/control-plane/src/module-preflight");
const { handleConsoleOrigin } =
  await import("../../legacy/control-plane/src/console-origin");
const config = readConfig(process.env);
assert.deepEqual(config.modules!.modules, {});
await verifyModuleShutdown();
const registry = await registryService();
let readyChecks = 0;
const ready = async () => {
  await verifyDatabaseReadiness(databasePools, config.domains, registry.raw);
  readyChecks++;
};
await ready();
const app = await createApp(config, {
  ready,
  mounts: [
    authProtocol(config, (request) => auth.handler(request)),
    ...portalProtocols(config),
    await registryProtocol(config),
  ],
});
const handle = (request: Request) =>
  handleConsoleOrigin(request, config, (request) => app.handle(request));
let cookies = "";
async function call(path: string, body?: unknown) {
  const response = await handle(
    new Request(origin + "/api/auth" + path, {
      method: body ? "POST" : "GET",
      headers: {
        Origin: origin,
        "content-type": "application/json",
        "x-webdock-client-ip": "127.0.0.1",
        Cookie: cookies,
      },
      body: body ? JSON.stringify(body) : undefined,
    }),
  );
  const jar = new Map(
    cookies
      .split("; ")
      .filter(Boolean)
      .map((c) => [c.split("=")[0], c]),
  );
  for (const c of response.headers.getSetCookie()) {
    const value = c.split(";")[0];
    jar.set(value.split("=")[0], value);
  }
  cookies = [...jar.values()].join("; ");
  return response;
}
function totp(uri: string) {
  const secret = new URL(uri).searchParams.get("secret")!;
  let bits = "";
  for (const c of secret.toUpperCase().replace(/=+$/, ""))
    bits += "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
      .indexOf(c)
      .toString(2)
      .padStart(5, "0");
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2))),
    counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const h = createHmac("sha1", key).update(counter).digest(),
    at = h[h.length - 1] & 15;
  return String((h.readUInt32BE(at) & 0x7fffffff) % 1000000).padStart(6, "0");
}
try {
  const password = process.env.OPERATOR_TEST_PASSWORD!;
  assert.equal(
    (await call("/sign-in/email", { email: "operator@example.test", password }))
      .status,
    200,
  );
  assert.equal((await currentClaims(subject, binding)).disabled, true);
  const changed = randomBytes(32).toString("base64url");
  assert.equal(
    (
      await call("/change-password", {
        currentPassword: password,
        newPassword: changed,
        revokeOtherSessions: true,
      })
    ).status,
    200,
  );
  // A password change does not stand in for MFA enrollment.
  assert.equal((await currentClaims(subject, binding)).disabled, true);
  const enroll = await call("/two-factor/enable", { password: changed });
  assert.equal(enroll.status, 200);
  const setup = await enroll.json();
  assert.ok(setup.totpURI);
  assert.equal(
    (await call("/two-factor/verify-totp", { code: totp(setup.totpURI) }))
      .status,
    200,
  );
  assert.equal(
    (await currentClaims(subject, binding)).webdock_role,
    "operator",
  );
  const verifier = randomBytes(32).toString("base64url"),
    redirect = process.env.WEBDOCK_STUDIO_ORIGIN! + "/api/sso/callback";
  const query = new URLSearchParams({
    client_id: clientID,
    redirect_uri: redirect,
    response_type: "code",
    scope: "openid profile email",
    code_challenge: createHash("sha256").update(verifier).digest("base64url"),
    code_challenge_method: "S256",
    state: "operator-fixture",
    nonce: randomBytes(18).toString("hex"),
  });
  const authorize = await call("/oauth2/authorize?" + query);
  assert.equal(authorize.status, 302);
  const destination = new URL(authorize.headers.get("location")!);
  assert.equal(destination.origin, process.env.WEBDOCK_STUDIO_ORIGIN);
  const code = destination.searchParams.get("code");
  assert.ok(code);
  const tokens = await auth.handler(
    new Request(origin + "/api/auth/oauth2/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "authorization_code",
        client_id: clientID,
        client_secret: clientSecret,
        redirect_uri: redirect,
        code,
        code_verifier: verifier,
      }),
    }),
  );
  assert.equal(tokens.status, 200);
  const token = await tokens.json();
  assert.ok(token.id_token);
  const introspect = await auth.handler(
    new Request(origin + "/api/auth/oauth2/introspect", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        token: token.access_token,
        client_id: clientID,
        client_secret: clientSecret,
      }),
    }),
  );
  assert.equal(introspect.status, 200);
  const claims = await introspect.json();
  assert.equal(claims.active, true);
  assert.equal(claims.sub, subject);
  assert.equal(claims.webdock_role, "operator");
  // Exercise the real portal flow, including sealed flow/session cookies and the
  // distinct Console-to-Studio compatibility mapping. No cookie/session is fabricated.
  const readyResponse = await handle(new Request(config.apiOrigin + "/readyz"));
  assert.equal(readyResponse.status, 200);
  assert.deepEqual(await readyResponse.json(), { status: "ready" });
  assert.equal(readyChecks, 2);
  const login = await handle(
    new Request(config.consoleOrigin! + "/api/sso/login"),
  );
  assert.equal(login.status, 302);
  const flowCookies = login.headers
    .getSetCookie()
    .map((value) => value.split(";")[0])
    .join("; ");
  assert.ok(flowCookies);
  const authorizationURL = new URL(login.headers.get("location")!);
  assert.equal(authorizationURL.origin, origin);
  const portalAuthorization = await handle(
    new Request(authorizationURL, { headers: { Cookie: cookies } }),
  );
  assert.equal(portalAuthorization.status, 302);
  const compatibilityCallback = new URL(
    portalAuthorization.headers.get("location")!,
  );
  assert.equal(compatibilityCallback.origin, config.studioOrigin);
  const relocated = await handle(new Request(compatibilityCallback));
  assert.equal(relocated.status, 307);
  const consoleCallback = new URL(relocated.headers.get("location")!);
  assert.equal(consoleCallback.origin, config.consoleOrigin);
  const callback = await handle(
    new Request(consoleCallback, { headers: { Cookie: flowCookies } }),
  );
  assert.equal(callback.status, 302);
  assert.equal(
    new URL(callback.headers.get("location")!).origin,
    config.consoleOrigin,
  );
  const portalCookies = callback.headers
    .getSetCookie()
    .map((value) => value.split(";")[0])
    .join("; ");
  assert.ok(portalCookies);
  const sessionResponse = await handle(
    new Request(config.apiOrigin + "/v1/studio", {
      method: "POST",
      headers: {
        Origin: config.studioOrigin,
        "content-type": "application/json",
        Cookie: portalCookies,
      },
      body: JSON.stringify({ operation: "session", args: [] }),
    }),
  );
  assert.equal(sessionResponse.status, 200);
  const session = await sessionResponse.json();
  assert.equal(session.data.user.id, subject);
  assert.equal(session.data.operator, true);
  assert.deepEqual(session.data.installation.modules, {});
  for (const collection of ["customers", "projects", "audit-events"]) {
    const result = await handle(
      new Request(config.apiOrigin + "/v1/operator/registry/" + collection, {
        headers: { Cookie: portalCookies },
      }),
    );
    assert.equal(result.status, 200);
    assert.ok(Array.isArray((await result.json()).docs));
  }
  assert.equal((await handle(new Request(config.apiOrigin + "/v1/operator/registry/cms-instances", {headers: {Cookie: portalCookies}}))).status, 400);
  assert.equal(
    (
      await handle(
        new Request(config.apiOrigin + "/v1/operator/registry/customers"),
      )
    ).status,
    401,
  );
  for (const path of [
    "/api/mail/v1/emails",
    "/api/hosting/bridge",
    "/api/databases/bridge",
  ]) {
    const response = await handle(
      new Request(config.apiOrigin + path, {
        headers: { Cookie: portalCookies },
      }),
    );
    assert.equal(response.status, 404);
    assert.equal((await response.json()).error.code, "invalid_request");
  }
  const storage = await handle(
    new Request(config.apiOrigin + "/v1/studio", {
      method: "POST",
      headers: {
        Origin: config.studioOrigin,
        "content-type": "application/json",
        Cookie: portalCookies,
      },
      body: JSON.stringify({ operation: "getTenantStorage", args: ["123"] }),
    }),
  );
  assert.equal(storage.status, 400);
  assert.equal((await storage.json()).error.message, "Invalid Studio request.");
  // Explicit full composition restores product command admission and its existing DB authorization.
  const { configureFullProducts } = await import("../../legacy/control-plane/src/full-products");
  configureFullProducts();
  // Managed DB commands keep their legacy wire name without owning Hosting routes.
  process.env.WEBDOCK_MODULES = JSON.stringify({ configVersion: 1, modules: { databases: { enabled: true } } });
  process.env.WEBDOCK_API_DOMAINS = "auth,databases,registry";
  const databasesConfig = readConfig(process.env);
  const databasesApp = await createApp(databasesConfig, { mounts: portalProtocols(databasesConfig) });
  const plans = await databasesApp.handle(new Request(databasesConfig.apiOrigin + "/v1/studio", {
    method: "POST",
    headers: { Origin: databasesConfig.studioOrigin, "content-type": "application/json", Cookie: portalCookies },
    body: JSON.stringify({ operation: "hosting.command", args: [{ action: "databases.plans.list" }] }),
  }));
  assert.equal(plans.status, 200, await plans.clone().text());
  assert.ok(Array.isArray((await plans.json()).data));
  assert.equal(externalHTTPAttempts, 0);
  console.log(
    "Real MFA/PKCE plus composed API readiness, Console session, independent registry and disabled product routes passed",
  );
} finally {
  globalThis.fetch = originalFetch;
  await closeRegistry();
  await databasePools.close();
}
