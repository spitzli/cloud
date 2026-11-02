import test from "node:test";
import assert from "node:assert/strict";
import { createHmac, generateKeyPairSync, verify } from "node:crypto";
import {
  createGitHubProvider,
  parseGitHubWebhook,
} from "../../src/modules/hosting/git-github";
const privateKey = generateKeyPairSync("rsa", { modulusLength: 2048 })
  .privateKey.export({ type: "pkcs8", format: "pem" })
  .toString();
const config = {
  appID: "123",
  privateKey,
  clientID: "client",
  clientSecret: "secret",
};
test("OAuth permits loopback callbacks only outside production", async () => {
  const previous = process.env.NODE_ENV;
  try {
    Object.assign(process.env, { NODE_ENV: "development" });
    const f = fake({
      "/login/oauth/access_token": {
        access_token: "user-token",
        token_type: "bearer",
      },
    });
    const provider = createGitHubProvider(config, f.fetcher);
    assert.equal(
      await provider.exchangeOAuthCode(
        "code",
        "http://localhost:3120/api/hosting/git/callback",
      ),
      "user-token",
    );
    await assert.rejects(
      provider.exchangeOAuthCode("code", "http://evil.example/callback"),
    );
    await assert.rejects(
      provider.exchangeOAuthCode(
        "code",
        "http://user:pass@localhost:3120/callback",
      ),
    );
    Object.assign(process.env, { NODE_ENV: "production" });
    await assert.rejects(
      provider.exchangeOAuthCode(
        "code",
        "http://localhost:3120/api/hosting/git/callback",
      ),
    );
  } finally {
    if (previous === undefined)
      delete (process.env as Record<string, string | undefined>).NODE_ENV;
    else Object.assign(process.env, { NODE_ENV: previous });
  }
});
const repo = {
  id: 42,
  owner: { id: 7, login: "owner" },
  name: "repo",
  full_name: "owner/repo",
  default_branch: "main",
  permissions: { pull: true },
};
const installation = {
  id: 9,
  app_id: 123,
  account: { id: 7, login: "owner" },
  suspended_at: null,
  permissions: { metadata: "read", contents: "read", checks: "write" },
};
function fake(overrides: Record<string, unknown> = {}) {
  const calls: { path: string; init: RequestInit }[] = [];
  const fetcher = async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const path = new URL(String(input)).pathname;
    calls.push({ path, init });
    const responses: Record<string, unknown> = {
      "/user/installations": { installations: [installation] },
      "/app/installations/9": installation,
      "/user/installations/9/repositories": { repositories: [repo] },
      "/app/installations/9/access_tokens": {
        token: "installation-secret",
        expires_at: new Date(Date.now() + 3600000).toISOString(),
        permissions: { contents: "read", metadata: "read" },
        repositories: [repo],
      },
      "/repositories/42": repo,
      ["/repos/owner/repo/commits/" + "a".repeat(40) + "/check-runs"]: {
        check_runs: [],
      },
      "/repos/owner/repo/branches/main": {
        name: "main",
        commit: { sha: "a".repeat(40) },
      },
      ...overrides,
    };
    return Response.json(responses[path] ?? { message: "unexpected" }, {
      status: path in responses ? 200 : 404,
    });
  };
  return { calls, fetcher: fetcher as typeof fetch };
}
test("verifies intersection, signs App JWT, and requests exact read-only repository token", async () => {
  const f = fake();
  const p = createGitHubProvider(config, f.fetcher);
  const binding = await p.verifyRepository("user-secret", "9", "42");
  assert.equal(binding.fullName, "owner/repo");
  assert.equal(JSON.stringify(binding).includes("secret"), false);
  const resolved = await p.resolveSource({
    installationID: "9",
    repositoryID: "42",
    branch: "main",
  });
  assert.equal(resolved.sha, "a".repeat(40));
  const call = f.calls.find((c) => c.path.endsWith("access_tokens"))!;
  assert.deepEqual(JSON.parse(String(call.init.body)), {
    repository_ids: [42],
    permissions: { contents: "read", metadata: "read" },
  });
  const jwt = new Headers(call.init.headers).get("authorization")!.slice(7);
  const [h, payload, s] = jwt.split(".");
  assert.equal(
    verify(
      "RSA-SHA256",
      Buffer.from(h + "." + payload),
      privateKey,
      Buffer.from(s, "base64url"),
    ),
    true,
  );
  assert.equal(
    JSON.parse(Buffer.from(payload, "base64url").toString()).iss,
    "123",
  );
  assert.ok(f.calls.every((c) => c.init.redirect === "error" && c.init.signal));
});
test("rejects inaccessible user repositories and suspended/missing-permission installations", async () => {
  for (const overrides of [
    { "/user/installations/9/repositories": { repositories: [] } },
    { "/app/installations/9": { ...installation, suspended_at: "today" } },
    {
      "/app/installations/9": {
        ...installation,
        permissions: { metadata: "read" },
      },
    },
  ]) {
    await assert.rejects(
      createGitHubProvider(config, fake(overrides).fetcher).verifyRepository(
        "user-secret",
        "9",
        "42",
      ),
    );
  }
});
test("sanitizes HTTP failure and bounds provider responses", async () => {
  for (const response of [
    new Response("private-secret", { status: 500 }),
    new Response("x".repeat(2_000_001)),
  ]) {
    await assert.rejects(
      createGitHubProvider(config, async () => response).listUserInstallations(
        "user-secret",
      ),
      (e) => e instanceof Error && !e.message.includes("private-secret"),
    );
  }
});
function signed(payload: unknown, event = "push") {
  const raw = Buffer.from(JSON.stringify(payload));
  return {
    raw,
    headers: new Headers({
      "x-hub-signature-256":
        "sha256=" +
        createHmac("sha256", "hook-secret").update(raw).digest("hex"),
      "x-github-event": event,
      "x-github-delivery": "delivery-123",
    }),
  };
}
test("validates raw HMAC and projects only minimal signed push data", () => {
  const s = signed({
    installation: { id: 9 },
    repository: { id: 42 },
    ref: "refs/heads/main",
    after: "a".repeat(40),
    deleted: false,
    secret: "do-not-retain",
  });
  assert.deepEqual(parseGitHubWebhook(s.raw, s.headers, "hook-secret"), {
    deliveryID: "delivery-123",
    event: "push",
    installationID: "9",
    repositoryID: "42",
    branch: "main",
    sha: "a".repeat(40),
    deleted: false,
  });
  assert.throws(() =>
    parseGitHubWebhook(
      Buffer.from(s.raw.toString() + " "),
      s.headers,
      "hook-secret",
    ),
  );
  assert.throws(() =>
    parseGitHubWebhook(Buffer.alloc(2_000_001), s.headers, "hook-secret"),
  );
  const bad = signed({
    installation: { id: 9 },
    repository: { id: 42 },
    ref: "refs/heads/main",
    after: "not-sha",
  });
  assert.throws(() => parseGitHubWebhook(bad.raw, bad.headers, "hook-secret"));
});
test("fetches exact archive without forwarding credentials to codeload, blocks off-origin redirects", async () => {
  for (const location of [
    "https://codeload.github.com/owner/repo/legacy.tar.gz/" + "a".repeat(40),
    "https://attacker.example/steal",
  ]) {
    const f = fake();
    let download = false;
    const p = createGitHubProvider(config, (async (input, init) => {
      const url = new URL(String(input));
      if (url.pathname.includes("/tarball/"))
        return new Response(null, { status: 302, headers: { location } });
      if (url.hostname === "codeload.github.com") {
        download = true;
        assert.equal(new Headers(init?.headers).has("authorization"), false);
        return new Response("archive");
      }
      return f.fetcher(input, init);
    }) as typeof fetch);
    if (location.includes("attacker"))
      await assert.rejects(p.fetchSource("9", "42", "a".repeat(40)));
    else {
      const result = await p.fetchSource("9", "42", "a".repeat(40));
      assert.equal(result.bytes.toString(), "archive");
      assert.equal(result.sha, "a".repeat(40));
      assert.equal(download, true);
    }
  }
});
test("check writes request checks-only permissions and return stable ID", async () => {
  const f = fake({
    "/app/installations/9/access_tokens": {
      token: "check-secret",
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      permissions: { checks: "write", metadata: "read" },
      repositories: [repo],
    },
    "/repos/owner/repo/check-runs": { id: 99 },
  });
  const result = await createGitHubProvider(config, f.fetcher).reportCheck({
    installationID: "9",
    repositoryID: "42",
    sha: "a".repeat(40),
    externalID: "build-1",
    status: "completed",
    conclusion: "success",
  });
  assert.equal(result.checkID, "99");
  assert.deepEqual(
    JSON.parse(
      String(f.calls.find((c) => c.path.endsWith("/access_tokens"))!.init.body),
    ).permissions,
    { checks: "write", metadata: "read" },
  );
});
test("rejects escalated/multiple repository tokens and mismatched branch or stable repository IDs", async () => {
  for (const overrides of [
    {
      "/app/installations/9/access_tokens": {
        token: "secret",
        expires_at: new Date(Date.now() + 3600000).toISOString(),
        permissions: { contents: "write", metadata: "read" },
        repositories: [repo],
      },
    },
    { "/repositories/42": { ...repo, id: 43 } },
    {
      "/repos/owner/repo/branches/main": {
        name: "other",
        commit: { sha: "a".repeat(40) },
      },
    },
  ])
    await assert.rejects(
      createGitHubProvider(config, fake(overrides).fetcher).resolveSource({
        installationID: "9",
        repositoryID: "42",
        branch: "main",
      }),
    );
});
test("OAuth exchange uses private server POST and never returns refresh credentials", async () => {
  const provider = createGitHubProvider(config, (async (input, init) => {
    assert.equal(String(input), "https://github.com/login/oauth/access_token");
    assert.equal(init?.method, "POST");
    assert.equal(
      JSON.parse(String(init?.body)).redirect_uri,
      "https://studio.example/callback",
    );
    return Response.json({
      access_token: "user-token",
      refresh_token: "do-not-retain",
      token_type: "bearer",
    });
  }) as typeof fetch);
  assert.equal(
    await provider.exchangeOAuthCode("code", "https://studio.example/callback"),
    "user-token",
  );
});
test("projects lifecycle invalidations and rejects unknown signed event actions", () => {
  const s = signed(
    {
      installation: { id: 9 },
      action: "removed",
      repositories_removed: [{ id: 42 }],
      repositories_added: [],
    },
    "installation_repositories",
  );
  assert.deepEqual(parseGitHubWebhook(s.raw, s.headers, "hook-secret"), {
    deliveryID: "delivery-123",
    event: "installation_repositories",
    installationID: "9",
    action: "removed",
    repositoryIDs: ["42"],
  });
  const bad = signed(
    { installation: { id: 9 }, action: "unrecognized" },
    "installation",
  );
  assert.throws(() => parseGitHubWebhook(bad.raw, bad.headers, "hook-secret"));
});
test("prepares a trusted codeload descriptor without downloading source or exposing installation token", async () => {
  const f = fake();
  const p = createGitHubProvider(config, (async (input, init) => {
    const url = new URL(String(input));
    if (url.pathname.includes("/tarball/"))
      return new Response(null, {
        status: 302,
        headers: {
          location:
            "https://codeload.github.com/owner/repo/legacy.tar.gz/" +
            "a".repeat(40),
        },
      });
    assert.notEqual(url.hostname, "codeload.github.com");
    return f.fetcher(input, init);
  }) as typeof fetch);
  const descriptor = await p.prepareSourceDownload("9", "42", "a".repeat(40));
  assert.equal(new URL(descriptor.url).hostname, "codeload.github.com");
  assert.equal(
    JSON.stringify(descriptor).includes("installation-secret"),
    false,
  );
});
test("sanitizes source streaming errors containing private download details", async () => {
  const f = fake();
  const p = createGitHubProvider(config, (async (input, init) => {
    const url = new URL(String(input));
    if (url.pathname.includes("/tarball/"))
      return new Response(null, {
        status: 302,
        headers: { location: "https://codeload.github.com/owner/repo/archive" },
      });
    if (url.hostname === "codeload.github.com")
      return new Response(
        new ReadableStream({
          start(controller) {
            controller.error(new Error("private-signed-url-secret"));
          },
        }),
      );
    return f.fetcher(input, init);
  }) as typeof fetch);
  await assert.rejects(
    p.fetchSource("9", "42", "a".repeat(40)),
    (e) =>
      e instanceof Error && !e.message.includes("private-signed-url-secret"),
  );
});
test("reconciles an ambiguous check creation before any repeated write", async () => {
  const f = fake({
    "/app/installations/9/access_tokens": {
      token: "check-secret",
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      permissions: { checks: "write", metadata: "read" },
      repositories: [repo],
    },
    ["/repos/owner/repo/commits/" + "a".repeat(40) + "/check-runs"]: {
      check_runs: [
        {
          id: 99,
          external_id: "build-1",
          head_sha: "a".repeat(40),
          app: { id: 123 },
        },
      ],
    },
    "/repos/owner/repo/check-runs/99": {
      id: 99,
      external_id: "build-1",
      head_sha: "a".repeat(40),
      app: { id: 123 },
    },
  });
  const result = await createGitHubProvider(config, f.fetcher).reportCheck({
    installationID: "9",
    repositoryID: "42",
    sha: "a".repeat(40),
    externalID: "build-1",
    status: "completed",
    conclusion: "success",
  });
  assert.equal(result.checkID, "99");
  assert.equal(
    f.calls.some(
      (c) => c.path.endsWith("/check-runs") && c.init.method === "POST",
    ),
    false,
  );
  assert.equal(
    f.calls.some(
      (c) => c.path.endsWith("/check-runs/99") && c.init.method === "PATCH",
    ),
    true,
  );
});

test("default configuration accepts canonical App private key and legacy fallback", async () => {
  const keys = [
    "WEBDOCK_GITHUB_APP_ID",
    "WEBDOCK_GITHUB_APP_PRIVATE_KEY",
    "WEBDOCK_GITHUB_PRIVATE_KEY",
  ] as const;
  const previous = keys.map((key) => process.env[key]);
  try {
    process.env.WEBDOCK_GITHUB_APP_ID = config.appID;
    process.env.WEBDOCK_GITHUB_APP_PRIVATE_KEY = privateKey;
    process.env.WEBDOCK_GITHUB_PRIVATE_KEY =
      "invalid legacy key must not override canonical";
    await createGitHubProvider(undefined, fake().fetcher).revalidateRepository(
      "9",
      "42",
    );
    delete process.env.WEBDOCK_GITHUB_APP_PRIVATE_KEY;
    process.env.WEBDOCK_GITHUB_PRIVATE_KEY = privateKey;
    await createGitHubProvider(undefined, fake().fetcher).revalidateRepository(
      "9",
      "42",
    );
  } finally {
    keys.forEach((key, i) => {
      if (previous[i] === undefined) delete process.env[key];
      else process.env[key] = previous[i];
    });
  }
});

test("Actions artifacts bind exact workflow, repository, successful first attempt and immutable digest", async () => {
  const revision = "a".repeat(40),
    workflowPath = ".github/workflows/production-build.yml";
  const input = {
    installationID: "9",
    repositoryID: "42",
    workflowPath,
    runID: "81",
    runAttempt: 1,
    branch: "main",
    sha: revision,
    artifactName: "lunares-bot-" + revision,
  };
  const run = {
    id: 81,
    run_attempt: 1,
    workflow_id: 71,
    path: workflowPath,
    repository: { id: 42 },
    head_repository: { id: 42 },
    head_branch: "main",
    head_sha: revision,
    event: "push",
    status: "completed",
    conclusion: "success",
    created_at: "2026-10-10T10:00:00Z",
  };
  const artifact = {
    id: 91,
    name: input.artifactName,
    expired: false,
    digest: "sha256:" + "b".repeat(64),
    size_in_bytes: 100,
    created_at: "2026-10-10T10:01:00Z",
    workflow_run: {
      id: 81,
      repository_id: 42,
      head_repository_id: 42,
      head_sha: revision,
    },
  };
  function provider(
    runChanges = {},
    artifactChanges = {},
    redirect = "https://productionresult.blob.core.windows.net/secret?sig=capability",
  ) {
    const f = fake({
      "/app/installations/9": {
        ...installation,
        permissions: { ...installation.permissions, actions: "read" },
      },
      "/app/installations/9/access_tokens": {
        token: "actions-secret",
        expires_at: new Date(Date.now() + 3600000).toISOString(),
        permissions: { metadata: "read", actions: "read" },
        repositories: [repo],
      },
      "/repos/owner/repo/actions/workflows/production-build.yml": {
        id: 71,
        path: workflowPath,
        state: "active",
      },
      "/repos/owner/repo/actions/runs/81/attempts/1": { ...run, ...runChanges },
      "/repos/owner/repo/actions/runs/81": { ...run, ...runChanges },
      "/repos/owner/repo/actions/runs/81/artifacts": {
        artifacts: [{ ...artifact, ...artifactChanges }],
      },
    });
    return {
      calls: f.calls,
      api: createGitHubProvider(config, async (url, init) =>
        new URL(String(url)).pathname.endsWith("/91/zip")
          ? new Response(null, { status: 302, headers: { location: redirect } })
          : f.fetcher(url, init),
      ),
    };
  }
  const valid = provider();
  const verified = await valid.api.verifyActionsRun(input);
  assert.equal(verified.artifact.id, "91");
  assert.equal(verified.artifact.repository, "owner/repo");
  const tokenRequest = valid.calls.find((c) =>
    c.path.endsWith("/access_tokens"),
  )!;
  assert.deepEqual(JSON.parse(String(tokenRequest.init.body)).permissions, {
    actions: "read",
    metadata: "read",
  });
  for (const change of [
    { head_repository: { id: 99 } },
    { head_sha: "c".repeat(40) },
    { event: "pull_request" },
    { run_attempt: 2 },
    { workflow_id: 72 },
    { conclusion: "failure" },
  ])
    await assert.rejects(provider(change).api.verifyActionsRun(input));
  for (const change of [
    { expired: true },
    { digest: null },
    { workflow_run: { ...artifact.workflow_run, repository_id: 99 } },
    { size_in_bytes: 700_000_000 },
  ])
    await assert.rejects(provider({}, change).api.verifyActionsRun(input));
  await assert.rejects(
    provider({}, {}, "https://evil.example/artifact").api.verifyActionsRun(
      input,
    ),
  );
  await assert.rejects(
    provider().api.verifyActionsRun({ ...input, runAttempt: 2 }),
  );
});

test("signed workflow completion is parsed independently from installation changes", () => {
  const payload = {
    action: "completed",
    installation: { id: 9 },
    repository: { id: 42 },
    workflow_run: {
      id: 81,
      run_attempt: 1,
      path: ".github/workflows/production-build.yml",
      conclusion: "success",
      head_repository: { id: 42 },
      head_branch: "main",
      head_sha: "a".repeat(40),
    },
  };
  const raw = Buffer.from(JSON.stringify(payload));
  const headers = new Headers({
    "x-github-event": "workflow_run",
    "x-github-delivery": "workflow-81",
    "x-hub-signature-256":
      "sha256=" + createHmac("sha256", "secret").update(raw).digest("hex"),
  });
  const event = parseGitHubWebhook(raw, headers, "secret");
  assert.equal(event.event, "workflow_run");
  assert.equal("runID" in event && event.runID, "81");
});

test("repository binding persists granted Actions permission without breaking metadata connections", async () => {
  const f = fake({
    "/app/installations/9": {
      ...installation,
      permissions: { ...installation.permissions, actions: "read" },
    },
  });
  const binding = await createGitHubProvider(
    config,
    f.fetcher,
  ).verifyRepository("user-token", "9", "42");
  assert.equal(binding.permissions.actions, "read");
});

test("setup reads bounded files at the reviewed commit and rejects directories and escapes", async () => {
  const commit = "b".repeat(40);
  const f = fake({
    ["/repos/owner/repo/commits/" + commit]: { sha: commit },
    "/repos/owner/repo/contents/apps/web/Dockerfile": {
      type: "file",
      encoding: "base64",
      size: 12,
      content: Buffer.from("EXPOSE 3000\n").toString("base64"),
    },
  });
  const p = createGitHubProvider(config, async (input, init) => {
    const url = new URL(String(input));
    if (url.pathname.includes("/contents/"))
      assert.equal(url.searchParams.get("ref"), commit);
    return f.fetcher(input, init);
  });
  assert.equal(
    (
      await p.resolveCommit(
        { installationID: "9", repositoryID: "42", branch: "main" },
        commit,
      )
    ).sha,
    commit,
  );
  assert.equal(
    await p.readSourceFile(
      { installationID: "9", repositoryID: "42", branch: "main" },
      commit,
      "apps/web/Dockerfile",
    ),
    "EXPOSE 3000\n",
  );
  await assert.rejects(
    p.readSourceFile(
      { installationID: "9", repositoryID: "42", branch: "main" },
      commit,
      "../secret",
    ),
  );
  const bad = createGitHubProvider(
    config,
    fake({
      "/repos/owner/repo/contents/Dockerfile": {
        type: "symlink",
        target: "../../secret",
        size: 12,
        content: "AAAA",
      },
    }).fetcher,
  );
  await assert.rejects(
    bad.readSourceFile(
      { installationID: "9", repositoryID: "42", branch: "main" },
      commit,
      "Dockerfile",
    ),
  );
});
