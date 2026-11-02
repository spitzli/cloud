import { test, expect } from "bun:test";
import { createIntegrationCallbackHandoff } from "../src/protocols/integration-callback-handoff";
import { createIntegrationBrowserRunner } from "../src/protocols/integration-browser";
const origin = "https://studio.example.test",
  consoleOrigin = "https://console.example.test";
function fixture() {
  let now = 1_800_000_000_000;
  const handoff = createIntegrationCallbackHandoff({
    origin,
    consoleOrigin,
    secret: "fixture-secret-at-least-32-characters",
    now: () => now,
  });
  return {
    handoff,
    expire: () => {
      now += 181_000;
    },
  };
}
test("callback refresh carries exact allowlisted query only in an encrypted HttpOnly cookie", () => {
  const { handoff } = fixture();
  for (const path of [
    "/api/github/callback",
    "/api/vercel/callback",
    "/api/hosting/git/callback",
    "/api/hosting/vercel/callback",
  ]) {
    const target = path + "?code=secret-code&state=secret-state%2Bvalue";
    const response = handoff.begin(new Request(origin + target))!;
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe(
      consoleOrigin + "/auth/integration-resume",
    );
    expect(response.headers.get("referrer-policy")).toBe("no-referrer");
    expect(response.headers.get("cache-control")).toBe("no-store");
    const cookie = response.headers.get("set-cookie")!;
    expect(cookie).toContain("HttpOnly");
    expect(cookie).toContain("Secure");
    expect(cookie).not.toContain("secret-code");
    expect(cookie).not.toContain("secret-state");
    expect(handoff.read(new Headers({ cookie: cookie.split(";")[0] }))).toBe(
      target,
    );
  }
});
test("handoff rejects external or arbitrary paths, tampering, duplicate cookies and expiry", () => {
  const f = fixture();
  for (const target of [
    "https://evil.invalid/api/github/callback",
    origin + "/api/github/connect",
    origin + "/api/github/callback/extra",
    origin + "/api/sso/login",
  ])
    expect(f.handoff.begin(new Request(target))).toBeNull();
  const response = f.handoff.begin(
      new Request(origin + "/api/github/callback?code=secret&state=state"),
    )!,
    cookie = response.headers.get("set-cookie")!.split(";")[0];
  expect(
    f.handoff.read(new Headers({ cookie: cookie + "; " + cookie })),
  ).toBeNull();
  expect(
    f.handoff.read(new Headers({ cookie: cookie.slice(0, -4) + "AAAA" })),
  ).toBeNull();
  f.expire();
  expect(f.handoff.read(new Headers({ cookie }))).toBeNull();
});
test("expired callback proof defers all provider side effects, attempts refresh once and never accepts expired proof", async () => {
  const { handoff } = fixture();
  let verified = false,
    effects = 0,
    legacy = 0;
  const runner = createIntegrationBrowserRunner({
    provider: "clerk",
    origin,
    refreshCallback: (request) => handoff.begin(request),
    clerk: async () =>
      verified
        ? {
            headers: new Headers({ cookie: "native=private" }),
            reference: { subject: "123", sessionID: "456" },
          }
        : null,
    legacy: async () => {
      legacy++;
      return null;
    },
    withIdentity: async (_identity, _browser, _kind, run) => run(),
  });
  const request = new Request(
      origin + "/api/github/callback?code=code&state=state",
      { headers: { cookie: "__session=expired.signed.proof" } },
    ),
    provider = async () => {
      effects++;
      return new Response(null, {
        status: 303,
        headers: { location: origin + "/integrations" },
      });
    };
  const refresh = await runner(request, "provision", provider);
  expect(refresh.status).toBe(303);
  expect(effects).toBe(0);
  const refused = await runner(request, "provision", provider, {
    refreshed: true,
  });
  expect(refused.status).toBe(401);
  expect(refused.headers.get("location")).toBeNull();
  expect(effects).toBe(0);
  verified = true;
  const completed = await runner(request, "provision", provider, {
    refreshed: true,
  });
  expect(completed.status).toBe(303);
  expect(effects).toBe(1);
  expect(legacy).toBe(0);
});
