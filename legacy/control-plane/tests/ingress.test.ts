import { expect, test } from "bun:test";
import { normalizeIngress } from "../src/ingress";
const policy = {
  origins: [
    "https://auth.webdock.dev",
    "https://api.webdock.dev",
    "https://studio.webdock.dev",
  ],
  trustedProxies: ["10.20.0.0/16"],
  secret: "fixture-ingress-key-at-least-32-characters",
  production: true,
};
const request = (extra: Record<string, string> = {}) =>
  new Request("http://internal:3130/api/auth/get-session", {
    headers: {
      "x-forwarded-host": "auth.webdock.dev",
      "x-forwarded-proto": "https",
      "x-forwarded-for": "203.0.113.25",
      "x-webdock-ingress-token": policy.secret,
      ...extra,
    },
  });
test("forwarded identity requires both trusted peer and secret", () => {
  expect(normalizeIngress(request(), "192.0.2.1", policy)).toBeInstanceOf(
    Response,
  );
  expect(
    normalizeIngress(
      request({ "x-webdock-ingress-token": "wrong" }),
      "10.20.1.2",
      policy,
    ),
  ).toBeInstanceOf(Response);
  expect(
    normalizeIngress(
      request({ "x-forwarded-host": "evil.invalid" }),
      "10.20.1.2",
      policy,
    ),
  ).toBeInstanceOf(Response);
});
test("verified ingress reconstructs canonical HTTPS and overwrites caller IP metadata", () => {
  const result = normalizeIngress(
    request({ "x-webdock-client-ip": "forged" }),
    "10.20.1.2",
    policy,
  );
  expect(result).toBeInstanceOf(Request);
  const req = result as Request;
  expect(req.url).toBe("https://auth.webdock.dev/api/auth/get-session");
  expect(req.headers.get("x-webdock-client-ip")).toBe("203.0.113.25");
  expect(req.headers.has("x-webdock-ingress-token")).toBe(false);
  expect(req.headers.has("x-forwarded-host")).toBe(false);
});
test("forwarded body bytes are not parsed or rewritten", async () => {
  const body = ' { "a": "ä" }\n';
  const source = request();
  const input = new Request(source, { method: "POST", body });
  const result = normalizeIngress(input, "10.20.1.2", policy) as Request;
  expect(await result.text()).toBe(body);
});
