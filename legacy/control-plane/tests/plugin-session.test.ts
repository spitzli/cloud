import { test, expect } from "bun:test";
import {
  currentDatabaseScope,
  withDatabaseScope,
} from "@webdock/database-runtime";
import * as sessions from "../src/modules/plugins/invocation-session";
const principal: any = {
  userID: "100",
  sessionID: "sess_one",
  issuer: "https://clerk.example",
  clerkUserID: "user_one",
  clerkOrganizationID: "org_one",
  permissions: ["new"],
  expiresAt: Date.now() + 60000,
};
const session: any = {
  kind: "session",
  subject: "100",
  sessionID: "sess_one",
  clerk: {
    issuer: principal.issuer,
    userID: "user_one",
    clerkOrganizationID: "org_one",
    permissions: ["old"],
  },
};
test("plugin callback revalidates native Clerk identity and preserves invocation with fresh proof", async () => {
  expect(typeof sessions.createPluginSessionRunner).toBe("function");
  let reference: any;
  const run = sessions.createPluginSessionRunner({
    revalidate: async (value) => {
      reference = value;
      return principal;
    },
    withPrincipal: (_principal, fn) =>
      withDatabaseScope(
        { ...session, clerk: { ...session.clerk, permissions: ["new"] } },
        fn,
      ),
  });
  const scope: any = await run(session, "invocation", async () =>
    currentDatabaseScope(),
  );
  expect(reference).toEqual({
    issuer: principal.issuer,
    userID: "user_one",
    sessionID: "sess_one",
    organizationID: "org_one",
  });
  expect(scope.pluginInvocationID).toBe("invocation");
  expect(scope.clerk.permissions).toEqual(["new"]);
});
test("revoked or remapped stored Console identity never enters callback", async () => {
  let called = false;
  for (const revalidate of [
    async () => {
      throw Error("revoked");
    },
    async () => ({ ...principal, userID: "other" }),
  ]) {
    const run = sessions.createPluginSessionRunner({
      revalidate,
      withPrincipal: (_principal, fn) => fn(),
    });
    await expect(
      run(session, "invocation", async () => {
        called = true;
      }),
    ).rejects.toBeDefined();
  }
  expect(called).toBe(false);
});
test("native principal restoration carries fresh provider authority into nested Hosting scope", async () => {
  const { withConsolePrincipal, currentConsolePrincipal } =
    await import("../src/modules/auth/principal");
  const { withHostingDatabase } = await import("../src/db/database-context");
  const live = {
    ...principal,
    provider: "clerk" as const,
    operator: false,
    organizationID: "200",
    customerID: "300",
    role: "admin" as const,
    name: "Fixture",
    email: "fixture@example.invalid",
    verifiedAt: Date.now(),
  };
  const run = sessions.createPluginSessionRunner({
    revalidate: async () => live,
    withPrincipal: withConsolePrincipal,
  });
  await run(session, "native-invocation", async () =>
    withHostingDatabase(
      {
        subject: "100",
        sessionID: "sess_one",
        source: "studio",
        scopes: ["hosting:read"],
      },
      async () => {
        expect(currentConsolePrincipal()?.clerkUserID).toBe("user_one");
        const scope = currentDatabaseScope();
        expect(scope?.kind).toBe("session");
        if (scope?.kind === "session") {
          expect(scope.pluginInvocationID).toBe("native-invocation");
          expect(scope.clerk?.permissions).toEqual(["new"]);
          expect(scope.clerk?.organizationID).toBe("200");
        }
      },
    ),
  );
  expect(currentConsolePrincipal()).toBeUndefined();
});
