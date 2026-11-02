import { expect, test } from "bun:test";

for (const selected of [false, true]) {
  test(`live Clerk membership proof owns global and custom roles (${selected ? "selected Clerk" : "legacy compatibility"})`, () => {
    const result = Bun.spawnSync([process.execPath, "--no-env-file", "-e", `
      import { mock } from "bun:test";
      import { strict as assert } from "node:assert";
      let mapped = true, organizationMapped = true;
      const metadata = { webdockRole: "operator" };
      const user = { id: "user_one", privateMetadata: metadata, banned: false, locked: false, twoFactorEnabled: true,
        primaryEmailAddressId: "email_one", emailAddresses: [{ id: "email_one", verification: { status: "verified" } }] };
      const member = { organization: { id: "org_one" }, role: "org:project_manager", permissions: ["org:projects:manage", "org:unknown:manage"] };
      mock.module("@clerk/backend", () => ({ verifyToken: async () => { throw Error("Worker cannot verify browser tokens"); }, createClerkClient: () => ({ users: {
        getUser: async () => user,
        getOrganizationMembershipList: async () => ({ data: [member], totalCount: 1 }),
      } }) }));
      const { authDatabase } = await import("./apps/control-plane/src/db/runtime");
      let reads = 0;
      authDatabase.query = async (sql) => {
        reads++;
        if (sql.includes("FROM webdock_auth.clerk_session_mapping")) return { rows: [] };
        if (sql.includes("SELECT clerk_user_id,enabled")) return { rows: mapped ? [{ clerk_user_id: "user_one", enabled: true }] : [] };
        if (sql.includes("SELECT o.clerk_organization_id")) return { rows: [{ clerk_organization_id: "org_one", organization_id: "local_org", customer_id: "customer" }] };
        if (sql.includes("SELECT enabled FROM webdock_auth.clerk_organization")) return { rows: organizationMapped ? [{ enabled: true }] : [] };
        if (sql.includes("SELECT 1 FROM webdock_auth.clerk_")) return { rows: [], rowCount: 0 };
        throw Error("Unexpected query: " + sql);
      };
      const { legacyClerkMemberships, linkedMembershipRole } = await import("./apps/control-plane/src/modules/auth/clerk-provider");
      if (${!selected}) {
        assert.equal(await legacyClerkMemberships("local_user"), undefined);
        assert.equal(await linkedMembershipRole("local_user", "local_org"), undefined);
        assert.equal(reads, 0);
        const { withConsolePrincipal } = await import("./apps/control-plane/src/modules/auth/principal");
        const native = { provider: "clerk", issuer: "https://identity.example", userID: "local_user", clerkUserID: "user_one", sessionID: "native_session", operator: true,
          organizationID: null, clerkOrganizationID: null, customerID: null, role: null, verifiedAt: Date.now(), expiresAt: Date.now()+59000, name: "Fixture", email: "fixture@example.invalid" };
        assert.equal((await withConsolePrincipal(native, () => legacyClerkMemberships("local_user")))?.operator, true);
        process.exit(0);
      }
      const proof = await legacyClerkMemberships("local_user");
      assert.equal(proof.operator, true);
      assert.equal(proof.twoFactorEnabled, true);
      user.twoFactorEnabled = false;
      assert.equal((await legacyClerkMemberships("local_user")).twoFactorEnabled, false);
      assert.deepEqual(proof.roles, [{ organizationID: "local_org", customerID: "customer", role: "member", permissions: ["org:projects:manage"] }]);
      const { withDatabaseScope } = await import("@webdock/database-runtime");
      const { selectedIdentityDirectory } = await import("./apps/control-plane/src/modules/auth/principal");
      const workerProof = { ...proof, sessionID: "local_session" };
      let revoked = false, banned = false;
      const db = { query: async sql => ({ rows: sql.includes("SELECT session_id") ? [] : revoked ? [] : [{ id: "local_user", role: "user", banned, emailVerified: false, mustChangePassword: true, twoFactorEnabled: false }] }) };
      const directory = () => selectedIdentityDirectory.session(db, { subject: "local_user", sessionID: "local_session" });
      const run = proof => withDatabaseScope({ kind: "system", purpose: "hosting-setup", authProvider: "clerk", memberships: proof }, directory);
      assert.equal((await run(workerProof))?.role, "operator");
      assert.equal((await run({ ...workerProof, operator: false })).role, "user");
      assert.equal(await run({ ...workerProof, sessionID: "wrong" }), null);
      assert.equal(await run({ ...workerProof, subject: "wrong" }), null);
      assert.equal(await run({ ...workerProof, issuer: "https://wrong.example" }), null);
      assert.equal(await run({ ...workerProof, expiresAt: 1 }), null);
      banned = true;
      assert.equal(await run(workerProof), null);
      banned = false;
      revoked = true;
      assert.equal(await run(workerProof), null);
      revoked = false;
      assert.deepEqual(await withDatabaseScope({ kind: "system", purpose: "hosting-setup", authProvider: "clerk", memberships: workerProof }, () => selectedIdentityDirectory.membership(db, "local_user", "customer")), { memberID: null, role: "member", permissions: ["org:projects:manage"] });
      metadata.webdockRole = "user";
      assert.equal((await legacyClerkMemberships("local_user")).operator, false);
      member.role = "org:admin";
      assert.equal((await legacyClerkMemberships("local_user")).operator, false);
      assert.equal((await linkedMembershipRole("local_user", "local_org")), "admin");
      user.id = "wrong_user";
      await assert.rejects(legacyClerkMemberships("local_user"));
      user.id = "user_one";
      organizationMapped = false;
      assert.equal(await linkedMembershipRole("local_user", "local_org"), ${selected ? "null" : "undefined"});
      mapped = false;
      ${selected ? 'await assert.rejects(legacyClerkMemberships("local_user"), /mapping is unavailable/);' : 'assert.equal(await legacyClerkMemberships("local_user"), undefined);'}
    `], {
      cwd: new URL("../../../", import.meta.url).pathname,
      env: {
        ...process.env,
        WEBDOCK_AUTH_PROVIDER: selected ? JSON.stringify({ provider: "clerk", issuer: "https://identity.example", authorizedParties: ["https://console.example"], audience: "console" }) : JSON.stringify({ provider: "better-auth" }),
        CLERK_ISSUER: "https://identity.example",
        CLERK_SECRET_KEY: "sk_test_fixture",
        CLERK_PUBLISHABLE_KEY: "pk_test_fixture",
        DATABASE_URL: "postgres://fixture:fixture@localhost:5432/webdock_test",
      },
    });
    expect(result.stderr.toString()).toBe("");
    expect(result.exitCode).toBe(0);
  });
}
