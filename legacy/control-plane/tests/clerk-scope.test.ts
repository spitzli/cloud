import { test, expect } from "bun:test";
import {
  withDatabaseScope,
  currentDatabaseScope,
} from "@webdock/database-runtime";
import {
  withConsolePrincipal,
  currentConsolePrincipal,
} from "../src/modules/auth/principal";
const p = {
  provider: "clerk" as const,
  issuer: "https://fixture.clerk.accounts.dev",
  userID: "1001",
  clerkUserID: "user_one",
  sessionID: "sess_one",
  operator: false,
  organizationID: "2001",
  clerkOrganizationID: "org_one",
  customerID: "3001",
  role: "admin" as const,
  verifiedAt: Date.now(),
  expiresAt: Date.now() + 60000,
  name: "Fixture",
  email: "fixture@example.test",
};
test("nested business scope cannot discard Clerk identity or extend its expiry", () => {
  withConsolePrincipal(p, () => {
    withDatabaseScope(
      { kind: "session", subject: p.userID, sessionID: p.sessionID },
      () => {
        expect(currentDatabaseScope()).toMatchObject({
          authProvider: "clerk",
          clerk: { issuer: p.issuer, operator: false, expiresAt: p.expiresAt },
        });
        expect(currentConsolePrincipal()?.userID).toBe(p.userID);
      },
    );
  });
  expect(currentConsolePrincipal()).toBeUndefined();
});
test("different identity cannot inherit Clerk proof", () =>
  withConsolePrincipal(p, () => {
    withDatabaseScope(
      { kind: "session", subject: "other", sessionID: "different" },
      () => expect(currentDatabaseScope()).not.toHaveProperty("clerk"),
    );
  }));
