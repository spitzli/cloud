import { expect, test } from "bun:test";

test("mail authorization and people use live provider roles, never local operator flags", () => {
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "-e",
      `
    import { mock } from "bun:test";
    import assert from "node:assert/strict";
    let principal = null;
    let selected = true, mapped = true, orgMapped = true, present = true, reads = 0;
    const link = { user_id: "123", clerk_user_id: "user_one", issuer: "https://identity.example", enabled: true, role: "operator", banned: false };
    const providerUser = { id: "user_one", banned: false, locked: false, privateMetadata: {}, primaryEmailAddressId: "email", emailAddresses: [{ id: "email", verification: { status: "verified" } }] };
    const member = { id: "mem_one", organization: { id: "org_one" }, role: "org:member", publicUserData: { userId: "user_one" } };
    const people = [
      { id: "123", name: "Member", email: "member@example.test", clerk_user_id: "user_one" },
      { id: "124", name: "Demoted", email: "demoted@example.test", clerk_user_id: "user_demoted" },
      { id: "125", name: "Operator", email: "op@example.test", clerk_user_id: "user_operator" },
    ];
    const db = { query: async (sql) => {
      reads++;
      if(sql.includes("to_regclass")) return { rows: [{ present }] };
      if(sql.includes("SELECT i.*")) return { rows: mapped ? [link] : [] };
      if(sql.includes("SELECT organization_id")) return { rows: [{ organization_id: "local_org" }] };
      if(sql.includes("clerk_organization")) return { rows: orgMapped ? [{ issuer: link.issuer, organization_id: "local_org", clerk_organization_id: "org_one", enabled: true }] : [], rowCount: orgMapped ? 1 : 0 };
      if(sql.includes("SELECT u.id")) return { rows: people };
      throw Error(sql);
    } };
    mock.module("./apps/control-plane/src/modules/auth/principal", () => ({ currentConsolePrincipal: () => principal }));
    mock.module("./apps/control-plane/src/db/runtime", () => ({ authDatabase: db }));
    mock.module("./apps/control-plane/src/modules/auth/console-configuration", () => ({ consoleAuthConfiguration: () => ({ requireExistingMapping: selected, issuer: link.issuer }) }));
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider", () => ({ clerkProvider: () => ({ users: { getUser: async id => id === "user_one" ? providerUser : { ...providerUser, id, privateMetadata: id === "user_operator" ? { webdockRole: "operator" } : {} } }, organizations: { getOrganizationMembershipList: async () => ({ data: [member], totalCount: 1 }) } }), liveMemberships: async () => [member] }));
    const { clerkMailAccess, clerkMailPeople } = await import("./apps/control-plane/src/modules/mail/mail-clerk-access");
    let access = await clerkMailAccess(db, "123", "456");
    assert.equal(access.operator, false);
    assert.equal(access.canManage, false);
    member.role = "org:custom_mail";
    assert.equal((await clerkMailAccess(db, "123", "456")).canManage, false);
    member.role = "org:admin";
    assert.equal((await clerkMailAccess(db, "123", "456")).canManage, true);
    member.role = "org:member"; link.role = "user";
    providerUser.privateMetadata = { webdockRole: "operator" };
    assert.equal((await clerkMailAccess(db, "123", "456")).operator, true);
    providerUser.privateMetadata = {}; link.role = "operator";
    assert.equal((await clerkMailAccess(db, "123", "456")).canManage, false);
    member.role = "org:custom_mail";
    assert.deepEqual((await clerkMailPeople(db, "456", true)).map(p => p.id), ["123", "125"]);
    assert.deepEqual((await clerkMailPeople(db, "456", false)).map(p => p.id), ["123"]);
    mapped = false; orgMapped = false;
    assert.equal(await clerkMailAccess(db, "123", "456"), null);
    assert.deepEqual(await clerkMailPeople(db, "456"), []);
    mapped = true; providerUser.privateMetadata = { webdockRole: "operator" };
    assert.equal(await clerkMailAccess(db, "123", "456"), null);
    present = false;
    assert.equal(await clerkMailAccess(db, "123", "456"), null);
    assert.deepEqual(await clerkMailPeople(db, "456"), []);
    selected = false; reads = 0;
    assert.equal(await clerkMailAccess(db, "123", "456"), undefined);
    assert.equal(await clerkMailPeople(db, "456"), undefined);
    assert.equal(reads, 0);
    principal = { provider: "clerk", userID: "123" };
    mapped = true; orgMapped = true; present = true;
    providerUser.privateMetadata = {};
    assert.equal((await clerkMailAccess(db, "123", "456")).operator, false);
    assert.deepEqual((await clerkMailPeople(db, "456", false)).map(p => p.id), ["123"]);
    assert.ok(reads > 0);
  `,
    ],
    {
      cwd: new URL("../../../", import.meta.url).pathname,
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  expect(result.exitCode, result.stderr.toString()).toBe(0);
});
