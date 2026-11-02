// Runs only against the owned native HTTP/Core/daemon fixture; no intercepted responses.
import assert from "node:assert/strict";
import { z } from "zod";

const change = z.strictObject({
  data: z.strictObject({ ok: z.literal(true), id: z.string().regex(/^[1-9][0-9]*$/).optional() }),
});

export async function configureMail(page, customerID) {
  const requests = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname !== "/api/console/mail" || request.method() !== "POST") return;
    const body = request.postDataJSON();
    if (body.action === "manageTenantMailboxes") {
      assert.equal(body.customerID, customerID);
      requests.push(body.input);
    }
  });
  async function submit(button, action) {
    const response = page.waitForResponse((value) =>
      new URL(value.url()).pathname === "/api/console/mail" &&
      value.request().method() === "POST" &&
      value.request().postDataJSON().action === "manageTenantMailboxes",
    );
    await button.click();
    const accepted = await response;
    assert.equal(accepted.status(), 200);
    const request = accepted.request().postDataJSON();
    assert.equal(request.customerID, customerID);
    assert.equal(request.input.action, action);
    return change.parse(await accepted.json()).data;
  }
  async function applied(revision, appliedRevision = revision) {
    const section = page.locator("section").filter({
      has: page.getByRole("heading", { name: "Mailbox configuration", exact: true }),
    });
    await section.getByText(
      `Requested revision ${revision} · Applied revision ${appliedRevision}`,
      { exact: true },
    ).waitFor({ timeout: 120000 });
    await section.getByText("succeeded", { exact: true }).waitFor({ timeout: 120000 });
  }
  const attach = page.getByRole("form", { name: "Attach incoming domain" });
  await attach.getByRole("combobox").selectOption("301");
  const domain = await submit(attach.getByRole("button"), "domain.attach");
  assert.ok(domain.id);
  await applied("1");
  const create = page.getByRole("form", { name: "Create mailbox" });
  await create.getByRole("combobox").selectOption(domain.id);
  await create.getByRole("textbox", { name: "Local part" }).fill("support");
  await create.getByRole("textbox", { name: "Mailbox name" }).fill("Native support");
  await create.getByRole("textbox", { name: "Quota in bytes" }).fill("1048576");
  const mailbox = await submit(create.getByRole("button"), "mailbox.create");
  assert.ok(mailbox.id);
  await applied("2");
  const grant = page.getByRole("form", { name: "Grant mailbox access" });
  await grant.getByRole("combobox", { name: "Mailbox", exact: true }).selectOption(mailbox.id);
  await grant.getByRole("combobox", { name: "Person", exact: true }).selectOption("1001");
  await grant.getByRole("checkbox").check();
  await submit(grant.getByRole("button"), "grant");
  await applied("3");
  const revoke = page.getByRole("form", { name: "Revoke mailbox access" });
  await revoke.getByRole("combobox").selectOption(`${mailbox.id}:1001`);
  await revoke.getByRole("checkbox").check();
  await submit(revoke.getByRole("button"), "revoke");
  await page.getByText("Revoked · Revision 2", { exact: true }).waitFor();
  // Restriction applies revocation, without claiming the entire requested configuration.
  await applied("4", "3");
  const retry = await submit(
    page.getByRole("button", { name: "Retry configuration", exact: true }),
    "configuration.retry",
  );
  assert.equal(retry.ok, true);
  await applied("5");
  assert.deepEqual(requests.map((request) => request.action), [
    "domain.attach", "mailbox.create", "grant", "revoke", "configuration.retry",
  ]);
  assert.equal(requests[4].revision, "4");
}
