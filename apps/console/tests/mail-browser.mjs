// Controlled HTTP fixtures, not native Mail acceptance.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { server, previewOrigin } from "./preview-server.ts";
import { fromRepo } from './repo.mjs';
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
    ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
    : {}),
});
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
let mode = "normal",
  writes = 0,
  preview = false;
const errors = [];
const service = {
  service: {
    customerID: "101",
    enabled: false,
    state: "disabled",
    revision: "0",
  },
  mailboxesAvailable: true,
  sendingAvailable: true,
  canActivate: true,
  canSuspend: false,
  canReconcile: false,
  pendingReview: false,
};
const directory = {
  customerID: "101",
  operator: true,
  configuration: { revision: "2", appliedRevision: "0", state: "pending" },
  canManage: true,
  canConfigure: false,
  mailboxes: [
    {
      id: "401",
      name: "Acceptance mailbox",
      address: "mailbox@example.test",
      state: "pending",
      enabled: true,
      quotaBytes: "1000000000",
      canOpen: false,
    },
  ],
  domains: [
    { id: "501", name: "example.test", state: "pending", dkimKeys: [] },
  ],
  verifiedDomains: [],
  people: [],
  grants: [],
};
page.on("pageerror", (error) => errors.push(error.message));
await page.route("**/console-config.json", (route) =>
  route.fulfill({
    json: {
      apiOrigin: previewOrigin,
      authOrigin: previewOrigin,
      enabledFeatures: ["mail"],
    },
  }),
);
await page.route("**/v1/auth/session", (route) =>
  route.fulfill({
    json: {
      data: {
        user: {
          id: "1",
          name: "Acceptance operator",
          email: "operator@example.test",
          role: "operator",
          emailVerified: true,
          preview,
        },
        session: {
          id: "mail-test-session",
          userId: "1",
          expiresAt: "2099-01-01T00:00:00Z",
        },
      },
    },
  }),
);
await page.route("**/api/console/mail", async (route) => {
  if (mode === "denied") return route.fulfill({ status: 403, json: {} });
  const body = route.request().postDataJSON();
  if (body.action === "getTenantMailService")
    return route.fulfill({ json: { data: service } });
  if (body.action === "getTenantMailboxes")
    return route.fulfill({ json: { data: directory } });
  writes++;
  if (mode === "uncertain") return route.fulfill({ status: 503, json: {} });
  assert.equal(body.input.revision, service.service.revision);
  service.service.revision = String(BigInt(service.service.revision) + 1n);
  service.service.enabled = body.input.action !== "suspend";
  service.service.state = service.service.enabled ? "pending" : "suspended";
  service.canActivate = !service.service.enabled;
  service.canSuspend = service.service.enabled;
  return route.fulfill({ json: { data: service } });
});
try {
  await page.goto(previewOrigin + "/tenants");
  await page.getByRole("main").getByRole("link", { name: "Mail", exact: true }).click();
  await page.getByText("Acceptance mailbox", { exact: true }).waitFor();
  assert.match(await page.getByRole("main").innerText(), /Applied revision 0/);
  mode = "uncertain";
  await page
    .getByRole("button", { name: "Activate incoming mail", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Confirm activate", exact: true })
    .click();
  await page
    .getByRole("heading", { name: "Request outcome is unknown", exact: true })
    .waitFor();
  await page.getByRole("button", { name: "Refresh Mail" }).click();
  assert.equal(
    await page
      .getByRole("button", { name: "Activate incoming mail", exact: true })
      .isDisabled(),
    true,
  );
  assert.equal(writes, 1);
  mode = "normal";
  Object.assign(service.service, {
    enabled: true,
    state: "ready",
    revision: "1",
  });
  service.canActivate = false;
  service.canSuspend = true;
  await page.getByRole("button", { name: "Refresh Mail" }).click();
  await page.waitForFunction(() =>
    [...document.querySelectorAll("button")].some(
      (button) =>
        button.textContent === "Suspend incoming mail" && !button.disabled,
    ),
  );
  await page
    .getByRole("button", { name: "Suspend incoming mail", exact: true })
    .click();
  assert.match(
    await page.getByRole("dialog").innerText(),
    /storage are retained/,
  );
  await page
    .getByRole("button", { name: "Confirm suspend", exact: true })
    .click();
  await page.getByText("suspended", { exact: true }).waitFor();
  for (const width of [320, 375, 768, 1280]) {
    await page.setViewportSize({ width, height: 900 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
      false,
      `Mail overflow at ${width}`,
    );
  }
  const screenshots = fromRepo(".runtime-test/mail-ui-screens/");
  await mkdir(screenshots, { recursive: true });
  await page.screenshot({
    path: fileURLToPath(new URL("mail-desktop.png", screenshots)),
    fullPage: true,
  });
  await page.setViewportSize({ width: 375, height: 900 });
  await page.screenshot({
    path: fileURLToPath(new URL("mail-mobile.png", screenshots)),
    fullPage: true,
  });
  mode = "denied";
  await page.getByRole("button", { name: "Refresh Mail" }).click();
  await page
    .getByRole("heading", { name: "Incoming mail", exact: true })
    .waitFor({ state: "hidden" });
  assert.doesNotMatch(
    await page.locator("body").innerText(),
    /Acceptance mailbox|mailbox@example.test/,
  );
  mode = "normal";
  preview = true;
  await page.reload();
  await page
    .getByText("Customer preview is read-only.", { exact: true })
    .waitFor();
  assert.equal(
    await page
      .getByRole("button", { name: "Activate incoming mail", exact: true })
      .isDisabled(),
    true,
  );
  assert.deepEqual(errors, []);
  console.log(
    "Mail controlled browser passed: workspace navigation, uncertainty, retention confirmation, denied stale data, preview and responsive widths. HTTP fixtures only.",
  );
} catch (error) {
  console.error(await page.locator("body").innerText());
  throw error;
} finally {
  await browser.close();
  await server.close();
}
