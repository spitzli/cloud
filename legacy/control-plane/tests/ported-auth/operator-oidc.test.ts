import { getCookies } from "better-auth/cookies";
import { cookiePolicy } from "../../src/modules/auth/cookie-policy";
import test from "node:test";
import { subtest } from "../helpers/node-test";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import {
  createIdentity,
  registerApplication,
} from "../../src/modules/auth/bootstrap";
import { currentClaims } from "../../src/modules/auth/authorization";
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
test("Operator dashboard wire flow rejects unauthorized tokens and checks live access", async (t) => {
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

  const session = await (await call("/get-session")).json();
  const metadata = {
    webdock_binding: app.binding,
    webdock_operator_only: true,
    webdock_allowed_subjects: [operator.id],
  };
  const setMetadata = async (value: unknown) =>
    database.query(
      'UPDATE webdock_auth."oauthClient" SET metadata=$2 WHERE "clientId"=$1',
      [app.clientID, JSON.stringify(value)],
    );
  await setMetadata(metadata);
  const exchange = async (
    mutate?: () => Promise<unknown>,
    scope = "openid profile email",
  ) => {
    await database.query('DELETE FROM webdock_auth."rateLimit"');
    const verifier = crypto.randomBytes(32).toString("base64url");
    const query = new URLSearchParams({
      client_id: app.clientID,
      redirect_uri: "http://127.0.0.1:3120/api/sso/callback",
      response_type: "code",
      scope,
      code_challenge: crypto
        .createHash("sha256")
        .update(verifier)
        .digest("base64url"),
      code_challenge_method: "S256",
      state: crypto.randomBytes(16).toString("hex"),
      nonce: crypto.randomBytes(16).toString("hex"),
    });
    const authorization = await call("/oauth2/authorize?" + query);
    assert.equal(authorization.status, 302);
    const code = new URL(
      authorization.headers.get("location")!,
    ).searchParams.get("code");
    assert.ok(code, "Fixture must obtain a real authorization code");
    if (mutate) await mutate();
    const response = await auth.handler(
      new Request(origin + "/api/auth/oauth2/token", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "authorization_code",
          client_id: app.clientID,
          client_secret: app.clientSecret,
          redirect_uri: "http://127.0.0.1:3120/api/sso/callback",
          code,
          code_verifier: verifier,
        }),
      }),
    );
    return { status: response.status, body: await response.json() };
  };
  const introspect = async (token: string) => {
    const response = await auth.handler(
      new Request(origin + "/api/auth/oauth2/introspect", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          client_id: app.clientID,
          client_secret: app.clientSecret,
          token,
        }),
      }),
    );
    return { status: response.status, body: await response.json() };
  };
  const denied = (response: {
    status: number;
    body: Record<string, unknown>;
  }) => {
    assert.notEqual(
      response.status,
      200,
      "Unauthorized client must receive no successful token response",
    );
    for (const key of ["id_token", "access_token", "refresh_token"])
      assert.equal(response.body[key], undefined);
  };
  await subtest(
    "non-allowlisted operator gets no tokens after code was issued",
    async () => {
      const response = await exchange(() =>
        setMetadata({ ...metadata, webdock_allowed_subjects: ["123456789"] }),
      );
      try {
        denied(response);
      } finally {
        await setMetadata(metadata);
      }
    },
  );
  await subtest(
    "real TOTP login succeeds with opaque access token",
    async () => {
      const response = await exchange();
      assert.equal(response.status, 200);
      assert.ok(response.body.id_token);
      assert.ok(response.body.access_token);
      assert.equal(response.body.refresh_token, undefined);
      const claims = await introspect(response.body.access_token);
      assert.equal(claims.body.active, true);
      assert.equal(claims.body.webdock_role, "operator");
      assert.equal(claims.body.sid, session.session.id);
    },
  );
  for (const [name, column, value] of [
    ["banned", "banned", true],
    ["email unverified", "emailVerified", false],
    ["password change required", "mustChangePassword", true],
    ["MFA removed", "twoFactorEnabled", false],
    ["customer role", "role", "user"],
  ] as const) {
    await subtest(
      name + " denies exchange and current introspection",
      async () => {
        const valid = await exchange();
        assert.equal(valid.status, 200);
        const response = await exchange(() =>
          database.query(
            `UPDATE webdock_auth."user" SET "${column}"=$2 WHERE id=$1`,
            [operator.id, value],
          ),
        );
        try {
          denied(response);
          assert.equal(
            (await introspect(valid.body.access_token)).body.active,
            false,
          );
        } finally {
          await database.query(
            `UPDATE webdock_auth."user" SET "${column}"=$2 WHERE id=$1`,
            [operator.id, column === "role" ? "operator" : !value],
          );
        }
      },
    );
  }
  for (const [name, invalid] of [
    ["empty allowlist", { ...metadata, webdock_allowed_subjects: [] }],
    [
      "numeric subject",
      { ...metadata, webdock_allowed_subjects: [Number(operator.id)] },
    ],
    ["unknown binding", { ...metadata, webdock_binding: "999" }],
    ["invalid marker", { ...metadata, webdock_operator_only: "true" }],
  ] as const) {
    await subtest(name + " fails closed", async () => {
      const valid = await exchange();
      const response = await exchange(() => setMetadata(invalid));
      try {
        denied(response);
        assert.equal(
          (await introspect(valid.body.access_token)).body.active,
          false,
        );
      } finally {
        await setMetadata(metadata);
      }
    });
  }
  await subtest(
    "disabled binding rejects exchange and existing token",
    async () => {
      const valid = await exchange();
      const response = await exchange(() =>
        database.query(
          "UPDATE webdock_auth.app_binding SET enabled=false WHERE id=$1",
          [app.binding],
        ),
      );
      try {
        denied(response);
        assert.equal(
          (await introspect(valid.body.access_token)).body.active,
          false,
        );
      } finally {
        await database.query(
          "UPDATE webdock_auth.app_binding SET enabled=true WHERE id=$1",
          [app.binding],
        );
      }
    },
  );
  await subtest("omitting openid cannot bypass the issuance gate", async () => {
    denied(await exchange(undefined, "profile email"));
  });
  await subtest(
    "expired session cannot introspect or obtain tokens",
    async () => {
      const valid = await exchange();
      const response = await exchange(() =>
        database.query(
          'UPDATE webdock_auth.session SET "expiresAt"=to_timestamp(0) WHERE id=$1',
          [session.session.id],
        ),
      );
      try {
        denied(response);
        assert.equal(
          (await introspect(valid.body.access_token)).body.active,
          false,
        );
      } finally {
        await database.query(
          'UPDATE webdock_auth.session SET "expiresAt"=$2 WHERE id=$1',
          [session.session.id, new Date(Date.now() + 3600000)],
        );
      }
    },
  );
  await subtest(
    "deleted central session does not become sessionless access",
    async () => {
      const valid = await exchange();
      await database.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        session.session.id,
      ]);
      assert.equal(
        (await introspect(valid.body.access_token)).body.active,
        false,
      );
    },
  );
});
