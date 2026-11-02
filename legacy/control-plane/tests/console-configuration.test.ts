import { expect, test } from "bun:test";
import { consoleAuthConfiguration } from "../src/modules/auth/console-configuration";
import { consoleBrowserRequest } from "../src/modules/portal/console-browser";
const configuration = {
  provider: "clerk",
  issuer: "https://fixture.clerk.accounts.dev",
  authorizedParties: ["https://console.example.test"],
  audience: "webdock-console",
};
test("JSON Clerk delegation requires an explicit service key and preserves provider boundaries", () => {
  expect(consoleAuthConfiguration({}).enabled).toBe(false);
  expect(
    consoleAuthConfiguration({ WEBDOCK_CONSOLE_AUTH_PROVIDER: "clerk" })
      .enabled,
  ).toBe(true);
  const selected = consoleAuthConfiguration({
    WEBDOCK_AUTH_PROVIDER: JSON.stringify(configuration),
  });
  expect(selected.enabled).toBe(false);
  expect(
    consoleAuthConfiguration({
      WEBDOCK_AUTH_PROVIDER: JSON.stringify(configuration),
      WEBDOCK_CONSOLE_AUTH_PROVIDER: "clerk",
    }).enabled,
  ).toBe(false);
  expect(consoleAuthConfiguration({
    WEBDOCK_AUTH_PROVIDER: JSON.stringify(configuration),
    WEBDOCK_CONSOLE_SERVICE_KEY: "s".repeat(32),
  }).enabled).toBe(true);
  expect(selected.audience).toBe("webdock-console");
  expect(selected.requireExistingMapping).toBe(true);
  expect(() =>
    consoleAuthConfiguration({
      WEBDOCK_AUTH_PROVIDER: JSON.stringify(configuration),
      CLERK_ISSUER: "https://other.clerk.accounts.dev",
    }),
  ).toThrow();
  expect(() =>
    consoleAuthConfiguration({
      WEBDOCK_AUTH_PROVIDER: JSON.stringify({
        ...configuration,
        audience: undefined,
      }),
    }),
  ).toThrow();
});
test("Console cookie protocol denies foreign origin and caller delegation before provider access", async () => {
  let calls = 0;
  const origin = "https://console.example.test";
  const dispatch = async () => {
    calls++;
    return Response.json({ ok: true });
  };
  for (const [headers, status] of [
    [
      {
        origin: "https://attacker.example",
        "content-type": "application/json",
      },
      403,
    ],
    [
      {
        origin,
        authorization: "Bearer injected",
        "content-type": "application/json",
      },
      401,
    ],
    [
      {
        origin,
        "x-webdock-service-key": "injected",
        "content-type": "application/json",
      },
      401,
    ],
    [{ origin, "content-type": "text/plain" }, 415],
  ] as const)
    expect(
      (
        await consoleBrowserRequest(
          new Request(origin, { method: "POST", headers }),
          origin,
          dispatch,
        )
      ).status,
    ).toBe(status);
  expect(calls).toBe(0);
  expect(
    (
      await consoleBrowserRequest(
        new Request(origin, {
          method: "POST",
          headers: {
            origin,
            cookie: "__session=verified-by-provider",
            "content-type": "application/json",
          },
        }),
        origin,
        dispatch,
      )
    ).status,
  ).toBe(200);
  expect(calls).toBe(1);
});
