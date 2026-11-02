import { describe, test, expect } from "bun:test";
import {
  createConsoleVerifier,
  ConsoleAuthError,
  type ConsoleIdentityDependencies,
} from "../src/modules/auth/clerk-identity";
const issuer = "https://fixture.clerk.accounts.dev",
  origin = "https://console.example.test";
function fixture() {
  const state = {
    sub: "user_one",
    sid: "sess_one",
    iss: issuer,
    azp: origin,
    exp: Math.floor(Date.now() / 1000) + 60,
    orgId: "org_one",
  };
  const local = {
    id: "1001",
    name: "Fixture",
    email: "fixture@example.test",
    role: "user",
    banned: false,
    enabled: true,
    emailVerified: true,
  };
  const user = {
    id: "user_one",
    banned: false,
    locked: false,
    privateMetadata: {} as Record<string, unknown>,
    primaryEmailAddressId: "email_one",
    emailAddresses: [{ id: "email_one", verification: { status: "verified" } }],
  };
  const session = {
    id: "sess_one",
    userId: "user_one",
    status: "active",
    expireAt: Date.now() + 60000,
  };
  const member = { organization: { id: "org_one" }, role: "org:admin", permissions: ["org:projects:manage"] };
  const deps: ConsoleIdentityDependencies = {
    verify: async () => state,
    session: async () => session,
    user: async () => user,
    identity: async () => local,
    organization: async () => ({
      id: "2001",
      customerID: "3001",
      active: true,
      enabled: true,
    }),
    memberships: async () => [member],
  };
  const verify = createConsoleVerifier(
    { issuer, authorizedParties: [origin], serviceKey: "s".repeat(32) },
    deps,
  );
  const request = () =>
    new Request("https://api.example.test/v1/console", {
      headers: {
        authorization: "Bearer signed-session",
        "x-webdock-service-key": "s".repeat(32),
      },
    });
  return { state, local, user, session, member, deps, verify, request };
}
describe("Clerk Console trust boundary", () => {
  test("uses immutable mapping and live membership without legacy session", async () => {
    const f = fixture(),
      p = await f.verify(f.request());
    expect(p.provider).toBe("clerk");
    expect(p.userID).toBe("1001");
    expect(p.customerID).toBe("3001");
    expect(p.role).toBe("admin");
    expect(p.operator).toBe(false);
  });
  test("a stale local operator cannot grant global access", async () => {
    const f = fixture();
    f.local.role = "operator";
    f.state.orgId = "";
    const p = await f.verify(f.request());
    expect(p.operator).toBe(false);
    expect(p.customerID).toBeNull();
  });
  for (const field of ["iss", "azp", "sub", "sid"] as const)
    test(`rejects wrong ${field}`, async () => {
      const f = fixture();
      f.state[field] = "wrong";
      await expect(f.verify(f.request())).rejects.toBeInstanceOf(
        ConsoleAuthError,
      );
    });
  test("rejects expired JWT even if live session remains active", async () => {
    const f = fixture();
    f.state.exp = 1;
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 401 });
  });
  test("requires service credential before provider access", async () => {
    const f = fixture();
    let calls = 0;
    f.deps.verify = async () => {
      calls++;
      return f.state;
    };
    await expect(
      f.verify(new Request("https://api.example.test")),
    ).rejects.toMatchObject({ status: 401 });
    expect(calls).toBe(0);
  });
  test("never maps an unknown user by email", async () => {
    const f = fixture();
    f.deps.identity = async () => null;
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  });
  test("rejects disabled local mapping and suspended account", async () => {
    for (const field of ["enabled", "banned"] as const) {
      const f = fixture();
      f.local[field] = field === "banned";
      await expect(f.verify(f.request())).rejects.toMatchObject({
        status: 403,
      });
    }
  });
  test("requires verified primary email", async () => {
    const f = fixture();
    f.user.emailAddresses[0].verification.status = "unverified";
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  });
  test("revoked session denies next request", async () => {
    const f = fixture();
    await f.verify(f.request());
    f.session.status = "revoked";
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 401 });
  });
  test("removed member denies next request despite stale JWT", async () => {
    const f = fixture();
    await f.verify(f.request());
    f.deps.memberships = async () => [];
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  });
  test("inactive customer and unsupported role deny", async () => {
    const f = fixture();
    f.member.role = "operator";
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
    f.member.role = "org:admin";
    f.deps.organization = async () => ({
      id: "2001",
      customerID: "3001",
      active: false,
      enabled: true,
    });
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  });
  test("provider outages are availability errors, with no fallback", async () => {
    const f = fixture();
    f.deps.session = async () => {
      throw Error("secret provider detail");
    };
    await expect(f.verify(f.request())).rejects.toMatchObject({
      status: 503,
      message: "Identity service is temporarily unavailable.",
    });
  });
});
test("projects an invited user only after live verified membership", async () => {
  const f = fixture();
  let projected = false,
    calls = 0;
  Object.assign(f.user.emailAddresses[0], {
    emailAddress: "new@example.invalid",
  });
  f.deps.identity = async () => (projected ? f.local : null);
  f.deps.projectIdentity = async () => {
    calls++;
    projected = true;
  };
  f.deps.memberships = async () => [];
  await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  expect(calls).toBe(0);
  f.deps.memberships = async () => [f.member];
  expect((await f.verify(f.request())).userID).toBe(f.local.id);
  expect(calls).toBe(1);
});

test("live permissions override stale token grants even when the role does not change", async () => {
 const f=fixture();Object.assign(f.state,{permissions:["org:databases:manage"]});
 expect((await f.verify(f.request())).permissions).toEqual(["org:projects:manage"]);
 f.member.permissions=[];
 const next=await f.verify(f.request());expect(next.role).toBe("admin");expect(next.permissions).toEqual([]);
});
test("only recognized permissions of the active organization enter the proof", async () => {
 const f=fixture();f.member.permissions=["org:projects:manage","org:projects:manage","org:sys_memberships:manage","org:operator"];
 f.deps.memberships=async()=>[{organization:{id:"org_other"},role:"org:admin",permissions:["org:databases:manage"]},f.member];
 expect((await f.verify(f.request())).permissions).toEqual(["org:projects:manage"]);
 f.member.permissions=undefined as unknown as string[];
 expect((await f.verify(f.request())).permissions).toEqual([]);
});

test("explicit configured audience rejects missing or foreign claims before provider session access", async () => {
  const f = fixture();
  let audience: unknown;
  f.deps.verify = async () => ({ ...f.state, aud: audience });
  let sessionReads = 0;
  f.deps.session = async () => { sessionReads++; return f.session; };
  const verify = createConsoleVerifier({ issuer, authorizedParties: [origin], serviceKey: "s".repeat(32), audience: "webdock-console" }, f.deps);
  for (audience of [undefined, "other-app", ["other-app"], 42])
    await expect(verify(f.request())).rejects.toMatchObject({ status: 401 });
  expect(sessionReads).toBe(0);
  audience = ["other-app", "webdock-console"];
  expect((await verify(f.request())).userID).toBe("1001");
  expect(sessionReads).toBe(1);
});


test("organization role changes are applied on the next request with the same token", async () => {
  const f = fixture();
  expect((await f.verify(f.request())).role).toBe("admin");
  f.member.role = "org:member";
  expect((await f.verify(f.request())).role).toBe("member");
  f.member.role = "org:admin";
  expect((await f.verify(f.request())).role).toBe("admin");
});

test("missing or disabled organization mapping denies an otherwise verified member", async () => {
  for (const mapping of [null, { id: "2001", customerID: "3001", active: true, enabled: false }]) {
    const f = fixture();
    f.deps.organization = async () => mapping;
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  }
});


test("live Clerk metadata grants and revokes operator access independently of local roles", async () => {
  const f = fixture();
  f.user.privateMetadata.webdockRole = "operator";
  for (const role of ["user", "operator", "legacy-role"]) {
    f.local.role = role;
    expect((await f.verify(f.request())).operator).toBe(true);
  }
  delete f.user.privateMetadata.webdockRole;
  expect((await f.verify(f.request())).operator).toBe(false);
});

test("unknown private roles and spoofed public, unsafe or token metadata cannot grant operator", async () => {
  const f = fixture();
  f.local.role = "operator";
  Object.assign(f.user, { unsafeMetadata: { webdockRole: "operator" }, publicMetadata: { webdockRole: "operator" } });
  Object.assign(f.state, { privateMetadata: { webdockRole: "operator" }, role: "operator" });
  for (const role of [undefined, null, "admin", "Operator", true, ["operator"], { role: "operator" }]) {
    f.user.privateMetadata.webdockRole = role;
    expect((await f.verify(f.request())).operator).toBe(false);
  }
});

test("custom organization roles retain explicit permissions without becoming admin or operator", async () => {
  const f = fixture();
  f.member.role = "org:project_manager";
  f.member.permissions = ["org:projects:manage", "org:deployments:create", "org:unknown:manage"];
  expect(await f.verify(f.request())).toMatchObject({
    role: "member", operator: false, permissions: ["org:projects:manage", "org:deployments:create"],
  });
  f.member.role = "org:operator";
  expect(await f.verify(f.request())).toMatchObject({ role: "member", operator: false });
  for (const role of ["", "admin", "org:", "org:admin:extra", "org:admin "]) {
    f.member.role = role;
    await expect(f.verify(f.request())).rejects.toMatchObject({ status: 403 });
  }
});
