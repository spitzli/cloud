// Actual built Console → native HTTP/RLS → enrolled agent → isolated Community Edition.
import assert from "node:assert/strict";
import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright";
import { z } from "zod";
import { configureMail } from "./mail-native-configuration.mjs";

const path = process.env.WEBDOCK_MAIL_BROWSER_FIXTURE;
assert.ok(path, "An owned native Mail fixture file is required");
const fixture = z.strictObject({
  origin: z.url(),
  cookie: z.strictObject({ name: z.string().min(1), value: z.string().min(1) }),
  customerID: z.string().regex(/^[1-9][0-9]*$/),
  customerName: z.string().min(1),
  configure: z.boolean(),
}).parse(JSON.parse(readFileSync(path, "utf8")));
const origin = new URL(fixture.origin);
assert.equal(origin.origin, fixture.origin);
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(origin.hostname));
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
    ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
    : {}),
});
const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
const page = await context.newPage();
page.setDefaultTimeout(60000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const writes = [];
page.on("request", (request) => {
  if (new URL(request.url()).pathname !== "/api/console/mail" || request.method() !== "POST") return;
  const body = request.postDataJSON();
  if (body.action === "manageTenantMailService") writes.push(body);
});
async function recordedState(state) {
  await page.waitForFunction((expected) => {
    const label = [...document.querySelectorAll("dt")].find((node) => node.textContent === "Recorded state");
    return label?.parentElement?.querySelector("dd")?.textContent === expected;
  }, state, { timeout: 120000 });
}
async function lifecycle(action, revision, expected) {
  const label = `${action[0].toUpperCase()}${action.slice(1)} incoming mail`;
  await page.getByRole("button", { name: label, exact: true }).click();
  if (action === "suspend")
    assert.match(await page.getByRole("dialog").innerText(), /storage are retained/);
  const response = page.waitForResponse((value) =>
    new URL(value.url()).pathname === "/api/console/mail" &&
    value.request().method() === "POST" &&
    value.request().postDataJSON().action === "manageTenantMailService",
  );
  await page.getByRole("button", { name: `Confirm ${action}`, exact: true }).click();
  const accepted = await response;
  assert.equal(accepted.status(), 200);
  const reply = await accepted.json();
  assert.equal(reply.data.service.customerID, fixture.customerID);
  assert.equal(reply.data.service.revision, revision);
  await recordedState(expected);
  await page.waitForFunction(() =>
    ![...document.querySelectorAll("h3")].some((node) => node.textContent === "Request recorded"),
  );
}
try {
  await context.addCookies([{
    ...fixture.cookie, url: fixture.origin, httpOnly: true, sameSite: "Lax",
    secure: origin.protocol === "https:",
  }]);
  await page.goto(`${fixture.origin}/tenants`);
  const workspace = page.getByRole("listitem").filter({
    has: page.getByText(fixture.customerName, { exact: true }),
  });
  await workspace.getByRole("link", { name: "Mail", exact: true }).click();
  await recordedState("disabled");
  await lifecycle("activate", "1", "ready");
  assert.equal(await page.getByRole("button", { name: "Activate incoming mail", exact: true }).isDisabled(), true);
  await lifecycle("suspend", "2", "suspended");
  await lifecycle("activate", "3", "ready");
  assert.deepEqual(writes.map((request) => request.input.action), ["activate", "suspend", "activate"]);
  assert.deepEqual(writes.map((request) => request.input.revision), ["0", "1", "2"]);
  if (fixture.configure) await configureMail(page, fixture.customerID);
  for (const width of [320, 375, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `Mail overflow at ${width}px`);
  }
  const screenshots = process.env.WEBDOCK_MAIL_BROWSER_SCREENSHOT_DIR;
  if (screenshots) {
    mkdirSync(screenshots, { recursive: true, mode: 0o700 });
    for (const [name, width] of [["desktop", 1280], ["mobile", 375]]) {
      await page.setViewportSize({ width, height: 900 });
      await page.evaluate(() => {
        document.activeElement?.blur();
        window.scrollTo(0, 0);
      });
      const prefix = fixture.configure ? "mail-native-configuration" : "mail-native";
      await page.screenshot({ path: join(screenshots, `${prefix}-${name}.png`), fullPage: true });
    }
  }
  assert.deepEqual(errors, []);
  console.log("Native Mail browser passed: explicit activation, confirmed engine readiness, retained suspension/resume, revision-bound requests and responsive navigation.");
  if (fixture.configure)
    console.log("Native configuration passed: verified domain, mailbox, explicit grant, remote restriction and fresh authorized full retry.");
} catch (error) {
  const screenshots = process.env.WEBDOCK_MAIL_BROWSER_SCREENSHOT_DIR;
  if (screenshots) {
    mkdirSync(screenshots, { recursive: true, mode: 0o700 });
    await page.screenshot({ path: join(screenshots, "mail-native-failed.png"), fullPage: true });
  }
  throw error;
} finally {
  await browser.close();
}
