// Captures the real EU-built customer UI with visibly synthetic data and disabled writes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium } from "playwright";

const url = new URL(process.env.NEON_PLUGIN_FIXTURE_URL ?? "");
assert(["127.0.0.1", "localhost"].includes(url.hostname) && url.pathname.endsWith("/tests/neon-plugin-fixture.html"), "Use the explicitly served loopback synthetic fixture");
const output = process.env.NEON_PLUGIN_SCREENSHOTS_DIR;
assert(output, "Set NEON_PLUGIN_SCREENSHOTS_DIR to the intended gallery directory");
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
const captures = [];
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 1100 }, deviceScaleFactor: 2 });
  await context.route("**/*", route => {
    assert.equal(new URL(route.request().url()).origin, url.origin, "Capture must make no external requests");
    return route.continue();
  });
  const page = await context.newPage();
  page.on("pageerror", error => { errors.push(error.message); });
  const open = async search => {
    const next = new URL(url); next.search = search;
    await page.goto(next.href);
    await page.getByRole("note").filter({ hasText: "Synthetic acceptance fixture" }).waitFor();
    await page.evaluate(() => document.fonts.ready);
  };
  const capture = async name => {
    const state = await page.evaluate(() => {
      const fixture = window.__neonPluginFixture.diagnostics();
      for (const form of document.querySelectorAll("form")) form.inert = true;
      for (const button of document.querySelectorAll("button")) button.disabled = true;
      return { role: fixture.sessionRole, uriHidden: !document.querySelector("code"), credentialsRequested: fixture.requests.some(request => request.action === "databases.credentials"), overflow: document.documentElement.scrollWidth > innerWidth };
    });
    assert.equal(state.role, "user"); assert(state.uriHidden); assert(!state.credentialsRequested); assert(!state.overflow);
    const file = resolve(output, name);
    await page.screenshot({ path: file, fullPage: true });
    const bytes = await readFile(file);
    captures.push({ file: name, sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length, ...state, writeActionsDisabled: true });
  };
  await open("view=create");
  await page.getByLabel("Display name", { exact: true }).fill("Store database");
  for (const [label, value] of [["Neon project", "neon-store"], ["Branch", "br-main"], ["Database", "store"], ["Role", "demo_owner"], ["Endpoint", "ep-demo"]]) {
    await page.getByRole("combobox", { name: label, exact: true }).locator(`option[value="${value}"]`).waitFor({ state: "attached" });
    await page.getByRole("combobox", { name: label, exact: true }).selectOption(value);
  }
  assert(await page.getByRole("button", { name: "Connect database", exact: true }).isEnabled());
  await capture("neon-connect.png");
  await page.setViewportSize({ width: 1280, height: 1550 });
  await open("view=resource");
  await page.getByRole("heading", { name: "Neon branches", exact: true }).waitFor();
  await capture("neon-database.png");
  await page.setViewportSize({ width: 390, height: 1700 });
  await open("view=resource&locale=de");
  await page.getByRole("heading", { name: "Verbindungseinstellungen", exact: true }).waitFor();
  await capture("neon-database-mobile-de.png");
  assert.deepEqual(errors, []);
  const proof = { browser: browser.version(), actualConsoleAndPluginUI: true, syntheticDataOnly: true, providerContacted: false, imageEditing: false, captures };
  await writeFile(resolve(output, "provenance.json"), JSON.stringify(proof, null, 2) + "\n");
  console.log(JSON.stringify(proof));
} finally { await browser.close(); }
