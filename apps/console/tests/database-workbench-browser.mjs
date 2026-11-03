// Controlled local HTTP only: real workbench and HttpTransport; no production services.
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { createServer } from "vite";
import react from "@vitejs/plugin-react";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
async function until(check) { for (let n = 0; n < 500; n++) { if (check()) return; await new Promise(resolve => setTimeout(resolve, 10)); } assert(check(), "Controlled request did not arrive"); }
const cacheDir = await mkdtemp(`${tmpdir()}/webdock-workbench-`);
const server = await createServer({
  configFile: false, root: fileURLToPath(new URL("../", import.meta.url)), cacheDir,
  plugins: [react(), { name: "workbench-http-fixture", configureServer(server) {
    server.middlewares.use((req, res, next) => {
      if (!req.url?.startsWith("/__workbench")) return next();
      void server.transformIndexHtml(req.url, '<!doctype html><html><body><div id="root"></div><script type="module" src="/tests/database-workbench-proof.tsx"></script></body></html>').then(html => { res.setHeader("Content-Type", "text/html"); res.end(html); });
    });
  } }],
  optimizeDeps: { entries: ["tests/database-workbench-proof.tsx"], include: ["react", "react-dom/client", "@tabularis/web-ui", "zod"], noDiscovery: true },
  // Dependency-optimizer reloads must not replace an in-flight controlled session.
  resolve: { dedupe: ["react", "react-dom"] }, server: { host: "127.0.0.1", port: 0, strictPort: true, hmr: false },
});
let browser;
try {
  await server.listen(); const address = server.httpServer.address(); assert(address && typeof address !== "string");
  const origin = `http://127.0.0.1:${address.port}`;
  browser = await chromium.launch({ headless: true });
  for (const stage of ["launch", "session", "initialize"]) {
    const page = await browser.newPage(); page.setDefaultTimeout(5000); page.setDefaultNavigationTimeout(30_000); await page.clock.install();
    const requests = [], aborted = []; let pending, attempts = 0;
    page.on("requestfailed", request => aborted.push(new URL(request.url()).pathname));
    await page.route("**/__launch/*", async route => {
      attempts++; requests.push(new URL(route.request().url()).pathname);
      if (stage === "launch" && attempts === 1) { pending = route; return; }
      await route.fulfill({ json: attempts === 1 ? { launch: { gatewayOrigin: origin, code: "first-code" } } : { error: "private-internal-host: credential-secret" } });
    });
    await page.route(`${origin}/session`, async route => {
      requests.push("/session"); assert.equal(route.request().postDataJSON().code, "first-code");
      if (stage === "session") { pending = route; return; }
      await route.fulfill({ json: { scopeID: "scope", connectionID: "connection", baseUrl: `${origin}/s/scope` } });
    });
    await page.route(`${origin}/s/scope/api/v1/session`, route => { requests.push("/s/scope/api/v1/session"); pending = route; });
    await page.goto(`${origin}/__workbench`); await page.getByRole("status").filter({ hasText: "Connecting to database…" }).waitFor();
    await until(() => pending);
    await page.clock.fastForward(31000);
    await page.getByRole("alert").filter({ hasText: "Database access is unavailable." }).waitFor();
    if (stage !== "launch") {
      await until(() => aborted.includes(stage === "session" ? "/session" : "/s/scope/api/v1/session"));
      assert(aborted.includes(stage === "session" ? "/session" : "/s/scope/api/v1/session"), `${stage} request must abort`);
    }
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    await page.getByRole("alert").filter({ hasText: "Database access is unavailable." }).waitFor();
    assert.equal(attempts, 2, "retry obtains a fresh one-time launch");
    assert(!await page.locator("body").innerText().then(text => text.includes("credential-secret")), "internal errors stay private");
    if (stage === "launch") {
      await pending.fulfill({ json: { launch: { gatewayOrigin: origin, code: "late-code" } } });
      await page.evaluate(() => new Promise(resolve => queueMicrotask(resolve)));
      assert(!requests.includes("/session"), "expired launch result cannot exchange its code");
    }
    await page.close();
  }
  for (const authenticated of [true, false]) {
    const page = await browser.newPage(); page.setDefaultTimeout(5000); page.setDefaultNavigationTimeout(30_000); let loggedOut = false;
    let navigations = 0;
    page.on("framenavigated", frame => { if (frame === page.mainFrame()) navigations++; });
    await page.route("**/__launch/*", route => route.fulfill({ json: { launch: { gatewayOrigin: origin, code: "valid-code" } } }));
    await page.route(`${origin}/session`, route => route.fulfill({ json: { scopeID: "valid", connectionID: "connection", baseUrl: `${origin}/s/valid` } }));
    await page.route(`${origin}/s/valid/api/v1/session`, route => route.fulfill({ json: {
      apiVersion: "v1", serverVersion: "fixture", serverBuild: { target: "owned", profile: "debug", commit: null },
      authenticated, csrfToken: "owned-csrf", access: { remote: true, authorizationLevel: "database", highRiskCapabilities: false },
      capabilities: { rpc: true, events: false, uploads: false, downloads: false, pluginAssets: false, mcpHostConfiguration: false, nativeUpdater: false },
      queryResponsePolicy: { maxRowsPerPage: 100, maxResponseBytes: 100000, streaming: false },
    } }));
    await page.route(`${origin}/s/valid/api/v1/logout`, async route => {
      assert.equal(route.request().method(), "POST"); assert.equal(route.request().headers()["x-tabularis-csrf"], "owned-csrf");
      loggedOut = true; await route.fulfill({ json: {} });
    });
    await page.route(`${origin}/s/valid/api/v1/rpc/**`, route => route.fulfill({ json: { ok: false, error: { code: "FIXTURE", message: "Outside startup proof", details: null, requestId: "owned" } } }));
    await page.goto(`${origin}/__workbench`);
    if (authenticated) {
      await page.locator(".database-workbench").waitFor();
      await page.getByRole("button", { name: "Leave database", exact: true }).click();
      await until(() => loggedOut);
      assert.equal(await page.locator(".database-workbench").count(), 0);
      assert.equal(navigations, 1, "controlled session must not be replaced by a development reload");
    } else {
      await page.getByRole("alert").filter({ hasText: "Database access is unavailable." }).waitFor();
      assert.equal(await page.locator(".database-workbench").count(), 0, "unauthenticated negotiation never exposes a workspace");
      assert.equal(loggedOut, false);
    }
    await page.close();
  }
  const page = await browser.newPage(); page.setDefaultTimeout(5000); page.setDefaultNavigationTimeout(30_000);
  let oldSession, lateNegotiations = 0;
  await page.route("**/__launch/*", route => route.fulfill({ json: { launch: { gatewayOrigin: origin, code: route.request().url().endsWith("first") ? "old" : "new" } } }));
  await page.route(`${origin}/session`, async route => {
    if (route.request().postDataJSON().code === "old") { oldSession = route; return; }
    await route.fulfill({ status: 503, json: {} });
  });
  await page.route(`${origin}/s/**`, async route => { lateNegotiations++; await route.fulfill({ status: 503, json: {} }); });
  await page.goto(`${origin}/__workbench?locale=de`);
  await until(() => oldSession);
  await page.getByRole("button", { name: "Switch database", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: "Der Datenbankzugriff ist nicht verfügbar." }).waitFor();
  await page.getByRole("button", { name: "Erneut versuchen", exact: true }).waitFor();
  await oldSession.fulfill({ json: { scopeID: "old", connectionID: "old", baseUrl: `${origin}/s/old` } }).catch(() => {});
  await page.evaluate(() => new Promise(resolve => queueMicrotask(resolve)));
  assert.equal(lateNegotiations, 0, "retired session response cannot initialize a transport");
  await page.close();
  console.log("Workbench launch/session/initialize deadlines, abort, fresh retry, stale-result fencing and German errors passed (owned HTTP).");
} finally {
  await browser?.close(); await server.close(); await rm(cacheDir, { recursive: true, force: true });
  assert.equal(server.httpServer?.listening ?? false, false);
}
