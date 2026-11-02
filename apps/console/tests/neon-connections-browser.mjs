// Local HTTP fixtures exercise native routes; no real Neon account is contacted.
import assert from "node:assert/strict";
import { chromium } from "playwright";
import { server, previewOrigin } from "./preview-server.ts";
const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE } : {}) });
const errors = [];
try {
  const context = await browser.newContext(); const page = await context.newPage(); page.setDefaultTimeout(10000);
  page.on("pageerror", error => errors.push(error.message));
  const path = value => url => url.pathname === value;
  const json = value => route => route.fulfill({ json: value });
  let role = "owner", preview = false, fail = 0, connection = null;
  const calls = [];
  const connected = { id: "301", revision: 1, status: "connected", authMethod: "api-key", organizationID: "org-example", neonProjectID: null, connectedAt: "2026-11-01T00:00:00Z" };
  await page.route(path("/console-config.json"), json({ apiOrigin: previewOrigin, authOrigin: previewOrigin, enabledFeatures: [] }));
  await page.route(path("/v1/auth/session"), route => route.fulfill({ json: { data: { user: { id: "2", name: "Fixture Owner", email: "fixture@example.test", role: role === "operator" || preview ? "operator" : "customer", preview, emailVerified: true }, session: { id: "fixture", userId: "2", expiresAt: "2099-12-01T00:00:00Z" } } } }));
  await page.route(path("/api/console/tenants"), route => route.fulfill({ json: [{ id: "101", name: "Fixture workspace", status: "active", role }, { id: "102", name: "Other workspace", status: "active", role }] }));
  await page.route(path("/api/console/tenant-preview/current"), json({ customerID: "101", customerName: "Fixture workspace", status: "active", mode: "read_only", readOnly: true, expiresAt: "2099-12-01T00:00:00Z" }));
  await page.route(path("/api/console/customers/101/self-service"), route => route.fulfill({ status: 403, json: {} }));
  await page.route(path("/api/console/customers/101/usage"), route => route.fulfill({ status: 403, json: {} }));
  await page.route(path("/api/console/plans"), route => route.fulfill({ status: 403, json: {} }));
  await page.route(path("/api/console/tenant/101/projects"), json([]));
  await page.route(path("/api/console/tenant/102/projects"), json([]));
  await page.route(path("/api/console/byok/neon"), async route => {
    const command = route.request().postDataJSON(); const safe = { ...command }; delete safe.apiKey; calls.push(safe);
    if (["connect", "disconnect"].includes(command.action) && fail) { const status = fail; fail = 0; await route.fulfill({ status, json: {} }); return; }
    if (command.action === "connect") connection = { ...connected, neonProjectID: command.neonProjectID ?? null };
    if (command.action === "disconnect") connection = { ...connected, revision: 2, status: "disconnected" };
    await route.fulfill({ json: { data: { connection: command.customerID === "102" ? null : connection, canManage: role !== "member" && !preview, ...(command.action === "projects" ? { projects: [{ id: connection?.neonProjectID ?? (command.cursor ? "project-second" : "project-first"), name: connection?.neonProjectID ? "Scoped Neon project" : command.cursor ? "Second Neon project" : "First Neon project", regionId: "aws-eu-central-1" }], nextCursor: connection?.neonProjectID || command.cursor ? null : "cursor-two" } : {}) } } });
  });
  const open = async () => { await page.goto(`${previewOrigin}/workspaces/101/connections`); await page.getByRole("heading", { name: "Connections", exact: true }).waitFor(); };
  for (const width of [375, 1280]) {
    connection = null; await page.setViewportSize({ width, height: 1000 }); await open();
    const form = page.getByRole("form", { name: "Connect Neon", exact: true }); const key = page.getByLabel("Neon API key", { exact: true });
    await form.getByLabel("Organization ID (optional)", { exact: true }).fill("org-retained");
    fail = 400; await key.fill("fixture-key-do-not-log"); await form.getByRole("button", { name: "Connect Neon", exact: true }).click();
    await page.getByText(/^Neon rejected this request/).waitFor(); assert.equal(await key.inputValue(), "");
    assert.equal(await form.getByLabel("Organization ID (optional)", { exact: true }).inputValue(), "org-retained");
    fail = 503; await key.fill("fixture-key-do-not-log"); await form.getByRole("button", { name: "Connect Neon", exact: true }).click(); await page.getByText(/^The result is uncertain/).waitFor(); assert.equal(await key.inputValue(), "");
    const first = calls.filter(c => c.action === "connect").at(-1);
    await key.fill("fixture-key-do-not-log"); await form.getByRole("button", { name: "Retry same connection", exact: true }).click(); await page.getByText(/^Neon connected\./).waitFor();
    assert.deepEqual(calls.filter(c => c.action === "connect").at(-1), first); assert.equal(await key.inputValue(), "");
    await page.getByRole("button", { name: "Refresh projects", exact: true }).click(); await page.getByText("First Neon project", { exact: true }).waitFor();
    await page.getByRole("button", { name: "Next project page", exact: true }).click(); await page.getByText("Second Neon project", { exact: true }).waitFor(); assert.equal(await page.getByText("First Neon project", { exact: true }).count(), 0);
    await page.getByRole("button", { name: "Disconnect Neon", exact: true }).click(); await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.getByRole("button", { name: "Disconnect Neon", exact: true }).click(); await page.getByText(/projects and data remain intact/).waitFor(); await page.getByRole("button", { name: "Confirm disconnect", exact: true }).click(); await page.getByText(/^Neon disconnected\./).waitFor();
    await page.getByText("Optional: create a project key with the Neon CLI", { exact: true }).click();
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No horizontal overflow at ${width}`);
    await page.screenshot({ path: `/tmp/neon-connections-${width}.png`, fullPage: true });
  }
  connection = null; await open();
  const scopedForm = page.getByRole("form", { name: "Connect Neon", exact: true });
  const scopedField = scopedForm.getByLabel("Neon project ID (optional)", { exact: true });
  await scopedField.fill("wrong-neon-project"); fail = 400;
  await scopedForm.getByLabel("Neon API key", { exact: true }).fill("fixture-project-key"); await scopedForm.getByRole("button", { name: "Connect Neon", exact: true }).click();
  await page.getByText(/Check your API key, Neon project ID and permissions/).waitFor(); assert.equal(await scopedField.inputValue(), "wrong-neon-project");
  await scopedField.fill("selected-neon-project"); fail = 503;
  await scopedForm.getByLabel("Neon API key", { exact: true }).fill("fixture-project-key"); await scopedForm.getByRole("button", { name: "Connect Neon", exact: true }).click();
  await page.getByText(/^The result is uncertain/).waitFor(); assert(await scopedField.isDisabled());
  const scopedAttempt = calls.filter(c => c.action === "connect").at(-1); assert.equal(scopedAttempt.neonProjectID, "selected-neon-project");
  await scopedField.evaluate(element => { element.value = "changed-neon-project"; });
  await scopedForm.getByLabel("Neon API key", { exact: true }).fill("fixture-project-key"); await scopedForm.getByRole("button", { name: "Retry same connection", exact: true }).click();
  await page.getByText(/keep the original organization and Neon project IDs/).waitFor(); assert.deepEqual(calls.filter(c => c.action === "connect").at(-1), scopedAttempt);
  await scopedField.evaluate(element => { element.value = "selected-neon-project"; });
  await scopedForm.getByLabel("Neon API key", { exact: true }).fill("fixture-project-key"); await scopedForm.getByRole("button", { name: "Retry same connection", exact: true }).click();
  await page.getByText(/^Neon connected\./).waitFor(); assert.deepEqual(calls.filter(c => c.action === "connect").at(-1), scopedAttempt);
  await page.getByText("Neon project scope:", { exact: false }).waitFor();
  await page.getByRole("button", { name: "Refresh projects", exact: true }).click(); await page.getByText("Scoped Neon project", { exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "Next project page", exact: true }).count(), 0);
  for (const restricted of ["member", "preview"]) {
    role = restricted === "preview" ? "operator" : "member"; preview = restricted === "preview"; await open(); await page.getByText(/^Read-only view/).waitFor();
    assert.equal(await page.getByRole("form", { name: "Connect Neon", exact: true }).count(), 0); assert.equal(await page.getByRole("button", { name: "Refresh projects", exact: true }).count(), 0);
  }
  role = "operator"; preview = false; await page.goto(`${previewOrigin}/customers/101`);
  const link = page.getByRole("link", { name: "Customer connections", exact: true }); await link.waitFor(); await link.click(); await page.getByRole("heading", { name: "Connections", exact: true }).waitFor();
  await page.getByLabel("Neon API key", { exact: true }).fill("unsent-customer-draft"); await page.goto(`${previewOrigin}/workspaces/102/connections`); await page.getByRole("form", { name: "Connect Neon", exact: true }).waitFor(); assert.equal(await page.getByLabel("Neon API key", { exact: true }).inputValue(), "");
  assert.deepEqual(errors, []); await context.close();
  console.log("Neon connections browser checks passed: customer/operator routes, failed key clearing, stable retry, public state, paged inventory, disconnect cancel/confirm, guards and 375/1280px layout. Local fixtures only.");
} finally { await browser.close(); await server.close(); }
