import { test } from "node:test";
import assert from "node:assert/strict";
import {
  createClerkAdapter,
  type ClerkConfiguration,
  type ClerkBackend,
  type ClerkClaims,
  type ClerkMappings,
} from "./index";
import { readAuthProviderConfiguration } from "@spitzli/auth-configuration";
import type { DatabaseReader, IdentityDirectory } from "@spitzli/core/identity";
const now = 2_000_000;
const config = {
  provider: "clerk" as const,
  issuer: "https://identity.example",
  audience: "webdock",
  authorizedParties: ["https://console.example"],
};
const reference = { subject: "123456789", sessionID: "local-session" };
const external = {
  issuer: config.issuer,
  subject: "user_one",
  sessionID: "sess_one",
};
const claims: ClerkClaims = {
  iss: config.issuer,
  sub: external.subject,
  sid: external.sessionID,
  azp: config.authorizedParties[0],
  aud: config.audience,
  exp: 2100,
  nbf: 1900,
  iat: 1900,
  fva: [0, -1],
};
const localIdentity = {
  subject: reference.subject,
  role: "user",
  banned: false,
  emailVerified: true,
  mustChangePassword: false,
  twoFactorEnabled: false,
};
function fixture(
  overrides: {
    config?: ClerkConfiguration;
    claims?: ClerkClaims;
    backend?: Partial<ClerkBackend>;
    mappings?: Partial<ClerkMappings>;
    local?: Partial<IdentityDirectory>;
  } = {},
) {
  const backend: ClerkBackend = {
    verifyToken: async () => ({ ...claims, ...overrides.claims }),
    session: async () => ({
      id: external.sessionID,
      userId: external.subject,
      status: "active",
      expireAt: now + 500000,
      actor: null,
    }),
    user: async () => ({
      id: external.subject,
      role: "user",
      banned: false,
      locked: false,
      twoFactorEnabled: true,
      emailVerified: true,
    }),
    membership: async () => ({
      id: "clerk-member",
      organizationID: "org_one",
      subject: external.subject,
      role: "org:admin",
      permissions: ["org:hosting:read"],
    }),
    ...overrides.backend,
  };
  const mappings: ClerkMappings = {
    localSubject: async () => reference.subject,
    localSession: async () => reference,
    externalSubject: async () => external,
    externalSession: async () => external,
    externalOrganization: async () => ({
      issuer: config.issuer,
      organizationID: "org_one",
    }),
    ...overrides.mappings,
  };
  const local: IdentityDirectory = {
    subject: async () => localIdentity,
    session: async () => ({
      ...localIdentity,
      sessionID: reference.sessionID,
      preview: false,
    }),
    membership: async () => ({ memberID: "member-1", role: "owner" }),
    ...overrides.local,
  };
  return createClerkAdapter({
    config: overrides.config ?? config,
    backend,
    mappings,
    local,
    now: () => now,
  });
}
const db = {} as DatabaseReader;
test("strict provider config keeps absent installs compatible and rejects unknown fields/origins", () => {
  assert.deepEqual(readAuthProviderConfiguration(undefined), {
    provider: "better-auth",
  });
  assert.deepEqual(
    readAuthProviderConfiguration(JSON.stringify(config)),
    config,
  );
  for (const value of [
    "",
    "{}",
    '{"provider":"unknown"}',
    JSON.stringify({ ...config, issuer: "http://identity.example" }),
    JSON.stringify({ ...config, audience: "" }),
    JSON.stringify({ ...config, authorizedParties: [] }),
    JSON.stringify({ ...config, secretKey: "do-not-accept" }),
  ])
    assert.throws(() => readAuthProviderConfiguration(value));
});
test("maps only explicit local subject/session and returns real MFA facts, never a token", async () => {
  const adapter = fixture();
  assert.deepEqual(
    await adapter.authenticator.authenticate(
      new Headers({ authorization: "Bearer signed-token" }),
    ),
    reference,
  );
  const evidence = await adapter.verify("signed-token");
  assert.deepEqual(evidence?.factorVerificationAge, [0, -1]);
  assert.equal("token" in evidence!, false);
  assert.equal(
    (await fixture({ claims: { fva: undefined } }).verify("signed-token"))
      ?.factorVerificationAge,
    null,
  );
  assert.equal(
    await adapter.authenticator.authenticate(
      new Headers({ cookie: "__session=unverified" }),
    ),
    null,
  );
});
test("rejects issuer, audience, party, expiration, future, malformed factor and impersonation claims", async () => {
  for (const bad of [
    { iss: "https://other.example" },
    { aud: "other" },
    { azp: "https://evil.example" },
    { exp: 1999 },
    { nbf: 2100 },
    { iat: 2100 },
    { exp: Infinity },
    { fva: [0, -2] },
    { act: { sub: "user_admin" } },
    { sid: "" },
  ])
    assert.equal(
      await fixture({ claims: bad }).verify("signed-token"),
      null,
      JSON.stringify(bad),
    );
});
test("signature failure, revoked/expired/mismatched provider sessions and provider outage deny", async () => {
  assert.equal(
    await fixture({
      backend: {
        verifyToken: async () => {
          throw Error("signature");
        },
      },
    }).verify("bad"),
    null,
  );
  for (const bad of [
    { status: "revoked" },
    { expireAt: now - 1 },
    { userId: "user_other" },
    { id: "sess_other" },
    { actor: { sub: "user_admin" } },
  ]) {
    assert.equal(
      await fixture({
        backend: {
          session: async () => ({
            id: external.sessionID,
            userId: external.subject,
            status: "active",
            expireAt: now + 1,
            actor: null,
            ...bad,
          }),
        },
      }).verify("token"),
      null,
    );
  }
  assert.equal(
    await fixture({
      backend: {
        session: async () => {
          throw Error("outage");
        },
      },
    }).identity.session(db, reference),
    null,
  );
});
test("missing mappings and local session subject mismatch deny", async () => {
  for (const mappings of [
    { localSubject: async () => null },
    { localSession: async () => null },
    { localSession: async () => ({ ...reference, subject: "someone-else" }) },
  ])
    assert.equal(await fixture({ mappings }).verify("token"), null);
  assert.equal(
    await fixture({
      local: {
        session: async () => ({
          ...localIdentity,
          subject: "other",
          sessionID: reference.sessionID,
          preview: false,
        }),
      },
    }).identity.session(db, reference),
    null,
  );
  assert.equal(
    await fixture({
      mappings: {
        externalSession: async () => ({
          ...external,
          issuer: "https://other.example",
        }),
      },
    }).identity.session(db, reference),
    null,
  );
});
test("rechecks revocation each call and retains local bans, setup and preview policy", async () => {
  let revoked = false;
  const adapter = fixture({
    backend: {
      session: async () => ({
        id: external.sessionID,
        userId: external.subject,
        status: revoked ? "revoked" : "active",
        expireAt: now + 1,
        actor: null,
      }),
    },
    local: {
      session: async () => ({
        ...localIdentity,
        banned: true,
        mustChangePassword: true,
        preview: true,
        sessionID: reference.sessionID,
      }),
    },
  });
  const first = await adapter.identity.session(db, reference);
  assert.equal(first?.banned, true);
  assert.equal(first?.mustChangePassword, true);
  assert.equal(first?.preview, true);
  assert.equal(first?.twoFactorEnabled, true); // Clerk owns security enrollment when selected.
  revoked = true;
  assert.equal(await adapter.identity.session(db, reference), null);
});
test("queued subject rechecks provider ban and memberships come from Clerk", async () => {
  assert.equal(
    await fixture({
      backend: {
        user: async () => ({
          id: external.subject,
          role: "user",
          banned: true,
          locked: false,
          twoFactorEnabled: false,
          emailVerified: true,
        }),
      },
    }).identity.subject(db, reference.subject),
    null,
  );
  assert.deepEqual(
    await fixture().identity.membership(db, reference.subject, "customer"),
    {
      memberID: "clerk-member",
      role: "admin",
      permissions: ["org:hosting:read"],
    },
  );
});
test("verified external evidence supports explicit enrollment without auto-linking", async () => {
  const adapter = fixture({
    mappings: {
      localSubject: async () => null,
      localSession: async () => null,
    },
  });
  assert.equal(
    (await adapter.verifyExternal("token"))?.user.id,
    external.subject,
  );
  assert.equal(await adapter.verify("token"), null);
});

test("configured installations default to Clerk while an absent setting preserves legacy", () => {
  const { provider, ...settings } = config;
  assert.equal(
    readAuthProviderConfiguration(JSON.stringify(settings)).provider,
    "clerk",
  );
  assert.equal(
    readAuthProviderConfiguration(undefined).provider,
    "better-auth",
  );
});

test("native session mode is explicit and keeps issuer, party and live session checks", async () => {
  const { audience, ...base } = config;
  const native = { ...base, tokenMode: "session" as const };
  assert.deepEqual(
    readAuthProviderConfiguration(JSON.stringify(native)),
    native,
  );
  assert.throws(
    () => readAuthProviderConfiguration(JSON.stringify(base)),
    /audience/,
  );
  for (const invalid of [
    { ...config, tokenMode: "session" },
    { ...base, tokenMode: "unknown" },
  ])
    assert.throws(() => readAuthProviderConfiguration(JSON.stringify(invalid)));
  assert.equal(
    await fixture({ claims: { aud: undefined } }).verify("token"),
    null,
  );
  assert.deepEqual(
    (
      await fixture({ config: native, claims: { aud: undefined } }).verify(
        "token",
      )
    )?.reference,
    reference,
  );
  for (const bad of [
    { iss: "https://other.example" },
    { azp: "https://evil.example" },
    { azp: undefined },
    { exp: 1999 },
    { sub: "m2m_test" },
    { sid: undefined },
  ])
    assert.equal(
      await fixture({
        config: native,
        claims: { aud: undefined, ...bad },
      }).verify("token"),
      null,
    );
  for (const status of ["revoked", "expired"])
    assert.equal(
      await fixture({
        config: native,
        claims: { aud: undefined },
        backend: {
          session: async () => ({
            id: external.sessionID,
            userId: external.subject,
            status,
            expireAt: now + 1,
            actor: null,
          }),
        },
      }).verify("token"),
      null,
    );
  assert.equal(
    await fixture({
      config: native,
      claims: { aud: undefined },
      mappings: { localSubject: async () => null },
    }).verify("token"),
    null,
  );
});

// Local persisted roles are never authority in a selected Clerk installation.
test("session and queued subject roles follow current Clerk grants and revocations", async () => {
  let role: "operator" | "user" = "user";
  const adapter = fixture({
    backend: {
      user: async () => ({
        id: external.subject,
        role,
        banned: false,
        locked: false,
        emailVerified: true,
        twoFactorEnabled: true,
      }),
    },
    local: {
      subject: async () => ({
        ...localIdentity,
        role: "operator",
        operatorAuthorized: true,
        emailVerified: false,
      }),
      session: async () => ({
        ...localIdentity,
        role: "operator",
        operatorAuthorized: true,
        sessionID: reference.sessionID,
        preview: false,
      }),
    },
  });
  for (const current of ["user", "operator", "user"] as const) {
    role = current;
    for (const actual of [
      await adapter.identity.subject(db, reference.subject),
      await adapter.identity.session(db, reference),
    ]) {
      assert.equal(actual?.role, current);
      assert.equal(actual?.operatorAuthorized, false);
      assert.equal(actual?.emailVerified, true);
    }
  }
  assert.equal(
    (
      await fixture({
        backend: {
          user: async () => ({
            id: external.subject,
            role: "operator",
            banned: false,
            locked: false,
            emailVerified: true,
            twoFactorEnabled: true,
          }),
        },
      }).identity.subject(db, reference.subject)
    )?.role,
    "operator",
  );
});
test("Clerk membership requires the exact enabled issuer mapping and live explicit permissions", async () => {
  for (const mappings of [
    { externalOrganization: async () => null },
    {
      externalOrganization: async () => ({
        issuer: "https://other.example",
        organizationID: "org_one",
      }),
    },
    { externalSubject: async () => null },
    { localSubject: async () => "other" },
  ])
    assert.equal(
      await fixture({ mappings }).identity.membership(
        db,
        reference.subject,
        "customer",
      ),
      null,
    );
  for (const member of [
    null,
    {
      id: "member",
      organizationID: "org_other",
      subject: external.subject,
      role: "org:admin",
      permissions: [],
    },
    {
      id: "member",
      organizationID: "org_one",
      subject: "user_other",
      role: "org:admin",
      permissions: [],
    },
    {
      id: "member",
      organizationID: "org_one",
      subject: external.subject,
      role: "operator",
      permissions: [],
    },
  ])
    assert.deepEqual(
      await fixture({
        backend: { membership: async () => member },
      }).identity.membership(db, reference.subject, "customer"),
      { memberID: null, role: null, permissions: [] },
    );
  const custom = fixture({
    backend: {
      membership: async () => ({
        id: "member",
        organizationID: "org_one",
        subject: external.subject,
        role: "org:deployer",
        permissions: ["org:hosting:create"],
      }),
    },
  });
  assert.deepEqual(
    await custom.identity.membership(db, reference.subject, "customer"),
    { memberID: "member", role: "member", permissions: ["org:hosting:create"] },
  );
  assert.equal(
    await fixture({
      backend: {
        membership: async () => {
          throw Error("outage");
        },
      },
    }).identity.membership(db, reference.subject, "customer"),
    null,
  );
});

test("missing provider permissions never re-enable legacy role-derived rights", async () => {
  const adapter = fixture({
    backend: {
      membership: async () => ({
        id: "member",
        organizationID: "org_one",
        subject: external.subject,
        role: "org:admin",
        permissions: undefined as unknown as string[],
      }),
    },
  });
  assert.deepEqual(
    (await adapter.identity.membership(db, reference.subject, "customer"))
      ?.permissions,
    [],
  );
});
