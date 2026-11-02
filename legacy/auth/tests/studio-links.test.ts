import assert from "node:assert/strict";
import test from "node:test";
import { businessURL, legacyStudioRedirect, studioURL } from "../src/lib/studio-links";

const enabled = { STUDIO_UI_ENABLED: "true", NODE_ENV: "production" as const };

test("Studio redirects preserve business paths and queries only for GET and HEAD after cutover", () => {
  for (const path of ["/admin", "/admin/plans", "/tenants", "/tenants/123/mail/keys", "/offers/private-token", "/people", "/sites"]) {
    const url = new URL(`${path}?notice=saved&returnTo=%2Ftenants`, "https://auth.webdock.dev");
    for (const method of ["GET", "HEAD"]) {
      assert.equal(legacyStudioRedirect(url, method, enabled), `https://studio.webdock.dev${url.pathname}${url.search}`);
      assert.equal(legacyStudioRedirect(url, method, {}), null);
    }
    for (const method of ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) assert.equal(legacyStudioRedirect(url, method, enabled), null);
  }
  for (const path of ["/", "/account", "/sign-in", "/two-factor", "/connections", "/invitation", "/api/auth/sign-in", "/api/studio/tenants", "/_next/static/main.js", "/administrator", "/tenants-other", "/offers", "/people/other", "/sites-extra"]) {
    assert.equal(legacyStudioRedirect(new URL(path, "https://auth.webdock.dev"), "GET", enabled), null);
  }
});

test("Studio links use a configured canonical origin and keep business links local before cutover", () => {
  assert.equal(businessURL("/tenants", {}), "/tenants");
  assert.equal(businessURL("/tenants", { STUDIO_UI_ENABLED: "false" }), "/tenants");
  assert.equal(businessURL("/tenants", enabled), "https://studio.webdock.dev/tenants");
  assert.equal(studioURL("/sites", { WEBDOCK_STUDIO_ORIGIN: "https://studio.example.test/" }), "https://studio.example.test/sites");
  assert.equal(studioURL("/tenants", { NODE_ENV: "development", WEBDOCK_STUDIO_ORIGIN: "http://localhost:3120" }), "http://localhost:3120/tenants");
  for (const origin of ["http://studio.example.test", "javascript:alert(1)", "https://user:secret@studio.example.test", "https://studio.example.test/nested", "https://studio.example.test/?evil=1", "https://studio.example.test/#fragment", "http://localhost:3120"]) {
    assert.throws(() => studioURL("/tenants", { ...enabled, WEBDOCK_STUDIO_ORIGIN: origin }));
  }
  for (const path of ["https://evil.test", "//evil.test", "/\\evil.test"]) assert.throws(() => studioURL(path, enabled));
});
