import { test, expect } from "bun:test";
import { fileURLToPath } from "node:url";

test("OAuth and MCP claims recheck provider operator roles and preserve default local authority", () => {
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "-e",
      `
    import { mock } from "bun:test";
    import assert from "node:assert/strict";
    let proof;
    let principal;
    const row = { id: "123", role: "operator", banned: false, emailVerified: true, twoFactorEnabled: true, mustChangePassword: false, enabled: true, client_disabled: false };
    mock.module("./apps/control-plane/src/db/runtime", () => ({ authDatabase: { query: async () => ({ rows: [row], rowCount: 1 }) } }));
    mock.module("./apps/control-plane/src/modules/auth/clerk-provider", () => ({ legacyClerkMemberships: async () => proof, linkedMembershipRole: async () => null }));
    mock.module("./apps/control-plane/src/modules/auth/principal", () => ({ currentConsolePrincipal: () => principal, consoleMembership: () => null }));
    const { currentClaims } = await import("./apps/control-plane/src/modules/auth/authorization");
    const { currentMCPClaims, currentHostingMCPClaims } = await import("./apps/control-plane/src/modules/auth/mcp");
    for (const providerOperator of [false, true, false]) {
      proof = { operator: providerOperator, twoFactorEnabled: true, roles: [] };
      assert.equal((await currentMCPClaims("123")).webdock_role, providerOperator ? "operator" : undefined);
      assert.equal((await currentClaims("123", "binding")).webdock_role, providerOperator ? "operator" : undefined);
      assert.equal((await currentHostingMCPClaims("123")).webdock_role, providerOperator ? "hosting-operator" : "hosting-customer");
    }
    row.role = "user";
    row.twoFactorEnabled = false;
    row.emailVerified = false;
    proof = { operator: true, twoFactorEnabled: true, roles: [] };
    assert.equal((await currentMCPClaims("123")).webdock_role, "operator");
    assert.equal((await currentClaims("123", "binding")).webdock_role, "operator");
    proof.twoFactorEnabled = false;
    assert.equal((await currentMCPClaims("123")).disabled, true);
    assert.equal((await currentClaims("123", "binding")).disabled, true);
    row.role = "operator"; row.twoFactorEnabled = true; row.emailVerified = true;
    proof = undefined;
    assert.equal((await currentMCPClaims("123")).webdock_role, "operator");
    assert.equal((await currentClaims("123", "binding")).webdock_role, "operator");
    process.env.WEBDOCK_STUDIO_CLIENT_ID = "studio";
    row.client_id = "studio";
    row.role = "unknown";
    assert.equal((await currentClaims("123", "binding")).disabled, true);
    row.role = "user";
    assert.equal((await currentClaims("123", "binding")).webdock_role, "reader");
    delete process.env.WEBDOCK_STUDIO_CLIENT_ID;
    row.role = "operator";
    principal = { userID: "123", operator: false };
    assert.equal((await currentClaims("123", "binding")).disabled, true);
    principal.operator = true; row.role = "user";
    assert.equal((await currentClaims("123", "binding")).webdock_role, "operator");
  `,
    ],
    {
      cwd: fileURLToPath(new URL("../../..", import.meta.url)),
      env: { ...process.env, WEBDOCK_STUDIO_CLIENT_ID: "" },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  expect(result.exitCode, new TextDecoder().decode(result.stderr)).toBe(0);
});

test("compatibility business identity projects provider authority on every read", () => {
  const result = Bun.spawnSync(
    [
      process.execPath,
      "--no-env-file",
      "-e",
      `
    import { mock } from "bun:test";
    import assert from "node:assert/strict";
    let projected = { subject: "123", sessionID: "session", role: "user", banned: false, emailVerified: true, twoFactorEnabled: false, mustChangePassword: false, preview: false };
    const raw = { user: { id: "123", name: "Local", role: "operator", emailVerified: false, twoFactorEnabled: true }, session: { id: "session" } };
    mock.module("./apps/control-plane/src/modules/auth/auth", () => ({ auth: { api: { getSession: async () => raw } } }));
    mock.module("./apps/control-plane/src/db/runtime", () => ({ authDatabase: {} }));
    mock.module("./apps/control-plane/src/modules/auth/mcp", () => ({ currentMCPClaims: async () => ({ disabled: true }) }));
    mock.module("./apps/control-plane/src/modules/auth/principal", () => ({ currentConsolePrincipal: () => null, resolveSessionReference: async () => ({ subject: "123", sessionID: "session" }), selectedIdentityDirectory: { session: async () => projected } }));
    const { getBusinessIdentity } = await import("./apps/control-plane/src/modules/auth/business-identity");
    for (const role of ["user", "operator", "user"]) {
      projected.role = role;
      const result = await getBusinessIdentity(new Headers());
      assert.equal(result.user.role, role);
      assert.equal(result.user.emailVerified, true);
      assert.equal(result.user.twoFactorEnabled, false);
    }
    projected = null;
    assert.equal(await getBusinessIdentity(new Headers()), null);
  `,
    ],
    {
      cwd: fileURLToPath(new URL("../../..", import.meta.url)),
      env: {
        ...process.env,
        WEBDOCK_AUTH_PROVIDER: JSON.stringify({
          provider: "clerk",
          issuer: "https://identity.example",
          authorizedParties: ["https://console.example"],
          audience: "webdock",
        }),
      },
      stdout: "pipe",
      stderr: "pipe",
    },
  );
  expect(result.exitCode, new TextDecoder().decode(result.stderr)).toBe(0);
});
