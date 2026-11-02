import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes, createHash } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { offlineProvisioning } from "../../src/modules/auth/offline";
import { mcpResource } from "../../src/modules/auth/mcp";
import { hostingBridge } from "../../src/modules/hosting/bridge";
import { migrateHosting } from "../../src/modules/hosting/schema";
const u = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(u.hostname) ||
  u.pathname != "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local DB required");
const origin = process.env.BETTER_AUTH_URL!;
test.after(async () => {
  await auth.$context;
  await database.end();
});
async function login(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `hosting-oauth-${randomBytes(10).toString("hex")}@example.invalid`,
    name: "OAuth fixture",
    password,
    operator,
    mustChangePassword: false,
  });
  const r = await auth.handler(
    new Request(origin + "/api/auth/sign-in/email", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        "x-webdock-client-ip": `192.0.2.${1 + (randomBytes(1)[0] % 250)}`,
      },
      body: JSON.stringify({ email: user.email, password }),
    }),
  );
  assert.equal(r.status, 200);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  return {
    user,
    headers: new Headers({
      Origin: origin,
      Cookie: r.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
    }),
  };
}
test("real hosting OAuth grants customer access, rejects Registry scope and observes membership/session revocation", async () => {
  await auth.$context;
  await migrateHosting(database, mcpResource);
  const root = await login(true),
    customer = await login();
  const previous = process.env.WEBDOCK_STUDIO_CLIENT_ID;
  const cid = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Hosting OAuth fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [cid],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, customer.user.id],
  );
  const clients: string[] = [];
  try {
    const create = async (hosting: boolean) => {
      const id = (
        await database.query("SELECT webdock_auth.next_snowflake() AS id")
      ).rows[0].id;
      const c = await offlineProvisioning.run({ clientID: id }, () =>
        auth.api.adminCreateOAuthClient({
          headers: root.headers,
          body: {
            client_name: "Hosting test",
            redirect_uris: ["https://client.example/callback"],
            application_type: "web",
            scope: hosting
              ? "hosting:read hosting:write"
              : "openid profile email",
            grant_types: ["authorization_code"],
            response_types: ["code"],
            token_endpoint_auth_method: hosting ? "none" : "client_secret_post",
            require_pkce: true,
            skip_consent: false,
            metadata: hosting
              ? { webdock_hosting: true }
              : { webdock_binding: "fixture-studio" },
          },
        }),
      );
      clients.push(c.client_id);
      await auth.api.adminLinkClientResource({
        headers: root.headers,
        params: { identifier: mcpResource, client_id: c.client_id },
      });
      return c;
    };
    const app = await create(true),
      resource = await create(false);
    process.env.WEBDOCK_STUDIO_CLIENT_ID = resource.client_id;
    const verifier = randomBytes(32).toString("base64url");
    const params = new URLSearchParams({
      client_id: app.client_id,
      redirect_uri: "https://client.example/callback",
      response_type: "code",
      scope: "hosting:read",
      resource: mcpResource,
      state: "fixture",
      code_challenge: createHash("sha256").update(verifier).digest("base64url"),
      code_challenge_method: "S256",
    });
    const escalation = new URLSearchParams(params);
    escalation.set("scope", "hosting:read webdock:write");
    const bad = await auth.handler(
      new Request(origin + "/api/auth/oauth2/authorize?" + escalation, {
        headers: customer.headers,
      }),
    );
    assert.ok(
      new URL(bad.headers.get("location")!, origin).searchParams.has("error"),
    );
    const start = await auth.handler(
      new Request(origin + "/api/auth/oauth2/authorize?" + params, {
        headers: customer.headers,
      }),
    );
    assert.equal(start.status, 302);
    const consent = new URL(start.headers.get("location")!, origin);
    assert.equal(
      consent.pathname,
      "/consent",
      consent.searchParams.get("error_description") ??
        consent.searchParams.get("error") ??
        "unexpected redirect",
    );
    const granted = await auth.handler(
      new Request(origin + "/api/auth/oauth2/consent", {
        method: "POST",
        headers: new Headers([
          ...customer.headers,
          ["Content-Type", "application/json"],
        ]),
        body: JSON.stringify({
          accept: true,
          oauth_query: consent.search.slice(1),
        }),
      }),
    );
    assert.equal(granted.status, 200);
    const target = new URL((await granted.json()).url);
    const response = await auth.handler(
      new Request(origin + "/api/auth/oauth2/token", {
        method: "POST",
        headers: {
          Origin: origin,
          "Content-Type": "application/x-www-form-urlencoded",
        },
        body: new URLSearchParams({
          grant_type: "authorization_code",
          client_id: app.client_id,
          redirect_uri: "https://client.example/callback",
          code: target.searchParams.get("code")!,
          code_verifier: verifier,
          resource: mcpResource,
        }),
      }),
    );
    assert.equal(response.status, 200);
    const token = await response.json();
    assert.equal(token.scope, "hosting:read");
    const call = (command: unknown) =>
      hostingBridge(
        new Request(origin + "/api/hosting/bridge", {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Authorization:
              "Basic " +
              Buffer.from(
                resource.client_id + ":" + resource.client_secret,
              ).toString("base64"),
          },
          body: JSON.stringify({ accessToken: token.access_token, command }),
        }),
      );
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      200,
    );
    assert.equal(
      (
        await call({
          action: "limits.set",
          customerID: cid,
          values: { apps: 5 },
          revision: 0,
          subscriptionRevision: 0,
        })
      ).status,
      403,
    );
    await database.query(
      'DELETE FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2',
      [org, customer.user.id],
    );
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      404,
    );
    await auth.api.signOut({ headers: customer.headers });
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      401,
    );
  } finally {
    process.env.WEBDOCK_STUDIO_CLIENT_ID = previous;
    await database.query(
      'DELETE FROM webdock_auth.member WHERE "organizationId"=$1',
      [org],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [cid],
    );
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      cid,
    ]);
    await database.query(
      'UPDATE webdock_auth."oauthClient" SET disabled=true WHERE "clientId"=ANY($1)',
      [clients],
    );
  }
});
