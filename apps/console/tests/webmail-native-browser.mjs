// Actual built Console → Core HTTPS launch → HTTPS Mail gateway → real Community Edition JMAP.
import assert from "node:assert/strict";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright";
import { z } from "zod";

const path = process.env.WEBDOCK_WEBMAIL_BROWSER_FIXTURE;
assert.ok(path, "An owned native Webmail fixture file is required");
const loopback = z.url().refine((value) => {
  const url = new URL(value);
  return url.protocol === "https:" && url.hostname === "localhost" && url.origin === value;
});
const fixture = z.strictObject({
  origin: loopback, gatewayOrigin: loopback,
  cookie: z.strictObject({ name: z.string().min(1), value: z.string().min(1) }),
  customerID: z.string().regex(/^[1-9][0-9]*$/), customerName: z.string().min(1),
  mailboxName: z.string().min(1), address: z.email(),
  seededSubject: z.string().min(1), savedSubject: z.string().min(1),
}).parse(JSON.parse(readFileSync(path, "utf8")));
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}),
});
// Both owned servers use disposable self-signed loopback certificates.
const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, ignoreHTTPSErrors: true });
const page = await context.newPage();
page.setDefaultTimeout(60000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
page.on("console", (message) => { if (message.type() === "error") errors.push(message.text()); });
const gateway = [];
page.on("request", (request) => {
  const url = new URL(request.url());
  if (url.origin === fixture.gatewayOrigin) gateway.push({ method: request.method(), path: url.pathname, body: request.postData() ?? "" });
});
const network = [];
page.on("requestfailed", (request) => network.push(`${request.method()} ${request.url()} failed: ${request.failure()?.errorText ?? "unknown"}`));
page.on("response", (response) => { if (!response.ok()) network.push(`${response.request().method()} ${response.url()} -> ${response.status()}`); });
const launches = [];
page.on("response", async (response) => {
  const url = new URL(response.url());
  if (url.pathname !== "/api/console/mail" || response.request().method() !== "POST") return;
  if (response.request().postDataJSON().action === "launchTenantWebmail") launches.push((await response.json()).data);
});
try {
  await context.addCookies([{ ...fixture.cookie, url: fixture.origin, httpOnly: true, sameSite: "Lax", secure: true }]);
  await page.goto(`${fixture.origin}/tenants`);
  const workspace = page.getByRole("listitem").filter({ has: page.getByText(fixture.customerName, { exact: true }) });
  await workspace.getByRole("link", { name: "Mail", exact: true }).click();
  await page.getByRole("link", { name: "Open mailbox", exact: true }).click();
  await page.getByRole("heading", { level: 1, name: fixture.mailboxName }).waitFor();
  await page.getByText(fixture.address, { exact: true }).waitFor();

  // Exactly one launch, exchanged once by POST body; the code never enters a URL or storage.
  assert.equal(launches.length, 1);
  const [launch] = launches;
  assert.equal(launch.gatewayOrigin, fixture.gatewayOrigin);
  assert.equal(launch.mailboxID !== undefined, true);
  const exchanges = gateway.filter((request) => request.path === "/session");
  assert.equal(exchanges.length, 1);
  assert.equal(exchanges[0].method, "POST");
  assert.deepEqual(JSON.parse(exchanges[0].body), { code: launch.code });
  assert.equal(gateway.some((request) => request.path.includes(launch.code)), false);
  assert.equal(page.url().includes(launch.code), false);
  const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }, document.cookie]));
  assert.equal(stored.includes(launch.code), false);
  // Loopback cookies are not port-isolated; select the gateway's own scoped session cookie.
  const cookies = (await context.cookies()).filter((cookie) => cookie.name === "__Secure-webdock_mail");
  assert.equal(cookies.length, 1);
  assert.match(cookies[0].path, /^\/s\/[1-9][0-9]*\/$/);
  assert.equal(cookies[0].httpOnly, true);
  assert.equal(cookies[0].secure, true);
  assert.equal(cookies[0].sameSite, "Strict");

  // Real engine folders and the seeded message read through the scoped gateway session.
  const folders = page.getByRole("navigation", { name: "Mail folders" });
  await folders.getByRole("button", { name: /Drafts/ }).click();
  const order = (await folders.getByRole("button").allInnerTexts()).map((text) => text.split("\n")[0].trim());
  assert.deepEqual(order, ["Inbox", "Drafts", "Sent Items", "Junk Mail", "Deleted Items"]);
  await page.getByRole("button", { name: new RegExp(fixture.seededSubject) }).click();
  const message = page.getByRole("article", { name: "Message content" });
  await message.getByRole("heading", { name: fixture.seededSubject }).waitFor();
  await message.getByText("Seeded body for the native browser proof.").waitFor();

  // One explicit draft write; sending stays disabled for this tenant.
  await page.getByRole("button", { name: "New message", exact: true }).click();
  const composer = page.getByRole("form", { name: "Compose message" });
  await composer.getByLabel("To", { exact: true }).fill("recipient@customer.test");
  await composer.getByLabel("Subject", { exact: true }).fill(fixture.savedSubject);
  await composer.getByLabel("Message", { exact: true }).fill("Written in the native Console browser proof.");
  assert.equal(await composer.getByRole("button", { name: "Send message", exact: true }).isDisabled(), true);
  await composer.getByText("Sending is not enabled for this workspace. You can save a draft.").waitFor();
  await composer.getByRole("button", { name: "Save draft", exact: true }).click();
  await page.getByRole("status").filter({ hasText: "Draft saved." }).waitFor();
  assert.equal(await composer.getByRole("button", { name: "Send message", exact: true }).isDisabled(), true);
  // The refreshed Drafts list shows the saved draft from the real engine.
  await page.getByRole("button", { name: new RegExp(fixture.savedSubject) }).waitFor({ timeout: 15000 });
  const writes = gateway.filter((request) => request.path.endsWith("/mail") && JSON.parse(request.body).action === "draft");
  assert.equal(writes.length, 1);

  for (const width of [320, 375, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `Webmail overflow at ${width}px`);
  }
  const screenshots = process.env.WEBDOCK_MAIL_BROWSER_SCREENSHOT_DIR;
  if (screenshots) {
    mkdirSync(screenshots, { recursive: true, mode: 0o700 });
    for (const [name, width] of [["desktop", 1280], ["mobile", 375]]) {
      await page.setViewportSize({ width, height: 900 });
      await page.evaluate(() => { if (document.activeElement instanceof HTMLElement) document.activeElement.blur(); window.scrollTo(0, 0); });
      await page.screenshot({ path: join(screenshots, `webmail-native-${name}.png`), fullPage: true });
    }
  }

  // Leaving the mailbox closes the gateway session.
  await page.getByRole("link", { name: "Mailboxes", exact: true }).click();
  await page.waitForFunction(() => location.pathname === "/mail");
  await expectLogout();
  assert.deepEqual(errors, []);
  console.log("Native Webmail browser passed: one-use launch, POST-only exchange, Strict gateway cookie, real engine folders/message, one draft write, disabled sending and responsive layout.");
} catch (error) {
  // Network outcomes only: no request bodies, cookies or launch codes are printed.
  console.error(network.map((line) => line.replace(/[?#].*$/, "")).join("\n"));
  console.error(errors.join("\n"));
  console.error(gateway.map((request) => `${request.method} ${request.path}`).join("\n"));
  const screenshots = process.env.WEBDOCK_MAIL_BROWSER_SCREENSHOT_DIR;
  if (screenshots) {
    mkdirSync(screenshots, { recursive: true, mode: 0o700 });
    await page.screenshot({ path: join(screenshots, "webmail-native-failed.png"), fullPage: true });
  }
  throw error;
} finally {
  await browser.close();
}
async function expectLogout() {
  const deadline = Date.now() + 10000;
  while (!gateway.some((request) => request.path.endsWith("/logout"))) {
    assert.ok(Date.now() < deadline, "Leaving Webmail must close the gateway session");
    await page.waitForTimeout(100);
  }
}
