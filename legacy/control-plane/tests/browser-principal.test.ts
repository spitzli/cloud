import { test, expect } from "bun:test";
import type {
  DatabaseReader,
  IdentityDirectory,
  SessionReference,
} from "@webdock/core/identity";
import { createBrowserPrincipal } from "../src/modules/auth/browser-principal";
function fixture(provider: "clerk" | "better-auth" = "clerk") {
  let reference: SessionReference | null = { subject: "123", sessionID: "456" };
  const reads: string[] = [];
  const directory = (name: string): IdentityDirectory => ({
    session: async () => null,
    subject: async () => {
      reads.push(name);
      return null;
    },
    membership: async () => null,
  });
  const principal = createBrowserPrincipal({
    provider,
    local: { authenticate: async () => reference },
    localDirectory: directory("legacy"),
    clerkDirectory: () => directory("clerk"),
  });
  return {
    principal,
    reads,
    setReference(value: SessionReference | null) {
      reference = value;
    },
  };
}
test("Clerk selection never falls back to a valid legacy browser cookie", async () => {
  const f = fixture();
  expect(
    await f.principal.resolveSessionReference(
      new Headers({ cookie: "legacy=valid" }),
    ),
  ).toBeNull();
  expect(f.principal.currentTrustedBrowserReference()).toBeNull();
  const legacy = fixture("better-auth");
  expect(
    await legacy.principal.resolveSessionReference(
      new Headers({ cookie: "legacy=valid" }),
    ),
  ).toEqual({ subject: "123", sessionID: "456" });
});
test("trusted native headers recheck exact identity and cannot carry context into another request", async () => {
  const f = fixture(),
    headers = new Headers({ cookie: "private=verified" });
  await f.principal.withTrustedLocalSession(
    headers,
    { subject: "123", sessionID: "456" },
    "clerk",
    async () => {
      expect(await f.principal.resolveSessionReference(headers)).toEqual({
        subject: "123",
        sessionID: "456",
      });
      expect(
        await f.principal.resolveSessionReference(
          new Headers({ cookie: "other=token" }),
        ),
      ).toBeNull();
      f.setReference({ subject: "999", sessionID: "456" });
      expect(await f.principal.resolveSessionReference(headers)).toBeNull();
      f.setReference(null);
      expect(await f.principal.resolveSessionReference(headers)).toBeNull();
    },
  );
  expect(f.principal.currentTrustedBrowserReference()).toBeNull();
  expect(f.principal.trustedLocalSessionKind(headers)).toBeUndefined();
});
test("legacy delegated transport still uses the selected Clerk identity authority", async () => {
  const f = fixture(),
    db = {} as DatabaseReader,
    headers = new Headers({ cookie: "native=internal" });
  await f.principal.selectedIdentityDirectory.subject(db, "123");
  await f.principal.withTrustedLocalSession(
    headers,
    { subject: "123", sessionID: "456" },
    "legacy",
    async () => {
      expect(f.principal.currentTrustedBrowserReference()).toBeNull();
      await f.principal.selectedIdentityDirectory.subject(db, "123");
    },
  );
  await f.principal.selectedIdentityDirectory.subject(db, "123");
  expect(f.reads).toEqual(["clerk", "clerk", "clerk"]);
});

import { validateClerkWorkerConfiguration } from "../src/modules/auth/clerk-configuration";
test("Clerk Mail worker rejects missing backend verification key without needing a bootstrap connection", () => {
  const configuration = {
    provider: "clerk",
    issuer: "https://fixture.clerk.accounts.dev",
    authorizedParties: ["https://console.example.test"],
    audience: "webdock-console",
  };
  const env = { WEBDOCK_AUTH_PROVIDER: JSON.stringify(configuration) };
  expect(() => validateClerkWorkerConfiguration(["mail"], env)).toThrow();
  expect(() => validateClerkWorkerConfiguration(["hosting"], env)).toThrow();
  expect(() =>
    validateClerkWorkerConfiguration(["storage"], env),
  ).not.toThrow();
  expect(() =>
    validateClerkWorkerConfiguration(["mail"], {
      ...env,
      CLERK_SECRET_KEY: "sk_test_fixture",
    }),
  ).not.toThrow();
  expect(() => validateClerkWorkerConfiguration(["mail"], {})).not.toThrow();
});
