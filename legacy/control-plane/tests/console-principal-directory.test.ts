import { test, expect } from "bun:test";
import { activeIdentity } from "@webdock/core/identity";
import {
  selectedIdentityDirectory,
  withConsolePrincipal,
  resolveSessionReference,
} from "../src/modules/auth/principal";
import type { ConsolePrincipal } from "../src/modules/auth/clerk-identity";
const principal: ConsolePrincipal = {
  provider: "clerk",
  issuer: "https://fixture.clerk.accounts.dev",
  userID: "local",
  clerkUserID: "user_one",
  sessionID: "sess_one",
  operator: true,
  organizationID: "org-local",
  clerkOrganizationID: "org_one",
  customerID: "customer",
  role: "admin",
  permissions: ["org:projects:manage"],
  verifiedAt: Date.now(),
  expiresAt: Date.now() + 60000,
  name: "Fixture",
  email: "fixture@example.test",
};
const row = {
  id: "local",
  role: "operator",
  banned: false,
  emailVerified: false,
  mustChangePassword: true,
  twoFactorEnabled: false,
};
function database(overrides = {}) {
  return {
    query: async (sql: string) => ({
      rows: sql.includes("SELECT session_id")
        ? []
        : sql.includes("SELECT organization_id")
          ? [{ organization_id: "org-local" }]
          : [{ ...row, ...overrides }],
    }),
  } as unknown as Parameters<typeof selectedIdentityDirectory.session>[0];
}
test("verified Console identity carries provider authorization without inventing MFA", async () => {
  await withConsolePrincipal(principal, async () => {
    expect(await resolveSessionReference(new Headers())).toEqual({
      subject: "local",
      sessionID: "sess_one",
    });
    const identity = await selectedIdentityDirectory.session(database(), {
      subject: "local",
      sessionID: "sess_one",
    });
    expect(identity?.twoFactorEnabled).toBe(false);
    expect(identity?.operatorAuthorized).toBe(true);
    expect(activeIdentity(identity)).toBe(true);
    expect(
      await selectedIdentityDirectory.session(database(), {
        subject: "local",
        sessionID: "wrong",
      }),
    ).toBeNull();
    expect(
      await selectedIdentityDirectory.session(database({ banned: true }), {
        subject: "local",
        sessionID: "sess_one",
      }),
    ).toBeNull();
    expect(
      await selectedIdentityDirectory.session(database({ role: "user" }), {
        subject: "local",
        sessionID: "sess_one",
      }),
    ).toMatchObject({ role: "operator", operatorAuthorized: true });
    expect(
      await selectedIdentityDirectory.membership(
        database(),
        "local",
        "other-customer",
      ),
    ).toEqual({ memberID: null, role: null, permissions: [] });
    expect(
      await selectedIdentityDirectory.membership(
        database(),
        "local",
        "customer",
      ),
    ).toEqual({ memberID: null, role: "admin", permissions: ["org:projects:manage"] });
  });
});
test("expired Console proof never becomes a local-cookie fallback", async () => {
  await withConsolePrincipal(
    { ...principal, expiresAt: Date.now() - 1 },
    async () => {
      await expect(resolveSessionReference(new Headers())).rejects.toThrow(
        "Sign in again.",
      );
    },
  );
});


test("Console directory projects ordinary Clerk role over a stale local operator", async () => {
  await withConsolePrincipal({ ...principal, operator: false }, async () => {
    expect(await selectedIdentityDirectory.session(database(), {
      subject: "local", sessionID: "sess_one",
    })).toMatchObject({ role: "user", operatorAuthorized: false });
  });
});
