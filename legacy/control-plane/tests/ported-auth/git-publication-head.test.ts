import test from "node:test";
import assert from "node:assert/strict";
import { assertGitAutomaticSourceHead } from "../../src/modules/hosting/git-publication";

const source = { build_provider: "github-actions", branch: "main" };
const release = {
  approved_source: "git-policy",
  installation_id: "10",
  repository_id: "20",
  source_sha: "a".repeat(40),
};

test("automatic Actions publication rejects a branch advancing after import", async () => {
  await assert.rejects(
    assertGitAutomaticSourceHead(source, release, {
      resolveSource: async () => ({ sha: "b".repeat(40) }),
    }),
    { status: 409 },
  );
});

test("automatic publication resolves the exact configured source and accepts its head", async () => {
  let calls = 0;
  await assertGitAutomaticSourceHead(source, release, {
    resolveSource: async (input) => {
      calls++;
      assert.deepEqual(input, {
        installationID: "10",
        repositoryID: "20",
        branch: "main",
      });
      return { sha: release.source_sha };
    },
  });
  assert.equal(calls, 1);
});

test("manual approvals and rollback can intentionally publish an older commit", async () => {
  const denied = {
    resolveSource: async () => {
      throw Error("Must not look up current head");
    },
  };
  for (const approved_source of ["studio", "oauth"])
    await assertGitAutomaticSourceHead(
      source,
      { ...release, approved_source },
      denied,
    );
  await assertGitAutomaticSourceHead(
    { ...source, build_provider: "isolated" },
    release,
    denied,
  );
});

test("automatic publication fails closed when current head cannot be verified", async () => {
  await assert.rejects(
    assertGitAutomaticSourceHead(source, release, {
      resolveSource: async () => {
        throw Error("Repository access revoked");
      },
    }),
    /Repository access revoked/,
  );
});
