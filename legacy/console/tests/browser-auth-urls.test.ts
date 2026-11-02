import test from "node:test";
import assert from "node:assert/strict";
import { browserAuthPageURL } from "../src/lib/browser-auth-config";

test("Clerk login redirects use the configured public Console behind Vercel", () => {
  const env = { NEXT_PUBLIC_SERVER_URL: "https://console.webdock.dev", VERCEL_URL: "deployment.vercel.app" };
  assert.equal(browserAuthPageURL("/login", env), "https://console.webdock.dev/login");
  assert.equal(browserAuthPageURL("/sign-up", env), "https://console.webdock.dev/sign-up");
  assert.equal(browserAuthPageURL("/login", { WEBDOCK_CONSOLE_ORIGIN: "http://localhost:3460" }), "http://localhost:3460/login");
  assert.equal(browserAuthPageURL("/login", {}), "/login");
});
