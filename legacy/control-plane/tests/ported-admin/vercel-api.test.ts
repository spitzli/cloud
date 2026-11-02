import assert from "node:assert/strict";
import { test } from "node:test";
import {
  fetchVercelProject,
  listVercelProjects,
  VercelAPIError,
} from "../../src/modules/integrations/vercel/vercel-api";
const options = {
  token: "test-token",
  teamID: "team_test123",
  projectID: "prj_test123",
};
const deployment = {
  uid: "dpl_one",
  projectId: options.projectID,
  state: "READY",
  url: "demo-abc.vercel.app",
  target: "production",
  created: 1_800_000_000_000,
  ready: 1_800_000_005_000,
  meta: {
    githubCommitRef: "main",
    githubCommitSha: "a".repeat(40),
    githubCommitMessage: "Release",
    SECRET: "never-return",
  },
};
const project = {
  id: options.projectID,
  accountId: options.teamID,
  name: "demo",
  framework: "nextjs",
  nodeVersion: "24.x",
  env: [{ key: "SECRET", value: "never-return" }],
  targets: {
    production: { ...deployment, id: deployment.uid, readyState: "READY" },
  },
};
function mock(
  overrides: {
    project?: unknown;
    deployments?: unknown;
    domains?: unknown;
  } = {},
  seen: URL[] = [],
): typeof fetch {
  return async (input, init) => {
    const url = new URL(String(input));
    seen.push(url);
    assert.equal(url.origin, "https://api.vercel.com");
    assert.equal(url.searchParams.get("teamId"), options.teamID);
    assert.equal(init?.method, "GET");
    assert.equal(init?.redirect, "error");
    assert.equal(init?.cache, "no-store");
    assert.equal(
      new Headers(init?.headers).get("Authorization"),
      `Bearer ${options.token}`,
    );
    assert.ok(init?.signal instanceof AbortSignal);
    if (url.pathname.endsWith("/domains"))
      return Response.json(
        overrides.domains ?? {
          domains: [
            {
              name: "demo.example.com",
              verified: true,
              redirect: null,
              verification: [{ value: "never-return" }],
            },
          ],
        },
      );
    if (url.pathname === "/v7/deployments") {
      assert.equal(url.searchParams.get("projectId"), options.projectID);
      assert.equal(url.searchParams.get("limit"), "5");
      return Response.json(
        overrides.deployments ?? { deployments: [deployment] },
      );
    }
    assert.equal(url.pathname, `/v9/projects/${options.projectID}`);
    return Response.json(overrides.project ?? project);
  };
}
const code = (expected: VercelAPIError["code"]) => (error: unknown) =>
  error instanceof VercelAPIError &&
  error.code === expected &&
  !/never-return|test-token/.test(error.message);
test("GET-only scoped snapshots expose only explicit fields and canonical dates", async () => {
  const seen: URL[] = [];
  const result = await fetchVercelProject({
    ...options,
    fetcher: mock({}, seen),
  });
  assert.equal(seen.length, 3);
  assert.equal(result.projectID, options.projectID);
  assert.equal(result.productionDeployment?.id, "dpl_one");
  assert.equal(result.deployments[0].url, "https://demo-abc.vercel.app/");
  assert.equal(
    result.deployments[0].createdAt,
    new Date(deployment.created).toISOString(),
  );
  assert.equal(result.deployments[0].commitSHA, "a".repeat(40));
  assert.deepEqual(result.domains, [
    { name: "demo.example.com", verified: true, redirect: null },
  ]);
  assert.ok(Number.isFinite(Date.parse(result.checkedAt)));
  assert.doesNotMatch(
    JSON.stringify(result),
    /never-return|verification|"env"|SECRET/,
  );
});
test("invalid mapping IDs and absent or injected credentials never fetch", async () => {
  let calls = 0;
  const fetcher: typeof fetch = async () => {
    calls++;
    throw Error("unexpected");
  };
  for (const projectID of [
    "",
    "https://evil.test",
    "prj_x/../../env",
    "prj_x?teamId=team_other",
    "prj_%2f",
    "prj_x\n",
    "prj_" + "x".repeat(129),
  ])
    await assert.rejects(
      fetchVercelProject({ ...options, projectID, fetcher }),
      code("not_found"),
    );
  for (const teamID of [
    "",
    "user_123",
    "team_x&projectId=prj_other",
    "team_x\\evil",
  ])
    await assert.rejects(
      fetchVercelProject({ ...options, teamID, fetcher }),
      code("forbidden"),
    );
  for (const token of ["", "bad\r\nAuthorization: other", "with space"])
    await assert.rejects(
      fetchVercelProject({ ...options, token, fetcher }),
      code("disconnected"),
    );
  assert.equal(calls, 0);
});
test("HTTP, network and parser errors are classified without upstream text", async () => {
  for (const [status, expected] of [
    [401, "disconnected"],
    [403, "forbidden"],
    [404, "not_found"],
    [429, "unavailable"],
    [500, "unavailable"],
    [302, "unavailable"],
  ] as const)
    await assert.rejects(
      fetchVercelProject({
        ...options,
        fetcher: async () =>
          new Response("never-return test-token", { status }),
      }),
      code(expected),
    );
  for (const fetcher of [
    async () => {
      throw Error("never-return test-token");
    },
    async () => new Response("not JSON never-return"),
  ])
    await assert.rejects(
      fetchVercelProject({ ...options, fetcher }),
      code("unavailable"),
    );
});
test("malformed and mismatched identities fail closed", async () => {
  for (const invalid of [{}, [], { ...project, name: null }])
    await assert.rejects(
      fetchVercelProject({ ...options, fetcher: mock({ project: invalid }) }),
      code("unavailable"),
    );
  for (const invalid of [
    { ...project, id: "prj_other" },
    { ...project, accountId: "team_other" },
  ])
    await assert.rejects(
      fetchVercelProject({ ...options, fetcher: mock({ project: invalid }) }),
      code("forbidden"),
    );
  for (const overrides of [
    { deployments: {} },
    { domains: {} },
    { deployments: { deployments: [null] } },
    { domains: { domains: [{ name: "https://evil.test" }] } },
  ])
    await assert.rejects(
      fetchVercelProject({ ...options, fetcher: mock(overrides) }),
      code("unavailable"),
    );
  await assert.rejects(
    fetchVercelProject({
      ...options,
      fetcher: mock({
        deployments: {
          deployments: [{ ...deployment, projectId: "prj_other" }],
        },
      }),
    }),
    code("forbidden"),
  );
});
test("unsafe links, unknown status, invalid timestamps and metadata are sanitized", async () => {
  for (const url of [
    "javascript:alert(1)",
    "http://demo.vercel.app",
    "https://user:password@demo.vercel.app",
    "https://127.0.0.1",
    "//evil.test",
    "https://localhost",
    "https://demo.vercel.app/?secret=x",
    "https://demo.vercel.app\\@evil.test",
  ]) {
    const result = await fetchVercelProject({
      ...options,
      fetcher: mock({
        deployments: {
          deployments: [
            {
              ...deployment,
              url,
              state: "INVENTED",
              ready: -1,
              created: {},
              meta: {
                githubCommitRef: "x\n".repeat(400),
                githubCommitSha: "bad <sha>",
                githubCommitMessage: "<script>\u0000" + "x".repeat(1000),
              },
            },
          ],
        },
      }),
    });
    const d = result.deployments[0];
    assert.equal(d.url, null);
    assert.equal(d.status, "UNKNOWN");
    assert.equal(d.readyAt, null);
    assert.equal(d.createdAt, null);
    assert.equal(d.commitSHA, null);
    assert.ok((d.branch?.length || 0) <= 200);
    assert.ok((d.commitMessage?.length || 0) <= 500);
    assert.doesNotMatch(d.commitMessage || "", /[<>\u0000]/);
  }
});
test("five newest deployments are retained without guessing current production", async () => {
  const docs = Array.from({ length: 7 }, (_, index) => ({
    ...deployment,
    uid: `dpl_${index}`,
    created: deployment.created + index,
    state: index === 6 ? "BLOCKED" : "READY",
  }));
  const result = await fetchVercelProject({
    ...options,
    fetcher: mock({
      project: { ...project, targets: {} },
      deployments: { deployments: docs },
    }),
  });
  assert.equal(result.deployments.length, 5);
  assert.equal(result.deployments[0].id, "dpl_6");
  assert.equal(result.deployments[0].status, "BLOCKED");
  assert.equal(result.productionDeployment, undefined);
});
test("domains pagination stays on fixed origin and validates cursors", async () => {
  const original = mock();
  let pages = 0;
  const fetcher: typeof fetch = async (input, init) => {
    const url = new URL(String(input));
    if (!url.pathname.endsWith("/domains")) return original(input, init);
    pages++;
    assert.equal(url.origin, "https://api.vercel.com");
    if (pages === 1)
      return Response.json({
        domains: [{ name: "example.com", verified: true }],
        pagination: { next: 1234 },
      });
    assert.equal(url.searchParams.get("until"), "1234");
    return Response.json({
      domains: [
        { name: "www.example.com", verified: false, redirect: "example.com" },
      ],
      pagination: { next: null },
    });
  };
  const result = await fetchVercelProject({ ...options, fetcher });
  assert.equal(result.domains.length, 2);
  await assert.rejects(
    fetchVercelProject({
      ...options,
      fetcher: mock({
        domains: { domains: [], pagination: { next: "https://evil.test" } },
      }),
    }),
    code("unavailable"),
  );
});
test("timeouts and oversized bodies are unavailable without leaking details", async () => {
  const timeout: typeof fetch = async (_input, init) => {
    assert.ok(init?.signal);
    throw new DOMException("never-return", "TimeoutError");
  };
  await assert.rejects(
    fetchVercelProject({ ...options, fetcher: timeout }),
    code("unavailable"),
  );
  await assert.rejects(
    fetchVercelProject({
      ...options,
      fetcher: async () =>
        new Response("x", { headers: { "content-length": "999999999" } }),
    }),
    code("unavailable"),
  );
});

test("redirected responses, streaming oversize and repeated pagination cursors are rejected", async () => {
  for (const altered of [
    { url: "https://evil.example/secret" },
    { redirected: true },
  ]) {
    const response = Response.json(project);
    for (const [key, value] of Object.entries(altered))
      Object.defineProperty(response, key, { value });
    await assert.rejects(
      fetchVercelProject({ ...options, fetcher: async () => response }),
      code("unavailable"),
    );
  }
  const oversized = new Response(new Uint8Array(4 * 1024 * 1024 + 1));
  await assert.rejects(
    fetchVercelProject({ ...options, fetcher: async () => oversized }),
    code("unavailable"),
  );
  await assert.rejects(
    fetchVercelProject({
      ...options,
      fetcher: mock({ domains: { domains: [], pagination: { next: 123 } } }),
    }),
    code("unavailable"),
  );
});

test("unfinished deployments and other Git providers keep optional fields accurate", async () => {
  const result = await fetchVercelProject({
    ...options,
    fetcher: mock({
      deployments: {
        deployments: [
          {
            ...deployment,
            url: null,
            target: null,
            state: "BUILDING",
            ready: null,
            meta: {
              gitlabCommitRef: "release",
              gitlabCommitSha: "b".repeat(40),
              gitlabCommitMessage: "Fix build",
            },
          },
        ],
      },
      domains: {
        domains: [
          {
            name: "*.example.com",
            verified: "true",
            redirect: "javascript:alert(1)",
          },
        ],
      },
    }),
  });
  assert.equal(result.deployments[0].status, "BUILDING");
  assert.equal(result.deployments[0].readyAt, null);
  assert.equal(result.deployments[0].url, null);
  assert.equal(result.deployments[0].target, null);
  assert.equal(result.deployments[0].branch, "release");
  assert.equal(result.deployments[0].commitSHA, "b".repeat(40));
  assert.deepEqual(result.domains, [
    { name: "*.example.com", verified: false, redirect: null },
  ]);
});

test("project picker returns one scoped page, validated continuation and no secrets", async () => {
  let calls = 0;
  const fetcher: typeof fetch = async (input, init) => {
    calls++;
    const url = new URL(String(input));
    assert.equal(url.origin, "https://api.vercel.com");
    assert.equal(url.pathname, "/v10/projects");
    assert.equal(url.searchParams.get("teamId"), options.teamID);
    assert.equal(url.searchParams.get("limit"), "100");
    assert.equal(url.searchParams.get("from"), "9999");
    assert.equal(init?.method, "GET");
    assert.equal(init?.redirect, "error");
    return Response.json({ projects: [project], pagination: { next: 1234 } });
  };
  const result = await listVercelProjects({
    ...options,
    cursor: "9999",
    fetcher,
  });
  assert.equal(calls, 1);
  assert.deepEqual(result, {
    projects: [{ id: options.projectID, name: "demo" }],
    nextCursor: "1234",
  });
  assert.doesNotMatch(
    JSON.stringify(result),
    /never-return|env|targets|accountId/,
  );
});

test("project picker rejects malicious cursors, invalid records and cross-team results", async () => {
  let calls = 0;
  for (const cursor of [
    "",
    "https://evil.test",
    "1&teamId=team_other",
    "-1",
    "1e3",
    "9".repeat(20),
  ]) {
    await assert.rejects(
      listVercelProjects({
        ...options,
        cursor,
        fetcher: async () => {
          calls++;
          return Response.json({ projects: [] });
        },
      }),
      code("unavailable"),
    );
  }
  assert.equal(calls, 0);
  for (const result of [
    { projects: [{ ...project, id: "prj_x/evil" }] },
    { projects: [], pagination: { next: "https://evil.test" } },
    { projects: Array(101).fill(project) },
    { projects: [null] },
  ])
    await assert.rejects(
      listVercelProjects({
        ...options,
        fetcher: async () => Response.json(result),
      }),
      code("unavailable"),
    );
  await assert.rejects(
    listVercelProjects({
      ...options,
      fetcher: async () =>
        Response.json({ projects: [{ ...project, accountId: "team_other" }] }),
    }),
    code("forbidden"),
  );
  await assert.rejects(
    listVercelProjects({
      ...options,
      fetcher: async () => new Response("never-return", { status: 401 }),
    }),
    code("disconnected"),
  );
  await assert.rejects(
    listVercelProjects({
      ...options,
      fetcher: async () => {
        throw Error("never-return");
      },
    }),
    code("unavailable"),
  );
});

test("documented v10 Base32 cursors and unpaginated array responses are supported", async () => {
  const token = "JBSWY3DPEHPK3PXP";
  const result = await listVercelProjects({
    ...options,
    fetcher: async () =>
      Response.json({ projects: [project], pagination: { next: token } }),
  });
  assert.equal(result.nextCursor, token);
  const next = await listVercelProjects({
    ...options,
    cursor: token,
    fetcher: async (input) => {
      assert.equal(new URL(String(input)).searchParams.get("from"), token);
      return Response.json([project]);
    },
  });
  assert.deepEqual(next, {
    projects: [{ id: options.projectID, name: "demo" }],
    nextCursor: null,
  });
});
