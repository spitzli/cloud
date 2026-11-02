import { test, expect } from "bun:test";
import { configureFullInstallation } from "../src/full-installation";
configureFullInstallation();
import {
  createIntegrationBrowserRunner,
  clerkIntegrationProof,
} from "../src/protocols/integration-browser";
import { installationRegistry, moduleForRequest } from "../src/installation";
const origin = "https://studio.example.test";
function fixture(
  state: "enabled" | "disabled" | "draining" = "enabled",
  provider: "clerk" | "better-auth" = "clerk",
  valid = true,
  hosting = true,
) {
  const calls: string[] = [];
  const registry = installationRegistry({
    WEBDOCK_MODULES: JSON.stringify({
      configVersion: 1,
      modules: { hosting: { state } },
    }),
  });
  const run = createIntegrationBrowserRunner({
    provider,
    origin,
    ...(hosting
      ? {
          requireAdmission: (intent: "access" | "provision") =>
            registry.require("hosting", intent),
        }
      : {}),
    clerk: async (headers) => {
      calls.push("clerk:" + headers.get("authorization"));
      return valid
        ? {
            headers: new Headers({ cookie: "native=signed-internal" }),
            reference: { subject: "123", sessionID: "456" },
          }
        : null;
    },
    legacy: async (headers) => {
      calls.push("legacy");
      return {
        headers,
        reference: { subject: "123", sessionID: "456" },
        accessToken: "legacy-oauth",
      };
    },
    withIdentity: async (identity, browser, kind, handler) => {
      calls.push("scope:" + kind);
      expect(identity.reference).toEqual({ subject: "123", sessionID: "456" });
      if (kind === "clerk") {
        expect(identity.headers.get("cookie")).toBe("native=signed-internal");
        expect(identity.accessToken).toBeUndefined();
        expect(browser.get("cookie")).toContain(
          "webdock-github-flow=signed-state",
        );
      }
      return handler();
    },
  });
  return { calls, run };
}
function request(
  path = "/api/github/connect",
  options: {
    origin?: string;
    method?: string;
    cookie?: string;
    authorization?: string;
  } = {},
) {
  const headers = new Headers({
    origin: options.origin ?? origin,
    cookie:
      options.cookie ??
      "__session=header.payload.signature; webdock-github-flow=signed-state; webdock-sso=legacy",
  });
  if (options.authorization !== undefined)
    headers.set("authorization", options.authorization);
  return new Request(origin + path, {
    method: options.method ?? "POST",
    headers,
  });
}
test("selected Clerk integration flow verifies signed cookie proof, establishes native scope and preserves provider flow state", async () => {
  const f = fixture();
  const response = await f.run(
    request("/api/github/callback", { method: "GET" }),
    "provision",
    async (request) => {
      expect(request.headers.get("cookie")).toContain(
        "webdock-github-flow=signed-state",
      );
      return new Response(null, {
        status: 303,
        headers: { "set-cookie": "webdock-github=encrypted-provider-session" },
      });
    },
  );
  expect(response.status).toBe(303);
  expect(response.headers.get("set-cookie")).toContain(
    "encrypted-provider-session",
  );
  expect(f.calls).toEqual([
    "clerk:Bearer header.payload.signature",
    "scope:clerk",
  ]);
});
test("Clerk selection never authenticates through native or legacy cookies, nor falls back after rejected bearer", async () => {
  for (const input of [
    request(undefined, {
      cookie: "webdock-sso=valid; webdock-auth.session_token=valid",
    }),
    request(undefined, { authorization: "Basic legacy" }),
    request(undefined, {
      cookie: "__session=one.two.three; __session=other.two.three",
    }),
  ]) {
    const f = fixture();
    expect(
      (await f.run(input, "provision", async () => new Response())).status,
    ).toBe(401);
    expect(f.calls).toEqual([]);
  }
  const f = fixture("enabled", "clerk", false);
  expect(
    (
      await f.run(
        request(undefined, { authorization: "Bearer expired.token.value" }),
        "provision",
        async () => new Response(),
      )
    ).status,
  ).toBe(401);
  expect(f.calls).toEqual(["clerk:Bearer expired.token.value"]);
});
test("POST integration CSRF rejects foreign and missing origins before any identity/provider call", async () => {
  for (const provider of ["clerk", "better-auth"] as const)
    for (const supplied of ["https://evil.invalid", ""]) {
      const f = fixture("enabled", provider);
      expect(
        (
          await f.run(
            request("/api/vercel/disconnect", { origin: supplied }),
            "access",
            async () => new Response(),
          )
        ).status,
      ).toBe(403);
      expect(f.calls).toEqual([]);
    }
});
test("disabled Hosting rejects product integration work and draining rejects enrollment before provider verification", async () => {
  for (const state of ["disabled", "draining"] as const) {
    for (const action of ["connect", "callback", "select"]) {
      const f = fixture(state);
      expect(
        (
          await f.run(
            request("/api/hosting/git/" + action, {
              method: action === "callback" ? "GET" : "POST",
            }),
            "provision",
            async () => new Response(),
          )
        ).status,
      ).toBe(503);
      expect(f.calls).toEqual([]);
    }
  }
  const disabled = fixture("disabled");
  expect(
    (
      await disabled.run(
        request("/api/vercel/disconnect"),
        "access",
        async () => new Response(),
      )
    ).status,
  ).toBe(503);
  expect(disabled.calls).toEqual([]);
  const draining = fixture("draining");
  expect(
    (
      await draining.run(
        request("/api/vercel/disconnect"),
        "access",
        async () => new Response(null, { status: 204 }),
      )
    ).status,
  ).toBe(204);
});
test("generic GitHub paths belong to Core while project linking and Vercel retain Hosting gates", async () => {
  for (const path of [
    "/api/github/connect",
    "/api/github/callback",
    "/api/github/disconnect",
  ])
    expect(moduleForRequest(path)).toBeUndefined();
  for (const path of [
    "/api/github/select",
    "/api/vercel/connect",
    "/api/vercel/callback",
    "/api/vercel/disconnect",
    "/api/vercel/events",
  ])
    expect(moduleForRequest(path)).toBe("hosting");
  const f = fixture("enabled", "better-auth");
  expect(
    (
      await f.run(
        request(),
        "access",
        async () => new Response(null, { status: 204 }),
      )
    ).status,
  ).toBe(204);
  expect(f.calls).toEqual(["legacy", "scope:legacy"]);
  expect(
    clerkIntegrationProof(
      new Headers({ cookie: "__session=not-a-signed-token" }),
    ),
  ).toBeNull();
});

test("Core GitHub authentication works with Hosting disabled and never skips identity verification", async () => {
  for (const provider of ["clerk", "better-auth"] as const) {
    const f = fixture("disabled", provider, true, false);
    expect(
      (
        await f.run(
          request(),
          "provision",
          async () => new Response(null, { status: 204 }),
        )
      ).status,
    ).toBe(204);
    expect(f.calls).toContain(
      `scope:${provider === "clerk" ? "clerk" : "legacy"}`,
    );
  }
  const revoked = fixture("disabled", "clerk", false, false);
  expect(
    (await revoked.run(request(), "provision", async () => new Response()))
      .status,
  ).toBe(401);
});
