import test from "node:test";
import assert from "node:assert/strict";
import {
  canConfirmSetup,
  setupStage,
  defaultSetupConfiguration,
} from "../src/lib/project-setup-view";
test("deployment stays disabled while consent, prerequisites or the pending request are incomplete", () => {
  const state = {
    blockers: [],
    pending: false,
    requiredOfferIDs: ["42"],
    acceptedOfferIDs: [] as string[],
  };
  assert.equal(canConfirmSetup(state), false);
  assert.equal(canConfirmSetup({ ...state, acceptedOfferIDs: ["42"] }), true);
  assert.equal(
    canConfirmSetup({ ...state, acceptedOfferIDs: ["42"], pending: true }),
    false,
  );
  assert.equal(
    canConfirmSetup({
      ...state,
      acceptedOfferIDs: ["42"],
      blockers: ["Verify domain"],
    }),
    false,
  );
  assert.equal(
    canConfirmSetup({ ...state, acceptedOfferIDs: ["42", "42"] }),
    false,
  );
  assert.equal(setupStage("draft"), "edit");
  assert.equal(setupStage("awaiting-confirmation"), "review");
  assert.equal(setupStage("blocked"), "progress");
});
test("new drafts do not preselect paid services and avoid an existing application name", () => {
  const config = defaultSetupConfiguration(["web", "web-2"]);
  assert.equal(config.name, "web-3");
  assert.deepEqual(config.database, { mode: "none" });
  assert.equal(config.mail, null);
  assert.equal(config.autoPublish, false);
  assert.equal(config.source.kind, "git");
});
