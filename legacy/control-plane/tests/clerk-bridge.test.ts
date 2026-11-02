import { test, expect } from "bun:test";
import type { Pool } from "pg";
import type { ClerkExternalEvidence } from "@webdock/auth-clerk";
import {
  createClerkSessionBridge,
  allowedClerkProjection,
  type BridgeUser,
} from "../src/modules/auth/clerk-bridge";
const now = 1_800_000_000_000;
const evidence: ClerkExternalEvidence = {
  external: {
    issuer: "https://clerk.example.test",
    subject: "user_example",
    sessionID: "sess_example",
  },
  user: {
    id: "user_example",
    role: "operator",
    banned: false,
    locked: false,
    twoFactorEnabled: true,
    emailVerified: true,
  },
  expiresAt: now + 120_000,
  factorVerificationAge: [0, 0],
};
const user: BridgeUser = {
  id: "123",
  name: "Operator",
  email: "local@example.test",
  role: "operator",
  banned: false,
  emailVerified: true,
  twoFactorEnabled: true,
  mustChangePassword: false,
};
function fixture(
  options: {
    mapped?: boolean;
    existing?: {
      userId: string;
      external_subject: string;
      local_session_id: string;
      token: string;
    };
    external?: ClerkExternalEvidence | null;
    local?: BridgeUser;
  } = {},
) {
  const calls: { sql: string; values: unknown[] }[] = [];
  let connections = 0;
  const client = {
    query: async (sql: string, values: unknown[] = []) => {
      calls.push({ sql, values });
      const rows = sql.startsWith("SELECT u.id")
        ? options.mapped === false
          ? []
          : [options.local ?? user]
        : sql.startsWith("SELECT m.local_session_id")
          ? options.existing
            ? [options.existing]
            : []
          : sql.startsWith("INSERT INTO webdock_auth.session")
            ? [{ id: "456", token: "private-native-token" }]
            : [];
      return { rows, rowCount: rows.length };
    },
    release() {},
  };
  const database = {
    connect: async () => {
      connections++;
      return client;
    },
  } as unknown as Pick<Pool, "connect">;
  const bridge = createClerkSessionBridge({
    database,
    verifyExternal: async () =>
      options.external === undefined ? evidence : options.external,
    signSession: async () => "private-cookie=signed-native-token",
    origin: "https://auth.example.test",
    now: () => now,
  });
  return {
    bridge,
    calls,
    get connections() {
      return connections;
    },
  };
}
test("Clerk bridge requires an explicit local mapping and never matches email", async () => {
  const f = fixture({ mapped: false });
  await expect(
    f.bridge(new Headers({ authorization: "Bearer clerk.token" })),
  ).rejects.toThrow();
  expect(f.calls.some((c) => c.sql.startsWith("INSERT"))).toBe(false);
  expect(
    f.calls.filter((c) => c.sql.startsWith("SELECT u.id"))[0].values,
  ).toEqual([evidence.external.issuer, evidence.external.subject]);
});
test("Clerk bridge fails closed before database access for invalid or absent external proof", async () => {
  for (const headers of [
    new Headers(),
    new Headers({ cookie: "webdock-auth.session_token=legacy" }),
    new Headers({ authorization: "Bearer invalid" }),
  ]) {
    const f = fixture({ external: null });
    await expect(f.bridge(headers)).rejects.toThrow();
    expect(f.connections).toBe(0);
  }
});
test("live Clerk MFA still requires completed local setup and a verified second factor", () => {
  expect(allowedClerkProjection(user, evidence)).toBe(true);
  for (const proof of [
    { ...evidence, factorVerificationAge: null },
    { ...evidence, factorVerificationAge: [0, -1] as [number, number] },
    { ...evidence, user: { ...evidence.user, twoFactorEnabled: false } },
  ])
    expect(allowedClerkProjection(user, proof)).toBe(false);
  for (const local of [
    { ...user, mustChangePassword: true },
    { ...user, banned: true },
  ])
    expect(allowedClerkProjection(local, evidence)).toBe(false);
});
test("Clerk projection bounds local expiry and keeps native credentials out of its safe browser shape", async () => {
  const f = fixture();
  const projected = await f.bridge(
    new Headers({ authorization: "Bearer verified.token" }),
  );
  expect(projected.actor).toEqual({ userID: "123", sessionID: "456" });
  expect(projected.headers.get("cookie")).toBe(
    "private-cookie=signed-native-token",
  );
  expect(JSON.stringify(projected.safe)).not.toContain("token");
  expect(JSON.stringify(projected.safe)).not.toContain("cookie");
  const inserted = f.calls.find((c) =>
    c.sql.startsWith("INSERT INTO webdock_auth.session"),
  )!;
  expect(inserted.values[2]).toEqual(new Date(now + 60_000));
  expect(projected.safe.user.id).toBe("123");
});
test("projection cannot reuse a session bound to another immutable subject", async () => {
  const f = fixture({
    existing: {
      userId: "999",
      external_subject: evidence.external.subject,
      local_session_id: "456",
      token: "private",
    },
  });
  await expect(
    f.bridge(new Headers({ authorization: "Bearer valid.token" })),
  ).rejects.toThrow();
  expect(f.calls.some((c) => c.sql.startsWith("INSERT"))).toBe(false);
});
test("lease is refreshed before native session update so database clamp permits only the verified expiry", async () => {
  const f = fixture({
    existing: {
      userId: "123",
      external_subject: evidence.external.subject,
      local_session_id: "456",
      token: "private",
    },
  });
  await f.bridge(new Headers({ authorization: "Bearer valid.token" }));
  expect(
    f.calls.findIndex((c) =>
      c.sql.startsWith("INSERT INTO webdock_auth.clerk_session_mapping"),
    ),
  ).toBeLessThan(
    f.calls.findIndex((c) => c.sql.startsWith("UPDATE webdock_auth.session")),
  );
});

import { consoleAuthConfiguration } from "../src/modules/auth/console-configuration";
import { createConsoleVerifier } from "../src/modules/auth/clerk-identity";
test("compatibility and selected Clerk delegation each require explicit mappings and honor revocation", async () => {
  const options = { mapped: true };
  const f = fixture(options);
  const headers = new Headers({ authorization: "Bearer verified.token" });
  expect((await f.bridge(headers)).actor.userID).toBe("123");
  expect(
    f.calls.some(
      (c) =>
        c.sql.includes("clerk_identity_mapping") &&
        c.sql.includes("revoked_at IS NULL"),
    ),
  ).toBe(true);
  expect(f.calls.some((c) => /clerk_identity\s/.test(c.sql))).toBe(false);
  options.mapped = false;
  await expect(f.bridge(headers)).rejects.toThrow();
  for (const activation of [undefined, "clerk"]) {
    const config = consoleAuthConfiguration({
      WEBDOCK_AUTH_PROVIDER: JSON.stringify({
        provider: "clerk",
        issuer: evidence.external.issuer,
        authorizedParties: ["https://console.example.test"],
        audience: "webdock-console",
      }),
      WEBDOCK_CONSOLE_AUTH_PROVIDER: activation,
      WEBDOCK_CONSOLE_SERVICE_KEY: "s".repeat(32),
    });
    expect(config.enabled).toBe(true);
    expect(config.requireExistingMapping).toBe(true);
    let providerReads = 0;
    let local: {
      id: string;
      name: string;
      email: string;
      role: string;
      banned: boolean;
      enabled: boolean;
      emailVerified: boolean;
    } | null = null;
    let sessionActive = true;
    const verify = createConsoleVerifier(
      { ...config, issuer: config.issuer!, serviceKey: config.serviceKey! },
      {
        verify: async () => {
          providerReads++;
          return {
            iss: evidence.external.issuer,
            sub: evidence.external.subject,
            sid: evidence.external.sessionID,
            azp: config.authorizedParties[0]!,
            aud: config.audience,
            exp: Math.floor(Date.now() / 1000) + 60,
          };
        },
        session: async () => ({
          id: evidence.external.sessionID,
          userId: evidence.external.subject,
          status: sessionActive ? "active" : "revoked",
          expireAt: Date.now() + 60_000,
        }),
        user: async () => ({
          id: evidence.external.subject,
          banned: false,
          locked: false,
          primaryEmailAddressId: "email",
          emailAddresses: [
            {
              id: "email",
              emailAddress: user.email,
              verification: { status: "verified" },
            },
          ],
        }),
        identity: async (issuer, subject) => {
          expect(issuer).toBe(evidence.external.issuer);
          expect(subject).toBe(evidence.external.subject);
          return local;
        },
        organization: async () => null,
        memberships: async () => [],
      },
    );
    const request = (service = true) =>
      new Request("https://api.example.test/v1/console", {
        headers: {
          authorization: "Bearer valid-main-token",
          ...(service ? { "x-webdock-service-key": "s".repeat(32) } : {}),
        },
      });
    await expect(verify(request(false))).rejects.toThrow();
    expect(providerReads).toBe(0);
    // A valid external identity with the same email cannot create or recover a mapping.
    await expect(verify(request())).rejects.toThrow(
      "provision your Clerk identity mapping",
    );
    local = { ...user, enabled: true };
    expect((await verify(request())).userID).toBe(user.id);
    local.enabled = false;
    await expect(verify(request())).rejects.toThrow();
    local.enabled = true;
    sessionActive = false;
    await expect(verify(request())).rejects.toThrow();
    // Deleting the delegated mapping never falls back to the compatibility mapping above.
    sessionActive = true;
    local = null;
    await expect(verify(request())).rejects.toThrow(
      "provision your Clerk identity mapping",
    );
  }
});

test("bridge safe role and security flags follow live Clerk without local operator grants", async () => {
  for (const [localRole, providerRole] of [
    ["operator", "user"],
    ["user", "operator"],
  ] as const) {
    const projected = await fixture({
      local: {
        ...user,
        role: localRole,
        emailVerified: false,
        twoFactorEnabled: false,
      },
      external: { ...evidence, user: { ...evidence.user, role: providerRole } },
    }).bridge(new Headers({ authorization: "Bearer verified.token" }));
    expect(projected.safe.user.role).toBe(providerRole);
    expect(projected.safe.operator).toBe(providerRole === "operator");
    expect(projected.safe.user.emailVerified).toBe(true);
    expect(projected.safe.user.twoFactorEnabled).toBe(true);
  }
});
