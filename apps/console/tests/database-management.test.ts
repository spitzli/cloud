import assert from "node:assert/strict";
import { test } from "node:test";
import { acceptReviewedOffer, planFromForm } from "../src/database-management";
await test("a paid database order requires explicit price and interruption consent", () => {
  const offer = { id: "123", snapshot: { requiresInterruption: true } };
  const form = new FormData();
  assert.throws(() => acceptReviewedOffer("456", offer, form));
  form.set("confirmPrice", "yes");
  assert.throws(() => acceptReviewedOffer("456", offer, form));
  form.set("confirmInterruption", "yes");
  assert.deepEqual(acceptReviewedOffer("456", offer, form), {
    action: "databases.acceptOrder",
    projectID: "456",
    offerID: "123",
    idempotencyKey: "123-accept-database",
    confirmInterruption: true,
  });
});
await test("dedicated plan sizes and money are validated before submission", () => {
  const form = new FormData();
  for (const [k, v] of Object.entries({
    name: "Dedicated test",
    kind: "dedicated",
    monthlyPrice: "12.50",
    terms:
      "Fixed monthly service with no configured backups or recovery window.",
    cpu: "1000",
    memory: "2",
    volume: "20",
  }))
    form.set(k, v);
  const plan = planFromForm(form);
  assert.equal(plan.monthlyPriceCents, 1250);
  assert.equal(plan.volumeBytes, 21474836480);
  assert.equal(plan.memoryBytes, 2147483648);
  form.set("monthlyPrice", "12.001");
  assert.throws(() => planFromForm(form));
  form.set("monthlyPrice", "12.50");
  form.set("volume", "0");
  assert.throws(() => planFromForm(form));
});
await test("one malformed retained plan cannot hide valid catalog entries", async () => {
  const { catalogSchema } = await import("../src/database-management");
  const form = new FormData();
  for (const [k, v] of Object.entries({
    name: "Valid plan",
    kind: "dedicated",
    monthlyPrice: "12",
    terms:
      "Fixed monthly service with no configured backups or recovery window.",
    cpu: "1000",
    memory: "2",
    volume: "20",
  }))
    form.set(k, v);
  const good = {
    id: "123",
    revision: 1,
    availability: "pilot",
    plan: planFromForm(form),
  };
  const catalog = catalogSchema.parse([
    { id: "1", revision: 1, availability: "disabled", plan: {} },
    good,
  ]);
  assert.equal(catalog.unavailable, 1);
  assert.deepEqual(catalog.plans, [good]);
});
