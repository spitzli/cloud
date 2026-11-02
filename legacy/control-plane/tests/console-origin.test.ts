import { describe, expect, test } from "bun:test";
import { handleConsoleOrigin } from "../src/console-origin";
import { readConfig } from "../src/config";

const studioOrigin = "https://studio.webdock.dev";
const consoleOrigin = "https://console.webdock.dev";
const config = { studioOrigin, consoleOrigin };

describe("public Console origin", () => {
  test("maps verified Console requests without changing body, cookies or client identity", async () => {
    const response = await handleConsoleOrigin(new Request(`${consoleOrigin}/api/sso/logout`, {
      method: "POST", body: "exact=bytes%2B", headers: {
        origin: consoleOrigin, cookie: "__Host-webdock-sso=session", "x-webdock-client-ip": "192.0.2.1",
      },
    }), config, async request => {
      expect(request.url).toBe(`${studioOrigin}/api/sso/logout`);
      expect(request.headers.get("host")).toBe("studio.webdock.dev");
      expect(request.headers.get("origin")).toBe(studioOrigin);
      expect(request.headers.get("cookie")).toBe("__Host-webdock-sso=session");
      expect(request.headers.get("x-webdock-client-ip")).toBe("192.0.2.1");
      expect(await request.text()).toBe("exact=bytes%2B");
      const headers = new Headers({location: `${studioOrigin}/login`});
      headers.append("set-cookie", "session=; Path=/; Max-Age=0; Secure; HttpOnly");
      headers.append("set-cookie", "flow=; Path=/; Max-Age=0; Secure; HttpOnly");
      return new Response(null, {status: 303, headers});
    });
    expect(response.headers.get("location")).toBe(`${consoleOrigin}/login`);
    expect(response.headers.getSetCookie()).toHaveLength(2);
  });

  test("never turns foreign or legacy browser origins into same-origin requests", async () => {
    for (const origin of ["https://evil.example", studioOrigin, "null"]) {
      const response = await handleConsoleOrigin(new Request(`${consoleOrigin}/api/sso/logout`, {
        method: "POST", headers: { origin },
      }), config, () => { throw Error("must not reach session handler"); });
      expect(response.status).toBe(403);
    }
  });

  test("forwards registered browser callbacks to Console with their exact query", async () => {
    for (const path of ["/api/sso/login", "/api/sso/callback", "/api/github/callback", "/api/vercel/callback", "/api/hosting/git/callback", "/api/hosting/vercel/callback"]) {
      const response = await handleConsoleOrigin(new Request(`${studioOrigin}${path}?state=a%2Bb&code=example`), config,
        () => { throw Error("must redirect before consuming callback"); });
      expect(response.status).toBe(307);
      expect(response.headers.get("location")).toBe(`${consoleOrigin}${path}?state=a%2Bb&code=example`);
      expect(response.headers.get("cache-control")).toBe("no-store");
      expect(response.headers.get("referrer-policy")).toBe("no-referrer");
    }
  });

  test("leaves issuer redirects, legacy bearer APIs and webhook bodies untouched", async () => {
    const destination = `https://auth.webdock.dev/api/auth/oauth2/authorize?redirect_uri=${encodeURIComponent(studioOrigin + "/api/sso/callback")}`;
    const response = await handleConsoleOrigin(new Request(`${consoleOrigin}/api/sso/login`), config,
      () => new Response(null, {status: 302, headers: {location: destination}}));
    expect(response.headers.get("location")).toBe(destination);
    for (const path of ["/api/mcp", "/api/vercel/events", "/api/hosting/apps"]) {
      const request = new Request(`${studioOrigin}${path}`, {method: "POST", body: "signed bytes"});
      await handleConsoleOrigin(request, config, async received => {
        expect(received).toBe(request);
        expect(await received.text()).toBe("signed bytes");
        return new Response("ok");
      });
    }
  });

  test("alias is optional and must be a distinct secure origin in production", () => {
    const env = {NODE_ENV: "production", WEBDOCK_API_ORIGIN: "https://api.webdock.dev", BETTER_AUTH_URL: "https://auth.webdock.dev", WEBDOCK_STUDIO_ORIGIN: studioOrigin};
    expect(readConfig(env).consoleOrigin).toBeUndefined();
    expect(readConfig({...env, WEBDOCK_CONSOLE_ORIGIN: consoleOrigin}).consoleOrigin).toBe(consoleOrigin);
    for (const value of [studioOrigin, env.BETTER_AUTH_URL, env.WEBDOCK_API_ORIGIN, "http://localhost:3128", `${consoleOrigin}/path`])
      expect(() => readConfig({...env, WEBDOCK_CONSOLE_ORIGIN: value})).toThrow();
  });
});

test("Clerk Console login entry never dispatches to the legacy SSO handler", async () => {
 const prior=process.env.WEBDOCK_CONSOLE_AUTH_PROVIDER;
 process.env.WEBDOCK_CONSOLE_AUTH_PROVIDER="clerk";
 try {
  for (const method of ["GET","HEAD"]) {
   const response=await handleConsoleOrigin(new Request(`${consoleOrigin}/api/sso/login?returnTo=https://evil.example`,{method}),config,()=>{throw Error("legacy login must not run");});
   expect(response.status).toBe(307);
   expect(response.headers.get("location")).toBe(`${consoleOrigin}/login`);
   expect(response.headers.get("cache-control")).toBe("no-store");
  }
  const callback=await handleConsoleOrigin(new Request(`${consoleOrigin}/api/sso/callback?code=legacy`),config,request=>{
   expect(request.url).toBe(`${studioOrigin}/api/sso/callback?code=legacy`);return new Response("legacy callback preserved");
  });
  expect(await callback.text()).toBe("legacy callback preserved");
 } finally {if(prior===undefined)delete process.env.WEBDOCK_CONSOLE_AUTH_PROVIDER;else process.env.WEBDOCK_CONSOLE_AUTH_PROVIDER=prior;}
});
