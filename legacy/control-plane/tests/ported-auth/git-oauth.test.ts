import test from "node:test";
import assert from "node:assert/strict";
import { gitOAuthCommandSchema } from "@webdock/hosting-contracts/git-oauth";
import { commandSchema } from "@webdock/hosting-contracts";
import { gitOAuthSettings } from "../../src/modules/hosting/git-oauth";

test("Git OAuth is confined to the trusted interactive bridge", () => {
  const command = { action: "git.oauth.begin", customerID: "123" };
  assert.equal(gitOAuthCommandSchema.safeParse(command).success, true);
  assert.equal(commandSchema.safeParse(command).success, false);
  assert.equal(
    gitOAuthCommandSchema.safeParse({ ...command, subject: "operator" })
      .success,
    false,
  );
  assert.equal(
    gitOAuthCommandSchema.safeParse({
      action: "git.oauth.exchange",
      state: "short",
      code: "code",
    }).success,
    false,
  );
});

test("Git OAuth redirect comes only from trusted configured origins", () => {
  const base = {
    WEBDOCK_GITHUB_CLIENT_ID: "Iv1.fixture",
    WEBDOCK_GITHUB_APP_SLUG: "webdock",
    WEBDOCK_GIT_STUDIO_ORIGIN: "https://studio.example",
  };
  assert.equal(
    gitOAuthSettings(base).redirectURI,
    "https://studio.example/api/hosting/git/callback",
  );
  for (const origin of [
    "https://user:password@studio.example",
    "https://studio.example/evil",
    "https://studio.example?next=evil",
    "http://studio.example",
  ]) {
    assert.throws(() =>
      gitOAuthSettings({ ...base, WEBDOCK_GIT_STUDIO_ORIGIN: origin }),
    );
  }
});
