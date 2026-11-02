import test from "node:test";
import assert from "node:assert/strict";
import {
  createGitVercelObserver,
  gitReleaseHealthPath,
} from "../../src/modules/hosting/git-vercel-publication";
const credential = {
  token: "secret",
  teamID: "team_customer",
  configurationID: "icfg_customer",
  projectID: "prj_app",
  generation: 1,
};
const deployment = {
  id: "dpl_one",
  url: "app-123.vercel.app",
  projectId: "prj_app",
  ownerId: "team_customer",
  meta: { webdockReleaseID: "1" },
  prebuilt: true,
  target: "production",
  readyState: "READY",
  aliasAssigned: true,
  regions: ["fra1"],
  passiveRegions: [],
  functions: { "api/index": { runtime: "nodejs24.x", regions: ["fra1"] } },
};
test("resolves canonical deployment and verifies ready region and credential-free health", async () => {
  const observer = createGitVercelObserver(
    credential,
    async () => deployment,
    (async (input, init) => {
      assert.equal(String(input), "https://app-123.vercel.app/api/health");
      assert.equal(new Headers(init?.headers).has("authorization"), false);
      assert.equal(init?.redirect, "error");
      return new Response("ok");
    }) as typeof fetch,
  );
  assert.deepEqual(
    await observer.resolveDeployment({
      providerURL: "https://app-123.vercel.app",
      releaseID: "1",
    }),
    { deploymentID: "dpl_one", url: "https://app-123.vercel.app" },
  );
  const result = await observer.observeDeployment({
    deploymentID: "dpl_one",
    releaseID: "1",
    healthPath: gitReleaseHealthPath({ healthPath: "/api/health" }),
  });
  assert.equal(result.status, "ready");
  assert.equal(result.region, "fra1");
});
test("rejects cross-tenant/release identity and unsafe provider URLs", async () => {
  for (const change of [
    { projectId: "prj_other" },
    { ownerId: "team_other" },
    { meta: { webdockReleaseID: "2" } },
    { prebuilt: false },
    { url: "attacker.example" },
  ])
    await assert.rejects(
      createGitVercelObserver(credential, async () => ({
        ...deployment,
        ...change,
      })).resolveDeployment({
        providerURL: "https://app-123.vercel.app",
        releaseID: "1",
      }),
    );
  await assert.rejects(
    createGitVercelObserver(credential, async () => {
      throw Error("must not fetch");
    }).resolveDeployment({ providerURL: "https://127.0.0.1", releaseID: "1" }),
  );
});
test("never reports ready with non-EU functions, passive failover, missing alias or unhealthy app", async () => {
  for (const change of [
    { regions: ["iad1"] },
    { functions: { api: { regions: ["iad1"] } } },
    { passiveRegions: ["iad1"] },
    { aliasAssigned: false },
    { readyState: "BUILDING" },
  ]) {
    const result = await createGitVercelObserver(
      credential,
      async () => ({ ...deployment, ...change }),
      async () => new Response("ok"),
    ).observeDeployment({
      deploymentID: "dpl_one",
      releaseID: "1",
      healthPath: "/",
    });
    assert.notEqual(result.status, "ready");
  }
  const unhealthy = await createGitVercelObserver(
    credential,
    async () => deployment,
    async () => new Response(null, { status: 503 }),
  ).observeDeployment({
    deploymentID: "dpl_one",
    releaseID: "1",
    healthPath: "/",
  });
  assert.equal(unhealthy.status, "failed");
});
test("reconciles by persisted release identity and treats duplicate matches or incomplete search as ambiguous", async () => {
  for (const count of [0, 1, 2]) {
    const observer = createGitVercelObserver(credential, async (path) =>
      path.startsWith("/v6/")
        ? {
            deployments: Array.from({ length: count }, (_, i) => ({
              ...deployment,
              uid: "dpl_" + i,
            })),
            pagination: { next: null },
          }
        : { ...deployment, id: "dpl_0" },
    );
    const r = await observer.reconcileDeployment({ releaseID: "1", since: 1 });
    assert.equal(
      r.status,
      count === 0 ? "not-found" : count === 1 ? "found" : "ambiguous",
    );
  }
  const incomplete = await createGitVercelObserver(credential, async () => ({
    deployments: [],
    pagination: { next: 12 },
  })).reconcileDeployment({ releaseID: "1", since: 1 });
  assert.equal(incomplete.status, "ambiguous");
});

test("release health uses immutable snapshot and historical root fallback", () => {
  assert.equal(
    gitReleaseHealthPath({ healthPath: "/api/health" }),
    "/api/health",
  );
  assert.equal(gitReleaseHealthPath({}), "/");
  assert.equal(gitReleaseHealthPath(null), "/");
  for (const healthPath of [
    "//evil.test",
    "https://evil.test",
    "/bad?query",
    "/bad\\path",
  ])
    assert.throws(() => gitReleaseHealthPath({ healthPath }));
});

test("protected unique URL uses only a public Vercel alias pinned before and after health", async () => {
  const alias = {
    alias: "app.vercel.app",
    projectId: "prj_app",
    deploymentId: "dpl_one",
    uid: "alias-one",
    updatedAt: 123,
  };
  for (const change of [
    null,
    { deploymentId: "dpl_other" },
    { projectId: "prj_other" },
    { updatedAt: 124 },
    { uid: "recreated-alias" },
    { deletedAt: 1 },
    { redirect: "https://evil.example" },
    { updatedAt: undefined },
  ]) {
    let lookups = 0;
    const visited: string[] = [];
    const observer = createGitVercelObserver(
      credential,
      async (path) => {
        if (path.startsWith("/v13/")) return deployment;
        if (path.startsWith("/v2/"))
          return {
            aliases: [
              { alias: "127.0.0.1" },
              { alias: "evil.example" },
              { alias: "app.vercel.app" },
            ],
          };
        assert.equal(path, "/v4/aliases/app.vercel.app");
        lookups++;
        return lookups === 1 ? alias : { ...alias, ...change };
      },
      (async (input, init) => {
        visited.push(String(input));
        assert.equal(new Headers(init?.headers).has("authorization"), false);
        assert.equal(init?.redirect, "error");
        return new Response(null, {
          status: String(input).includes("app-123.") ? 302 : 200,
        });
      }) as typeof fetch,
    );
    const result = await observer.observeDeployment({
      deploymentID: "dpl_one",
      releaseID: "1",
      healthPath: "/api/health",
    });
    assert.equal(result.status, change ? "failed" : "ready");
    assert.deepEqual(visited, [
      "https://app-123.vercel.app/api/health",
      "https://app.vercel.app/api/health",
    ]);
  }
});

test("foreign alias is never requested even when returned in deployment alias listing", async () => {
  const visited: string[] = [];
  const observer = createGitVercelObserver(
    credential,
    async (path) =>
      path.startsWith("/v13/")
        ? deployment
        : path.startsWith("/v2/")
          ? { aliases: [{ alias: "other.vercel.app" }] }
          : {
              alias: "other.vercel.app",
              deploymentId: "dpl_other",
              projectId: "prj_other",
              uid: "other",
              updatedAt: 1,
            },
    (async (input) => {
      visited.push(String(input));
      return new Response(null, { status: 403 });
    }) as typeof fetch,
  );
  const result = await observer.observeDeployment({
    deploymentID: "dpl_one",
    releaseID: "1",
    healthPath: "/api/health",
  });
  assert.equal(result.status, "failed");
  assert.deepEqual(visited, ["https://app-123.vercel.app/api/health"]);
});

test("health requires HTTP 200 rather than other successful status codes", async () => {
  for (const status of [201, 202, 204]) {
    const observer = createGitVercelObserver(
      credential,
      async (path) =>
        path.startsWith("/v13/")
          ? deployment
          : path.startsWith("/v2/")
            ? { aliases: [{ alias: "app.vercel.app" }] }
            : {
                alias: "app.vercel.app",
                projectId: "prj_app",
                deploymentId: "dpl_one",
                uid: "alias",
                updatedAt: 1,
              },
      (async () => new Response(null, { status })) as typeof fetch,
    );
    assert.equal(
      (
        await observer.observeDeployment({
          deploymentID: "dpl_one",
          releaseID: "1",
          healthPath: "/api/health",
        })
      ).status,
      "failed",
    );
  }
});
