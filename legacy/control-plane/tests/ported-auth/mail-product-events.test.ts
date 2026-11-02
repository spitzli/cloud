import assert from "node:assert/strict";
import test from "node:test";
import {
  emptyRecipientState,
  projectRecipient,
} from "../../src/modules/mail/product/projections";
test("late acceptance and deferral cannot erase delivery; engagement never invents delivery", () => {
  const state = projectRecipient(emptyRecipientState(), {
    type: "email.delivered",
    occurredAt: "2026-10-12T12:00:00Z",
  });
  for (const type of ["email.sent", "email.delivery_delayed"] as const) {
    const late = projectRecipient(state, {
      type,
      occurredAt: "2026-10-12T11:59:59Z",
    });
    assert.equal(late.delivery, "delivered");
    assert.equal(late.firstDeliveredAt, state.firstDeliveredAt);
  }
  const click = projectRecipient(emptyRecipientState(), {
    type: "email.clicked",
    occurredAt: "2026-10-12T12:00:00Z",
  });
  assert.equal(click.delivery, "pending");
  assert.equal(click.firstClickedAt, "2026-10-12T12:00:00.000Z");
  const early = projectRecipient(click, {
    type: "email.clicked",
    occurredAt: "2026-10-12T11:00:00Z",
  });
  assert.equal(early.firstClickedAt, "2026-10-12T11:00:00.000Z");
  assert.equal(early.lastClickedAt, click.lastClickedAt);
});
