import test from "node:test";
import assert from "node:assert/strict";
import { assertSetupConfirmation } from "../../src/modules/hosting/setup-confirmation";
test("setup confirmation binds the exact unexpired preview and every paid offer", () => {
  const input = {
    setupRevision: 3,
    submittedRevision: 3,
    previewID: "current",
    submittedPreviewID: "current",
    expiresAt: new Date("2026-10-14T12:00:00Z"),
    now: new Date("2026-10-14T11:00:00Z"),
    requiredOfferIDs: ["42"],
    acceptedOfferIDs: ["42"],
  };
  assert.doesNotThrow(() => assertSetupConfirmation(input));
  assert.throws(
    () => assertSetupConfirmation({ ...input, submittedRevision: 2 }),
    /changed/,
  );
  assert.throws(
    () => assertSetupConfirmation({ ...input, submittedPreviewID: "old" }),
    /changed/,
  );
  assert.throws(
    () =>
      assertSetupConfirmation({
        ...input,
        now: new Date("2026-10-14T12:00:00Z"),
      }),
    /expired/,
  );
  assert.throws(
    () => assertSetupConfirmation({ ...input, acceptedOfferIDs: [] }),
    /price and terms/,
  );
  assert.throws(
    () =>
      assertSetupConfirmation({
        ...input,
        acceptedOfferIDs: ["42", "foreign"],
      }),
    /price and terms/,
  );
});
