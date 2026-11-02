import { test, afterAll, mock } from "bun:test";
import assert from "node:assert/strict";
import { randomBytes, createHash } from "node:crypto";
import { assertTestDatabase } from "./helpers/database";
assertTestDatabase(process.env.API_TEST_DATABASE_URL!);
const issuer = "https://oauth-fixture.clerk.accounts.dev";
process.env.WEBDOCK_AUTH_PROVIDER = JSON.stringify({
  provider: "clerk",
  issuer,
  authorizedParties: ["https://console.example.invalid"],
  tokenMode: "session",
});
process.env.WEBDOCK_OAUTH_PROVIDER = "better-auth";
process.env.CLERK_ISSUER = issuer;
process.env.CLERK_SECRET_KEY = "sk_test_fixture";
process.env.CLERK_PUBLISHABLE_KEY = "pk_test_fixture";
const providerUsers = new Map<string, any>();
const providerMembers = new Map<string, any[]>();
// Only external Clerk Backend calls are replaced; OAuth, sessions, directory and Hosting use real code and SQL.
mock.module("@clerk/backend", () => ({
  createClerkClient: () => ({
    users: {
      getUser: async (id: string) => {
        const user = providerUsers.get(id);
        if (!user) throw Error("unknown Clerk user");
        return user;
      },
      getOrganizationMembershipList: async ({ userId }: { userId: string }) => {
        const data = providerMembers.get(userId) ?? [];
        return { data, totalCount: data.length };
      },
    },
  }),
  verifyToken: async () => {
    throw Error("OAuth must not fabricate a Clerk session token");
  },
}));
const { configureFullInstallation } = await import("../src/full-installation");
configureFullInstallation();
const { auth } = await import("../src/modules/auth/auth");
const { database, closeDatabases } = await import("../src/db/runtime");
const { createIdentity } = await import("../src/modules/auth/bootstrap");
const { offlineProvisioning } = await import("../src/modules/auth/offline");
const { mcpResource } = await import("../src/modules/auth/mcp");
const { hostingBridge } = await import("../src/modules/hosting/bridge");
const { migrateHosting } = await import("../src/modules/hosting/schema");
const origin = process.env.BETTER_AUTH_URL!;
afterAll(async () => {
  await auth.$context;
  await closeDatabases();
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
test("selected Clerk keeps real Better Auth OAuth sessions while owning Hosting permissions", async () => {
  if (
    !(
      await database.query(
        "SELECT to_regclass('webdock_auth.clerk_identity') AS existing",
      )
    ).rows[0].existing
  ) {
    await database.query(
      await Bun.file(
        new URL("../drizzle/0036_clerk_identity.sql", import.meta.url),
      ).text(),
    );
    await database.query(
      await Bun.file(
        new URL(
          "../drizzle/0038_clerk_legacy_eligibility.sql",
          import.meta.url,
        ),
      ).text(),
    );
  }
  await database.query(
    (await import("../src/modules/auth/clerk-schema")).clerkSchemaSQL,
  );
  await auth.$context;
  await migrateHosting(database, mcpResource);
  const root = await login(true),
    customer = await login();
  for (const account of [root, customer]) {
    const external = "user_" + account.user.id;
    providerUsers.set(external, {
      id: external,
      privateMetadata: { webdockRole: account === root ? "operator" : "user" },
      banned: false,
      locked: false,
      twoFactorEnabled: account === root,
      primaryEmailAddressId: "email_" + account.user.id,
      emailAddresses: [
        {
          id: "email_" + account.user.id,
          verification: { status: "verified" },
        },
      ],
    });
    await database.query(
      "INSERT INTO webdock_auth.clerk_identity(issuer,clerk_user_id,user_id,enabled,legacy_eligible) VALUES($1,$2,$3,true,true)",
      [issuer, external, account.user.id],
    );
  }
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
  await database.query(
    "INSERT INTO webdock_auth.clerk_organization(issuer,clerk_organization_id,organization_id,enabled) VALUES($1,$2,$3,true)",
    [issuer, "org_" + org, org],
  );
  providerMembers.set("user_" + customer.user.id, [
    {
      organization: { id: "org_" + org },
      role: "org:admin",
      permissions: ["org:projects:manage"],
    },
  ]);
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
    // Provision pre-existing legacy OAuth clients before the selector cutover.
    const selected = process.env.WEBDOCK_AUTH_PROVIDER;
    process.env.WEBDOCK_AUTH_PROVIDER = JSON.stringify({
      provider: "better-auth",
    });
    let app, resource;
    try {
      app = await create(true);
      resource = await create(false);
    } finally {
      process.env.WEBDOCK_AUTH_PROVIDER = selected;
    }
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
    const { selectedIdentityDirectory, resolveSessionReference } =
      await import("../src/modules/auth/principal");
    const { withDatabaseScope } = await import("@webdock/database-runtime");
    const { legacyClerkMemberships } =
      await import("../src/modules/auth/clerk-provider");
    const native = await auth.api.getSession({ headers: customer.headers });
    assert.ok(native);
    const reference = {
      subject: customer.user.id,
      sessionID: native.session.id,
    };
    const proof = {
      ...(await legacyClerkMemberships(customer.user.id))!,
      ...reference,
    };
    const scoped = (patch = {}, scopePatch = {}) =>
      withDatabaseScope(
        {
          kind: "session",
          authProvider: "clerk",
          ...reference,
          memberships: { ...proof, ...patch },
          ...scopePatch,
        },
        () => selectedIdentityDirectory.session(database, reference),
      );
    assert.equal(
      await resolveSessionReference(customer.headers),
      null,
      "a native cookie is not an OAuth authentication context",
    );
    assert.equal(
      await selectedIdentityDirectory.session(database, reference),
      null,
    );
    assert.equal((await scoped())?.role, "user");
    for (const patch of [
      { subject: root.user.id },
      { sessionID: "wrong" },
      { issuer: "https://wrong.example" },
      { userID: "user_wrong" },
      { verifiedAt: Date.now() + 60000 },
      { expiresAt: 1 },
      { expiresAt: Date.now() + 120000 },
    ])
      assert.equal(await scoped(patch), null);
    assert.equal(await scoped({}, { sessionID: "wrong" }), null);
    assert.equal(await scoped({}, { authProvider: "better-auth" }), null);
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS n FROM webdock_auth.clerk_identity_mapping WHERE local_subject=$1",
          [customer.user.id],
        )
      ).rows[0].n,
      0,
    );
    assert.equal(
      (
        await database.query(
          "SELECT count(*)::int AS n FROM webdock_auth.clerk_session_mapping WHERE local_session_id=$1",
          [native.session.id],
        )
      ).rows[0].n,
      0,
    );
    await database.query(
      `UPDATE webdock_auth.session SET "expiresAt"=now()-interval '1 second' WHERE id=$1`,
      [native.session.id],
    );
    assert.equal(await scoped(), null);
    await database.query(
      `UPDATE webdock_auth.session SET "expiresAt"=now()+interval '1 hour' WHERE id=$1`,
      [native.session.id],
    );
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
    // Retained local roles cannot replace revoked Clerk membership or operator authority.
    await database.query(
      `UPDATE webdock_auth."user" SET role='operator',"twoFactorEnabled"=true WHERE id=$1`,
      [customer.user.id],
    );
    providerMembers.set("user_" + customer.user.id, []);
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      404,
    );
    const externalUser = providerUsers.get("user_" + customer.user.id)!;
    externalUser.privateMetadata.webdockRole = "operator";
    externalUser.twoFactorEnabled = true;
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      200,
    );
    externalUser.twoFactorEnabled = false;
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      403,
    );
    externalUser.twoFactorEnabled = true;
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
      [customer.user.id],
    );
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      401,
    );
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [customer.user.id],
    );
    externalUser.privateMetadata.webdockRole = "user";
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      404,
    );
    providerMembers.set("user_" + customer.user.id, [
      {
        organization: { id: "org_" + org },
        role: "org:member",
        permissions: [],
      },
    ]);
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      200,
    );
    await database.query(
      "UPDATE webdock_auth.clerk_identity SET enabled=false WHERE user_id=$1",
      [customer.user.id],
    );
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      403,
    );
    await database.query(
      "UPDATE webdock_auth.clerk_identity SET enabled=true WHERE user_id=$1",
      [customer.user.id],
    );
    await auth.api.signOut({ headers: customer.headers });
    assert.equal(
      (await call({ action: "limits.get", customerID: cid })).status,
      401,
    );
  } finally {
    if (previous === undefined) delete process.env.WEBDOCK_STUDIO_CLIENT_ID;
    else process.env.WEBDOCK_STUDIO_CLIENT_ID = previous;
    await database.query(
      "DELETE FROM webdock_auth.clerk_organization WHERE issuer=$1 AND organization_id=$2",
      [issuer, org],
    );
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
    await database.query(
      "DELETE FROM webdock_auth.clerk_identity WHERE user_id=ANY($1::text[])",
      [[root.user.id, customer.user.id]],
    );
    await database.query(
      'DELETE FROM webdock_auth."user" WHERE id=ANY($1::text[])',
      [[root.user.id, customer.user.id]],
    );
  }
});
