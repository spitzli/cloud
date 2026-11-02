import { getCookies } from "better-auth/cookies";
import { cookiePolicy } from "../src/modules/auth/cookie-policy";
import test from "node:test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { auth } from "../src/modules/auth/auth";
import { database } from "../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../src/modules/auth/bootstrap";
import { currentClaims } from "../src/modules/auth/authorization";
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test"
)
  throw Error("Use the disposable local database");
const origin = process.env.BETTER_AUTH_URL!;
let cookies = "";
const cookie = (r: Response) => {
  const values = new Map(
    cookies
      .split("; ")
      .filter(Boolean)
      .map((x) => [x.split("=")[0], x]),
  );
  for (const c of r.headers.getSetCookie()) {
    const v = c.split(";")[0];
    values.set(v.split("=")[0], v);
  }
  cookies = [...values.values()].join("; ");
};
const call = async (path: string, body?: unknown) => {
  const response = await auth.handler(
    new Request(origin + "/api/auth" + path, {
      method: body ? "POST" : "GET",
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        "x-webdock-client-ip": "127.0.0.1",
        Cookie: cookies,
      },
      body: body ? JSON.stringify(body) : undefined,
    }),
  );
  cookie(response);
  return response;
};
function totp(uri: string) {
  const secret = new URL(uri).searchParams.get("secret")!;
  let bits = "";
  for (const c of secret.toUpperCase().replace(/=+$/, ""))
    bits += "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
      .indexOf(c)
      .toString(2)
      .padStart(5, "0");
  const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const h = crypto.createHmac("sha1", key).update(counter).digest();
  const at = h[h.length - 1] & 15;
  return String((h.readUInt32BE(at) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}
test("Real Better Auth flow: closed signup, password change, MFA, OIDC and immediate authorization revocation", async (t) => {
  await database.query(
    'TRUNCATE webdock_auth."user",webdock_auth."oauthClient",webdock_auth.organization,webdock_auth.verification,webdock_auth."rateLimit" CASCADE',
  );
  const password = crypto.randomBytes(24).toString("base64url");
  const operator = await createIdentity({
    email: "operator@example.invalid",
    name: "Operator",
    password,
    operator: true,
  });
  assert.match(operator.id, /^[0-9]+$/);
  assert.equal((BigInt(operator.id) >> 12n) & 1023n, 1n);
  assert.equal(
    (
      await call("/sign-up/email", {
        name: "No signup",
        email: "unknown@example.invalid",
        password,
      })
    ).status,
    400,
  );
  assert.equal(
    (await call("/sign-in/email", { email: operator.email, password })).status,
    200,
  );
  const app = await registerApplication({
    label: "Test app",
    origin: "http://127.0.0.1:3120",
    logoutPath: "/login",
    headers: new Headers({ Cookie: cookies }),
  });
  assert.equal((await currentClaims(operator.id, app.binding)).disabled, true);
  const changed = crypto.randomBytes(24).toString("base64url");
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
  assert.equal(
    (
      await database.query(
        'SELECT "mustChangePassword" FROM webdock_auth."user" WHERE id=$1',
        [operator.id],
      )
    ).rows[0].mustChangePassword,
    false,
  );
  const concurrent = await auth.api.signInEmail({
    body: { email: operator.email, password: changed },
    asResponse: true,
  });
  const earlierCookies = concurrent.headers
    .getSetCookie()
    .map((c) => c.split(";")[0])
    .join("; ");
  assert.equal(
    (
      await call("/organization/create", {
        name: "Blocked before MFA",
        slug: "blocked",
      })
    ).status,
    403,
  );
  const enrollment = await call("/two-factor/enable", { password: changed });
  assert.equal(enrollment.status, 200);
  const setup = await enrollment.json();
  assert.ok(setup.totpURI);
  assert.equal(
    (await call("/two-factor/verify-totp", { code: totp(setup.totpURI) }))
      .status,
    200,
  );
  const oldSession = await auth.handler(
    new Request(origin + "/api/auth/get-session", {
      headers: { Cookie: earlierCookies },
    }),
  );
  assert.equal(
    await oldSession.json(),
    null,
    "Enrollment revokes earlier non-MFA sessions",
  );
  assert.equal(
    (await currentClaims(operator.id, app.binding)).webdock_role,
    "operator",
  );
  await t.test("selected Clerk compatibility rejects local admin and organization management with an existing operator session", async () => {
    const previous = process.env.WEBDOCK_AUTH_PROVIDER;
    process.env.WEBDOCK_AUTH_PROVIDER = JSON.stringify({provider:"clerk",issuer:"https://fixture.clerk.accounts.dev",authorizedParties:["https://console.example.invalid"],tokenMode:"session"});
    try {
      assert.equal((await call("/admin/set-role", {userId:operator.id,role:"user"})).status,403);
      assert.equal((await call("/admin/ban-user", {userId:operator.id})).status,403);
      assert.equal((await call("/organization/create", {name:"Local bypass",slug:"local-bypass"})).status,403);
    } finally {
      if(previous === undefined)delete process.env.WEBDOCK_AUTH_PROVIDER;else process.env.WEBDOCK_AUTH_PROVIDER=previous;
      await database.query(`UPDATE webdock_auth."user" SET role='operator' WHERE id=$1`,[operator.id]);
    }
  });
  const crossOrigin = await auth.handler(
    new Request(origin + "/api/auth/change-password", {
      method: "POST",
      headers: {
        Origin: "https://customer.webdock.dev",
        Cookie: cookies,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        currentPassword: changed,
        newPassword: changed + "blocked",
      }),
    }),
  );
  assert.equal(
    crossOrigin.status,
    403,
    "Sibling domains cannot submit authenticated changes",
  );
  const verifier = crypto.randomBytes(32).toString("base64url");
  const nonce = crypto.randomBytes(18).toString("hex");
  const query = new URLSearchParams({
    client_id: app.clientID,
    redirect_uri: "http://127.0.0.1:3120/api/sso/callback",
    response_type: "code",
    scope: "openid profile email",
    code_challenge: crypto
      .createHash("sha256")
      .update(verifier)
      .digest("base64url"),
    code_challenge_method: "S256",
    state: "fixture-state",
    nonce,
  });
  const authorization = await call("/oauth2/authorize?" + query);
  assert.equal(authorization.status, 302);
  const destination = new URL(authorization.headers.get("location")!);
  assert.equal(destination.origin, "http://127.0.0.1:3120");
  const code = destination.searchParams.get("code");
  assert.ok(code);
  const tokenRequest = () =>
    auth.handler(
      new Request(origin + "/api/auth/oauth2/token", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "authorization_code",
          client_id: app.clientID,
          client_secret: app.clientSecret,
          redirect_uri: "http://127.0.0.1:3120/api/sso/callback",
          code: code!,
          code_verifier: verifier,
        }),
      }),
    );
  const tokenResponse = await tokenRequest();
  assert.equal(tokenResponse.status, 200);
  const tokens = await tokenResponse.json();
  assert.ok(tokens.id_token);
  const introspect = async () => {
    const r = await auth.handler(
      new Request(origin + "/api/auth/oauth2/introspect", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          token: tokens.access_token,
          client_id: app.clientID,
          client_secret: app.clientSecret,
        }),
      }),
    );
    assert.equal(r.status, 200);
    return r.json();
  };
  let claims = await introspect();
  assert.equal(claims.active, true);
  assert.equal(claims.webdock_role, "operator");
  assert.equal(claims.sub, operator.id);
  await database.query(
    'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
    [operator.id],
  );
  claims = await introspect();
  assert.ok(!claims.active || claims.disabled === true);
  await database.query(
    'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
    [operator.id],
  );
  await database.query(
    "UPDATE webdock_auth.app_binding SET enabled=false WHERE id=$1",
    [app.binding],
  );
  assert.equal((await introspect()).disabled, true);
  await database.query(
    "UPDATE webdock_auth.app_binding SET enabled=true WHERE id=$1",
    [app.binding],
  );
  const originalBrowserCookies = cookies;
  cookies = "";
  assert.equal(
    (await call("/sign-in/email", { email: operator.email, password: changed }))
      .status,
    200,
  );
  assert.equal(
    (
      await call("/two-factor/verify-backup-code", {
        code: setup.backupCodes[1],
      })
    ).status,
    200,
  );
  const otherBrowserCookies = cookies;
  const otherBrowser = await auth.api.getSession({
    headers: new Headers({ Cookie: otherBrowserCookies }),
  });
  assert.ok(otherBrowser);
  cookies = originalBrowserCookies;
  // The provider verifies its hint through its public JWKS endpoint. Route that
  // loopback request to the real handler without requiring a running web server.
  t.mock.method(globalThis, "fetch", async (input: Request | URL | string) => {
    assert.equal(String(input), origin + "/api/auth/jwks");
    return auth.handler(new Request(String(input)));
  });
  const logout = await call(
    "/oauth2/end-session?" +
      new URLSearchParams({
        id_token_hint: tokens.id_token,
        client_id: app.clientID,
        post_logout_redirect_uri: "http://127.0.0.1:3120/login",
      }),
  );
  assert.equal(logout.status, 302);
  assert.equal(logout.headers.get("location"), "http://127.0.0.1:3120/login");
  assert.ok(
    logout.headers.getSetCookie().some((value) => value.includes("Max-Age=0")),
    "Central browser cookie is cleared",
  );
  assert.equal(
    await auth.api.getSession({
      headers: new Headers({ Cookie: originalBrowserCookies }),
    }),
    null,
    "RP logout invalidates the central browser session",
  );
  assert.equal(
    (
      await auth.api.getSession({
        headers: new Headers({ Cookie: otherBrowserCookies }),
      })
    )?.session.id,
    otherBrowser.session.id,
    "Other devices remain signed in",
  );
  assert.equal((await introspect()).active, false);
  assert.equal((await tokenRequest()).status, 400, "Code replay denied");
  const nextLogin = await call("/sign-in/email", {
    email: operator.email,
    password: changed,
  });
  assert.equal(nextLogin.status, 200);
  assert.equal((await nextLogin.json()).twoFactorRedirect, true);
  assert.equal(
    (await call("/two-factor/verify-totp", { code: "000000" })).status,
    401,
  );
  assert.equal(
    (
      await call("/two-factor/verify-backup-code", {
        code: setup.backupCodes[0],
      })
    ).status,
    200,
  );
});
test("Organisation ownership never becomes platform access; membership and project grants are both required", async () => {
  const password = crypto.randomBytes(24).toString("base64url");
  const member = await createIdentity({
    name: "Org owner",
    email: "owner@example.invalid",
    password,
    mustChangePassword: false,
  });
  const orgResponse = await call("/organization/create", {
    name: "Customer A",
    slug: "customer-a",
  });
  assert.equal(orgResponse.status, 200);
  const org = await orgResponse.json();
  const orgBResponse = await call("/organization/create", {
    name: "Customer B",
    slug: "customer-b",
  });
  assert.equal(orgBResponse.status, 200);
  const orgB = await orgBResponse.json();
  const app = await registerApplication({
    label: "Customer A CMS",
    origin: "https://customer.example",
    logoutPath: "/admin/login",
    organizationID: org.id,
    headers: new Headers({ Cookie: cookies }),
  });
  await database.query(
    `INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,'owner',now())`,
    [org.id, member.id],
  );
  assert.equal((await currentClaims(member.id, app.binding)).disabled, true);
  await database.query(
    "INSERT INTO webdock_auth.project_grant(user_id,binding_id,organization_id,role) VALUES($1,$2,$3,'editor')",
    [member.id, app.binding, org.id],
  );
  assert.equal(
    (await currentClaims(member.id, app.binding)).webdock_role,
    "editor",
  );
  await database.query(
    "UPDATE webdock_auth.project_grant SET organization_id=$1 WHERE user_id=$2",
    [orgB.id, member.id],
  );
  assert.equal((await currentClaims(member.id, app.binding)).disabled, true);
  await database.query(
    "UPDATE webdock_auth.project_grant SET organization_id=$1 WHERE user_id=$2",
    [org.id, member.id],
  );
  await database.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
    member.id,
  ]);
  assert.equal((await currentClaims(member.id, app.binding)).disabled, true);
  cookies = "";
  assert.equal(
    (await call("/sign-in/email", { email: member.email, password })).status,
    200,
  );
  await call("/update-user", { role: "operator", mustChangePassword: false });
  assert.equal(
    (
      await database.query('SELECT role FROM webdock_auth."user" WHERE id=$1', [
        member.id,
      ])
    ).rows[0].role,
    "user",
  );
  assert.ok([401, 403].includes((await call("/admin/list-users")).status));
  assert.ok(
    [401, 403].includes(
      (
        await call("/oauth2/create-client", {
          redirect_uris: ["https://attacker.example/callback"],
        })
      ).status,
    ),
  );
});
test("Production cookies cannot be set by customer sibling subdomains", () => {
  const cookies = getCookies({
    ...auth.options,
    baseURL: "https://auth.webdock.dev",
    advanced: {
      ...auth.options.advanced,
      ...cookiePolicy("https://auth.webdock.dev"),
    },
  });
  for (const cookie of Object.values(cookies)) {
    assert.match(cookie.name, /^__Host-/);
    assert.equal(cookie.attributes.secure, true);
    assert.equal(cookie.attributes.path, "/");
    assert.equal(cookie.attributes.domain, undefined);
    assert.equal(cookie.attributes.httpOnly, true);
  }
});
test.after(async () => database.end());
