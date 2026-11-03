// Run against an explicitly built fixture. Does not build, start a server, or take screenshots.
import assert from "node:assert/strict";
import { chromium } from "playwright";

const configuredURL = process.env.NEON_PLUGIN_FIXTURE_URL;
assert(configuredURL, "Set NEON_PLUGIN_FIXTURE_URL to the served tests/neon-plugin-fixture.html");
const fixtureURL = new URL(configuredURL);
assert(["http:", "https:"].includes(fixtureURL.protocol) && !fixtureURL.username && !fixtureURL.password && fixtureURL.pathname.endsWith("/tests/neon-plugin-fixture.html"), "Expected an explicit fixture HTML URL");
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const pageErrors = [];
const externalRequests = [];
try {
  const context = await browser.newContext();
  await context.route("**/*", route => {
    if (new URL(route.request().url()).origin === fixtureURL.origin) return route.continue();
    externalRequests.push(new URL(route.request().url()).origin);
    return route.abort();
  });
  const page = await context.newPage();
  page.setDefaultTimeout(10_000);
  page.on("pageerror", error => { pageErrors.push(error.message); });
  const diagnostics = () => page.evaluate(() => window.__neonPluginFixture.diagnostics());
  const open = async (view = "create", retry = false, locale = "en") => {
    const url = new URL(fixtureURL);
    url.search = new URLSearchParams({ view, locale, ...(retry ? { retry: "1" } : {}) }).toString();
    await page.goto(url.href);
    await page.getByRole("note").filter({ hasText: "Synthetic acceptance fixture" }).waitFor();
    await page.waitForFunction(() => Boolean(window.__neonPluginFixture));
    const state = await diagnostics();
    assert.equal(state.sessionRole, "user", "Fixture must use a customer principal");
    assert.equal(state.writable, view !== "read-only");
  };
  const noOverflow = async () => assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "Customer surface must fit the viewport");
  const noCachedSecret = async () => {
    const state = await diagnostics();
    assert(!state.sharedCache.includes("postgresql://"), "A URI reached a shared query/mutation cache");
    assert(!state.sharedCache.includes("synthetic-only-password"), "A password reached a shared query/mutation cache");
  };
  const selectTuple = async () => {
    await page.getByLabel("Display name", { exact: true }).fill("Store database");
    await page.getByRole("combobox", { name: "Neon project", exact: true }).selectOption("neon-store");
    await page.getByRole("combobox", { name: "Branch", exact: true }).selectOption("br-main");
    await page.getByRole("combobox", { name: "Database", exact: true }).selectOption("store");
    await page.getByRole("combobox", { name: "Role", exact: true }).selectOption("demo_owner");
    await page.getByRole("combobox", { name: "Endpoint", exact: true }).selectOption("ep-demo");
  };
  const resourceReady = async () => {
    await page.getByRole("heading", { name: "Connection settings", exact: true }).waitFor();
    await page.getByRole("heading", { name: "Neon branches", exact: true }).waitFor();
    await page.getByRole("button", { name: "Reveal URI", exact: true }).waitFor();
  };

  await page.setViewportSize({ width: 1280, height: 1000 });
  await open("create", true);
  assert.equal(await page.getByLabel("Neon API key", { exact: true }).count(), 0, "API keys belong in Workspace Connections");
  assert.equal(await page.getByRole("link", { name: "Workspace Connections", exact: true }).getAttribute("href"), "/workspaces/101/connections");
  await page.getByRole("combobox", { name: "Neon project", exact: true }).locator('option[value="neon-store"]').waitFor({ state: "attached" });
  for (const project of ["neon-us", "neon-london"]) assert(await page.getByRole("combobox", { name: "Neon project", exact: true }).locator(`option[value="${project}"]`).evaluate(option => option.disabled), "Only Frankfurt is enabled for new attachments");
  assert(await page.getByRole("button", { name: "Connect database", exact: true }).isDisabled());
  await selectTuple();
  // Changing the branch must clear the previously selected database, role and endpoint.
  await page.getByRole("combobox", { name: "Branch", exact: true }).selectOption("br-preview");
  await page.getByRole("combobox", { name: "Endpoint", exact: true }).locator('option[value="ep-preview"]').waitFor({ state: "attached" });
  for (const label of ["Database", "Role", "Endpoint"]) assert.equal(await page.getByRole("combobox", { name: label, exact: true }).inputValue(), "");
  assert(await page.getByRole("button", { name: "Connect database", exact: true }).isDisabled());
  await page.getByRole("combobox", { name: "Branch", exact: true }).selectOption("br-main");
  await page.getByRole("combobox", { name: "Database", exact: true }).selectOption("store");
  await page.getByRole("combobox", { name: "Role", exact: true }).selectOption("demo_owner");
  await page.getByRole("combobox", { name: "Endpoint", exact: true }).selectOption("ep-demo");
  await noOverflow();
  await page.getByRole("button", { name: "Connect database", exact: true }).click();
  await page.getByRole("alert").filter({ hasText: "Could not attach this database" }).waitFor();
  await page.getByRole("button", { name: "Connect database", exact: true }).click();
  await resourceReady();
  const attachments = (await diagnostics()).requests.filter(request => request.action === "databases.attach");
  assert.equal(attachments.length, 2);
  assert.deepEqual(attachments[0], attachments[1], "Retry must retain the original tuple and idempotency key");
  assert.match(attachments[0].idempotencyKey, /^[0-9a-f-]{36}$/);
  assert.deepEqual(attachments[0].input, { name: "Store database", neon_project_id: "neon-store", branch_id: "br-main", database_name: "store", role_name: "demo_owner", endpoint_id: "ep-demo" });
  assert.equal(await page.locator("code").count(), 0, "URI must start hidden");
  await noCachedSecret();
  await page.getByRole("button", { name: "Reveal URI", exact: true }).click();
  await page.locator("code").filter({ hasText: "synthetic-only-password" }).waitFor();
  await noCachedSecret();
  await page.getByRole("button", { name: "Hide URI", exact: true }).click();
  await page.getByRole("button", { name: "Reveal URI", exact: true }).waitFor();
  assert.equal(await page.locator("code").count(), 0);
  await page.getByLabel("Use pooled connection", { exact: true }).uncheck();
  await page.getByRole("button", { name: "Reveal URI", exact: true }).click();
  await page.locator("code").waitFor();
  assert.equal((await diagnostics()).requests.filter(request => request.action === "databases.credentials").at(-1).input.pooled, false);
  await page.getByRole("button", { name: "Hide URI", exact: true }).click();
  await page.getByRole("button", { name: "Detach database", exact: true }).click();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal((await diagnostics()).detached, false, "Cancelling detach must not mutate the attachment");
  await noOverflow();

  // Deliver a response after tab-hide cancellation: it must never paint or enter a shared cache.
  await page.evaluate(() => { window.__neonPluginFixture.holdCredentials(); });
  await page.getByRole("button", { name: "Reveal URI", exact: true }).click();
  await page.waitForFunction(() => window.__neonPluginFixture.diagnostics().pendingCredentials === 1);
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.waitForFunction(() => window.__neonPluginFixture.diagnostics().credentialAborts === 1);
  await page.evaluate(async () => {
    await window.__neonPluginFixture.completeCredentials();
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.getByRole("button", { name: "Reveal URI", exact: true }).waitFor();
  assert.equal(await page.locator("code").count(), 0);
  await noCachedSecret();

  await page.evaluate(async () => { await window.__neonPluginFixture.setProviderUnavailable(); });
  await page.getByRole("alert").filter({ hasText: "The service is not responding" }).first().waitFor();
  assert.equal(await page.getByRole("button", { name: "Reveal URI", exact: true }).count(), 0);
  await page.getByRole("button", { name: "Detach database", exact: true }).click();
  await page.getByRole("button", { name: "Confirm detach", exact: true }).click();
  await page.getByRole("heading", { name: "Database detached", exact: true }).waitFor();
  assert.equal((await diagnostics()).detached, true, "Detach must remain available during a provider outage and without the creation feature");

  await open("read-only");
  await page.getByRole("heading", { name: "Connection settings", exact: true }).waitFor();
  for (const name of ["Reveal URI", "Detach database", "Connect database"]) assert.equal(await page.getByRole("button", { name, exact: true }).count(), 0);
  assert.equal(await page.getByRole("heading", { name: "Neon branches", exact: true }).count(), 0);
  assert.deepEqual((await diagnostics()).requests.map(request => request.action), ["databases.view"]);
  await noCachedSecret();

  await page.setViewportSize({ width: 320, height: 900 });
  await open();
  await selectTuple();
  await noOverflow();
  await page.getByRole("button", { name: "Connect database", exact: true }).click();
  await resourceReady();
  await noOverflow();
  await page.getByRole("button", { name: "Reveal URI", exact: true }).click();
  await page.locator("code").waitFor();
  await noOverflow();
  await noCachedSecret();
  await page.getByRole("button", { name: "Hide URI", exact: true }).click();
  await open();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.getByRole("heading", { name: "Connection cancelled", exact: true }).waitFor();
  assert.equal((await diagnostics()).requests.filter(request => request.action === "databases.attach").length, 0);
  await open("resource", false, "de");
  await page.getByRole("heading", { name: "Verbindungseinstellungen", exact: true }).waitFor();
  await noOverflow();
  const field = page.getByRole("cell", { name: "Neon-Projekt", exact: true });
  assert.equal(await field.evaluate(element => { const range = document.createRange(); range.selectNodeContents(element); return range.getClientRects().length; }), 1, "German field labels must stay legible on one line");
  assert.equal(await page.locator("code").count(), 0);
  await noCachedSecret();
  assert.deepEqual(pageErrors, []);
  assert.deepEqual(externalRequests, [], "No external service may be contacted by this fixture");
  await context.close();
  console.log("Neon plugin customer acceptance passed: real Create/Resource + SDK, native response contracts, tuple reset, Frankfurt restriction, stable attachment retry, private URI reveal/hide/cancellation, provider-outage local detach, read-only guards and English/German 320px layout. Synthetic transport only.");
} finally {
  await browser.close();
}
