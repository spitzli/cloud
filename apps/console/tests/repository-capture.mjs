// Real Console rendering, synthetic customer data, no production/provider requests.
import { chromium } from "playwright";
import assert from "node:assert/strict";
import { server, previewOrigin } from "./preview-server.ts";
import manifest from "../../../plugins/neon/manifest.json";

const browser = await chromium.launch({ headless: true, chromiumSandbox: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, locale: "en-US", colorScheme: "dark", deviceScaleFactor: 1 });
const page = await context.newPage();
const errors = [];
page.on("pageerror", error => errors.push(error.message));
const at = path => url => url.pathname === path;
try {
  await context.route("**/*", route => {
    const url = new URL(route.request().url());
    assert.equal(url.origin, previewOrigin, "Screenshot must make no external requests");
    if (url.pathname.startsWith("/api/") || url.pathname.startsWith("/v1/")) return route.fulfill({ status: 404, json: { error: "Not implemented by screenshot fixture" } });
    return route.continue();
  });
  const routes = {
    "/console-config.json": { apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] },
    "/v1/auth/session": { data: { user: { id: "2", name: "Alex Demo", email: "alex@example.test", role: "user", emailVerified: true }, session: { id: "synthetic-demo", userId: "2", expiresAt: "2099-01-01T00:00:00Z" } } },
    "/api/console/tenants": [{ id: "101", name: "Spitzli Demo", status: "active", role: "owner", icon: null }],
    "/api/console/tenant/101": { tenant: { name: "Spitzli Demo", icon: null, member_role: "owner" }, canManage: true, members: [{ id: "2", name: "Alex Demo", email: "alex@example.test", role: "owner", mustChangePassword: false }], invitations: [], activity: [] },
    "/api/console/tenant/101/projects": [{ id: "201", name: "Website", status: "active" }],
    "/api/console/plugins": [{ state: "enabled", runtime: "ready", manifest, generation: 1, manifestSha256: "a".repeat(64), launchLinks: [], features: ["connect"], resources: [{ id: "demo-neon", name: "Website database", offering: "database", createdVia: "self-service" }] }],
    "/api/console/customers/101/notifications": { items: [] },
  };
  for (const [path, value] of Object.entries(routes)) await page.route(at(path), route => route.fulfill({ json: value }));
  await page.goto(previewOrigin + "/tenants/101");
  await page.getByRole("heading", { name: "Workspace icon", exact: true }).waitFor();
  await page.getByText("Alex Demo", { exact: true }).last().waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.mouse.move(1420, 980);
  await page.evaluate(() => { if (document.activeElement instanceof HTMLElement) document.activeElement.blur(); window.scrollTo(0, 0); });
  assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  assert.equal(await page.getByRole("link", { name: "Plans", exact: true }).count(), 0);
  assert.ok(!(await page.locator("body").innerText()).includes("postgresql://"));
  assert.deepEqual(errors, []);
  await page.screenshot({ path: "docs/images/repository/console-workspace.png" });
  console.log("Console screenshot captured with synthetic customer data; no production/provider requests");
} finally {
  await context.close();
  await browser.close();
  await server.close();
}
