// Isolated HTTP fixtures exercise Console behavior, not native engine readiness.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { server, previewOrigin } from "./preview-server.ts";
import { commandSchema } from "../src/mail-contracts.ts";
import { fromRepo } from './repo.mjs';
const browser = await chromium.launch({
  headless: true,
  ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
    ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
    : {}),
});
const page = await browser.newPage({ viewport: { width: 1280, height: 1000 } });
let preview = false,
  denied = false;
const writes = [],
  errors = [];
const service = {
  service: { customerID: "101", enabled: true, state: "ready", revision: "1" },
  mailboxesAvailable: true,
  sendingAvailable: true,
  canActivate: false,
  canSuspend: true,
  canReconcile: false,
  pendingReview: false,
};
const directory = {
  customerID: "101",
  operator: true,
  canManage: true,
  canConfigure: true,
  canRetryConfiguration: false,
  configuration: null,
  mailboxes: [],
  domains: [],
  verifiedDomains: [{ id: "201", domain: "example.test" }],
  people: [
    { id: "501", name: "Acceptance member", email: "member@example.test" },
  ],
  grants: [],
};
const mailbox = {
  id: "401",
  name: "Support",
  address: "support@example.test",
  quotaBytes: "1048576",
  state: "pending",
  enabled: true,
  canOpen: false,
};
function advance(state = "pending") {
  const revision = String(
    BigInt(directory.configuration?.revision ?? "0") + 1n,
  );
  directory.configuration = {
    revision,
    appliedRevision: directory.configuration?.appliedRevision ?? "0",
    state,
  };
}
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
          id: "configuration-session",
          userId: "1",
          expiresAt: "2099-01-01T00:00:00Z",
        },
      },
    },
  }),
);
await page.route("**/api/console/mail", async (route) => {
  if (denied) return route.fulfill({ status: 403, json: {} });
  const command = commandSchema.parse(route.request().postDataJSON());
  if (command.action === "getTenantMailService")
    return route.fulfill({ json: { data: service } });
  if (command.action === "getTenantMailboxes")
    return route.fulfill({ json: { data: directory } });
  assert.equal(command.action, "manageTenantMailboxes");
  writes.push(command.input);
  const input = command.input;
  advance();
  if (input.action === "domain.attach") {
    directory.domains.push({
      id: "301",
      name: "example.test",
      state: "pending",
      dkimKeys: [],
    });
    return route.fulfill({ json: { data: { ok: true, id: "301" } } });
  }
  if (input.action === "mailbox.create")
    return route.fulfill({ status: 503, json: {} });
  if (input.action === "grant")
    directory.grants.push({
      mailboxID: input.mailboxID,
      subject: input.subject,
      enabled: true,
      revision: "1",
    });
  if (input.action === "revoke")
    directory.grants = directory.grants.map((grant) => ({
      ...grant,
      enabled: false,
      revision: "2",
    }));
  if (input.action === "configuration.retry")
    directory.canRetryConfiguration = false;
  return route.fulfill({ json: { data: { ok: true } } });
});
try {
  await page.goto(previewOrigin + "/mail?customer=101");
  const attach = page.getByRole("form", { name: "Attach incoming domain" });
  await attach.getByRole("combobox").selectOption("201");
  await attach.getByRole("button").click();
  const create = page.getByRole("form", { name: "Create mailbox" });
  await page
    .getByText("These changes are not yet applied.", { exact: false })
    .waitFor();
  assert.equal(
    await create.getByRole("button").isDisabled(),
    true,
    "pending domain cannot receive mailbox creation",
  );
  directory.domains[0].state = "ready";
  directory.configuration.appliedRevision = directory.configuration.revision;
  directory.configuration.state = "succeeded";
  await page.waitForFunction(
    () =>
      document.querySelector('form[aria-label="Create mailbox"] fieldset')
        ?.disabled === false,
  );
  await create.getByRole("combobox").selectOption("301");
  await create.getByRole("textbox", { name: "Local part" }).fill("support");
  await create.getByRole("textbox", { name: "Mailbox name" }).fill("Support");
  await create.getByRole("textbox", { name: "Quota in bytes" }).fill("1048576");
  await create.getByRole("button").click();
  await page
    .getByRole("heading", { name: "Configuration outcome is unknown" })
    .waitFor();
  await page
    .getByRole("combobox", { name: "Workspace", exact: true })
    .selectOption("");
  await page
    .getByRole("combobox", { name: "Workspace", exact: true })
    .selectOption("101");
  await page
    .getByRole("heading", { name: "Configuration outcome is unknown" })
    .waitFor();
  assert.equal(
    writes.filter((input) => input.action === "mailbox.create").length,
    1,
  );
  directory.mailboxes.push(mailbox);
  await page.getByRole("button", { name: "Refresh configuration" }).click();
  await page
    .getByText("support@example.test", { exact: true })
    .first()
    .waitFor();
  await page
    .getByRole("heading", { name: "Configuration outcome is unknown" })
    .waitFor({ state: "hidden" });
  const grant = page.getByRole("form", { name: "Grant mailbox access" });
  await grant
    .getByRole("combobox", { name: "Mailbox", exact: true })
    .selectOption("401");
  await grant
    .getByRole("combobox", { name: "Person", exact: true })
    .selectOption("501");
  await grant.getByRole("checkbox").check();
  await grant.getByRole("button").click();
  await page
    .getByText("Requested access · Revision 1", { exact: true })
    .waitFor();
  directory.canConfigure = false;
  await page.getByRole("button", { name: "Refresh Mail", exact: true }).click();
  const revoke = page.getByRole("form", { name: "Revoke mailbox access" });
  await revoke.getByRole("combobox").selectOption("401:501");
  await revoke.getByRole("checkbox").check();
  await revoke.getByRole("button").click();
  await page.getByText("Revoked · Revision 2", { exact: true }).waitFor();
  assert.equal(writes.filter((input) => input.action === "revoke").length, 1);
  directory.canConfigure = true;
  directory.canRetryConfiguration = true;
  directory.configuration.state = "succeeded";
  await page.getByRole("button", { name: "Refresh Mail", exact: true }).click();
  await page
    .getByRole("button", { name: "Retry configuration", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Retry configuration", exact: true })
    .waitFor({ state: "hidden" });
  assert.equal(
    writes.filter((input) => input.action === "configuration.retry").length,
    1,
  );
  for (const width of [320, 375, 768, 1280]) {
    await page.setViewportSize({ width, height: 1000 });
    assert.equal(
      await page.evaluate(
        () => document.documentElement.scrollWidth > innerWidth,
      ),
      false,
      `Configuration overflow at ${width}`,
    );
  }
  const shots = fromRepo(".runtime-test/mail-configuration-ui-screens/");
  await mkdir(shots, { recursive: true });
  await page.evaluate(() => {
    document.activeElement?.blur();
    window.scrollTo(0, 0);
  });
  await page.screenshot({
    path: fileURLToPath(new URL("configuration-desktop.png", shots)),
    fullPage: true,
  });
  await page.setViewportSize({ width: 375, height: 1000 });
  await page.evaluate(() => {
    document.activeElement?.blur();
    window.scrollTo(0, 0);
  });
  await page.screenshot({
    path: fileURLToPath(new URL("configuration-mobile.png", shots)),
    fullPage: true,
  });
  denied = true;
  await page.getByRole("button", { name: "Refresh Mail", exact: true }).click();
  await page.getByText("Unable to load", { exact: true }).waitFor();
  assert.doesNotMatch(
    await page.getByRole("main").innerText(),
    /support@example.test|Acceptance member/,
  );
  denied = false;
  preview = true;
  await page.reload();
  await page
    .getByText("Customer preview is read-only.", { exact: true })
    .waitFor();
  assert.equal(
    await page
      .getByRole("form", { name: "Grant mailbox access" })
      .getByRole("button")
      .isDisabled(),
    true,
  );
  assert.deepEqual(errors, []);
  console.log(
    "Mail configuration controlled browser passed: attach, actual polling, pending mailbox, uncertain creation without duplicate, explicit grant/revoke during blocked provisioning, authoritative retry, denial, preview and responsive forms. HTTP fixtures only.",
  );
} catch (error) {
  console.error(await page.getByRole("main").innerText());
  throw error;
} finally {
  await browser.close();
  await server.close();
}
