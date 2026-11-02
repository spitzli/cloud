import test from "node:test";
import assert from "node:assert/strict";
import {
  prepareGitVercelBuild,
  resolveGitVercelCredential,
} from "../../src/modules/hosting/git-vercel";
const credential = {
  token: "publisher-secret",
  teamID: "team_customer",
  configurationID: "icfg_customer",
  projectID: "prj_app",
  generation: 1,
};
const project = {
  id: "prj_app",
  accountId: "team_customer",
  link: null,
  functionDefaultRegions: ["fra1"],
  functionZeroConfigFailover: false,
  framework: "nextjs",
  nodeVersion: "24.x",
  buildCommand: "npm run build",
  rootDirectory: null,
};
const configuration = {
  id: "icfg_customer",
  teamId: "team_customer",
  scopes: [
    "read:project",
    "read-write:deployment",
    "read:integration-configuration",
  ],
  projects: ["prj_app"],
};
const fake =
  (overrides: Record<string, Record<string, unknown> | undefined> = {}) =>
  async (path: string, token: string, teamID: string) => {
    assert.equal(token, "publisher-secret");
    assert.equal(teamID, "team_customer");
    return (
      overrides[path] ??
      (path.includes("configuration") ? configuration : project)
    );
  };
test("prepares only allowed build values and canonical token-free Vercel project settings", async () => {
  const r = await prepareGitVercelBuild(
    credential,
    {
      buildEnvironment: { PUBLIC_ENDPOINT: "https://example.test" },
      allowedBuildVariables: ["PUBLIC_ENDPOINT"],
    },
    fake(),
  );
  assert.equal(r.vercelSettings.orgId, "team_customer");
  assert.equal(r.vercelSettings.projectId, "prj_app");
  assert.equal(r.vercelSettings.settings.nodeVersion, "24.x");
  assert.equal(JSON.stringify(r).includes("publisher-secret"), false);
  assert.deepEqual(r.buildEnvironment, {
    PUBLIC_ENDPOINT: "https://example.test",
  });
});
test("blocks native Git builds, cross-team project, missing deploy scopes, and non-EU function settings", async () => {
  for (const overrides of [
    { "/v9/projects/prj_app": { ...project, link: { type: "github" } } },
    { "/v9/projects/prj_app": { ...project, accountId: "team_other" } },
    {
      "/v9/projects/prj_app": { ...project, functionDefaultRegions: ["iad1"] },
    },
    {
      "/v1/integrations/configuration/icfg_customer": {
        ...configuration,
        scopes: ["read:project"],
      },
    },
    {
      "/v1/integrations/configuration/icfg_customer": {
        ...configuration,
        disabledAt: 123,
      },
    },
  ])
    await assert.rejects(
      prepareGitVercelBuild(
        credential,
        { buildEnvironment: {}, allowedBuildVariables: [] },
        fake(overrides),
      ),
    );
});
test("rejects provider credentials or unapproved variables at build boundary", async () => {
  for (const buildEnvironment of <Record<string, string>[]>[
    { VERCEL_TOKEN: "secret" },
    { UNAPPROVED: "value" },
  ])
    await assert.rejects(
      prepareGitVercelBuild(
        credential,
        { buildEnvironment, allowedBuildVariables: ["VERCEL_TOKEN"] },
        fake(),
      ),
    );
});
test("BYOK credential resolution never falls back to platform credentials and respects selected projects", async () => {
  const database = { query: async () => ({ rows: [] }) };
  await assert.rejects(
    resolveGitVercelCredential(
      database as never,
      { customerID: "1", targetID: "prj_app", mode: "byok" },
      () => ({ token: "publisher-secret", scopes: [] }),
    ),
  );
  const db = {
    query: async () => ({
      rows: [
        {
          customer_id: "1",
          team_id: "team_customer",
          configuration_id: "icfg_customer",
          projects: ["prj_app"],
          revision: 1,
          encrypted_token: "encrypted",
        },
      ],
    }),
  };
  const resolved = await resolveGitVercelCredential(
    db as never,
    { customerID: "1", targetID: "prj_app", mode: "byok" },
    (_cipher, purpose) => {
      assert.equal(purpose, "byok-vercel:1:team_customer");
      return { token: "publisher-secret", scopes: [] };
    },
  );
  assert.equal(resolved.token, "publisher-secret");
  await assert.rejects(
    resolveGitVercelCredential(
      db as never,
      { customerID: "1", targetID: "prj_other", mode: "byok" },
      () => ({ token: "publisher-secret", scopes: [] }),
    ),
  );
});
test("platform credential requires a matching persisted tenant/project target binding", async () => {
  const old = {
    token: process.env.WEBDOCK_GIT_VERCEL_TOKEN,
    team: process.env.WEBDOCK_GIT_VERCEL_TEAM_ID,
    config: process.env.WEBDOCK_GIT_VERCEL_CONFIGURATION_ID,
  };
  Object.assign(process.env, {
    WEBDOCK_GIT_VERCEL_TOKEN: "platform-secret",
    WEBDOCK_GIT_VERCEL_TEAM_ID: "team_platform",
    WEBDOCK_GIT_VERCEL_CONFIGURATION_ID: "icfg_platform",
  });
  try {
    const db = {
      query: async () => ({
        rows: [
          {
            customer_id: "1",
            project_id: "2",
            target_id: "prj_app",
            team_id: "team_platform",
            mode: "platform",
            generation: 3,
          },
        ],
      }),
    };
    const r = await resolveGitVercelCredential(db as never, {
      customerID: "1",
      projectID: "2",
      targetID: "prj_app",
      mode: "platform",
    });
    assert.equal(r.token, "platform-secret");
    assert.equal(r.generation, 3);
    await assert.rejects(
      resolveGitVercelCredential(db as never, {
        customerID: "9",
        projectID: "2",
        targetID: "prj_app",
        mode: "platform",
      }),
    );
  } finally {
    for (const [key, value] of Object.entries({
      WEBDOCK_GIT_VERCEL_TOKEN: old.token,
      WEBDOCK_GIT_VERCEL_TEAM_ID: old.team,
      WEBDOCK_GIT_VERCEL_CONFIGURATION_ID: old.config,
    })) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
});
test("reads real project resourceConfig and rejects unsafe nested settings without legacy fallback", async () => {
  const {
    functionDefaultRegions,
    functionZeroConfigFailover,
    ...actualProject
  } = project;
  const input = { buildEnvironment: {}, allowedBuildVariables: [] };
  const resourceConfig = { functionDefaultRegions, functionZeroConfigFailover };
  const nested = await prepareGitVercelBuild(
    credential,
    input,
    fake({
      "/v9/projects/prj_app": { ...actualProject, resourceConfig },
    }),
  );
  const legacy = await prepareGitVercelBuild(credential, input, fake());
  assert.deepEqual(nested.vercelSettings, legacy.vercelSettings);
  assert.equal(nested.configurationChecksum, legacy.configurationChecksum);
  for (const unsafe of [
    {},
    { functionDefaultRegions: ["iad1"], functionZeroConfigFailover: false },
    {
      functionDefaultRegions: ["fra1", "iad1"],
      functionZeroConfigFailover: false,
    },
    { functionDefaultRegions: ["fra1"], functionZeroConfigFailover: true },
    { functionDefaultRegions: ["fra1"] },
  ]) {
    await assert.rejects(
      prepareGitVercelBuild(
        credential,
        input,
        fake({
          "/v9/projects/prj_app": { ...project, resourceConfig: unsafe },
        }),
      ),
      /Frankfurt/,
    );
  }
  for (const wrongIdentity of [
    { id: "prj_other" },
    { accountId: "team_other" },
  ])
    await assert.rejects(
      prepareGitVercelBuild(
        credential,
        input,
        fake({
          "/v9/projects/prj_app": {
            ...actualProject,
            resourceConfig,
            ...wrongIdentity,
          },
        }),
      ),
      /could not be verified/,
    );
});
