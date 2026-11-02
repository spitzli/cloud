import assert from "node:assert/strict";
import test, { type TestContext, afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import { createHash } from "node:crypto";
import { sealData, unsealData } from "iron-session";
import { configureGitHub } from "../../src/modules/integrations/github/client";
afterEach(restoreMocks);

const options = {
  clientID: "Iv1.webdock",
  clientSecret: "client-secret",
  appSlug: "webdock-studio",
  origin: "https://admin.example.test",
  cookieSecret: "cookie-secret-with-at-least-thirty-two-characters",
};
const actor = "1001";
const cookieHeader = (response: Response) =>
  response.headers
    .getSetCookie()
    .filter((c) => !c.includes("Max-Age=0"))
    .map((c) => c.split(";")[0])
    .join("; ");
const request = (path: string, cookie = "", method = "GET") =>
  new Request(`${options.origin}${path}`, {
    method,
    headers: { cookie, origin: options.origin },
  });

test("GitHub state uses PKCE and secure cookies, is owner bound, and rejects cross-origin connect", async () => {
  const github = configureGitHub(options);
  assert.equal(
    (
      await github.connect(
        new Request(`${options.origin}/api/github/connect`, {
          method: "POST",
          headers: { origin: "https://evil.test" },
        }),
        actor,
      )
    ).status,
    403,
  );
  const start = await github.connect(
    request("/api/github/connect", "", "POST"),
    actor,
  );
  assert.equal(start.status, 303);
  const url = new URL(start.headers.get("location")!);
  assert.equal(url.origin, "https://github.com");
  assert.equal(url.searchParams.get("code_challenge_method"), "S256");
  assert.equal(url.searchParams.has("scope"), false);
  assert.match(
    start.headers.get("set-cookie")!,
    /^__Host-webdock-github-flow=/,
  );
  assert.match(
    start.headers.get("set-cookie")!,
    /HttpOnly; Secure; SameSite=Lax/,
  );
  assert.ok(!start.headers.get("set-cookie")!.includes("Domain="));
  const callback = request(
    `/api/github/callback?code=code&state=${url.searchParams.get("state")}`,
    cookieHeader(start),
  );
  assert.match(
    (await github.callback(callback, "another-operator")).headers.get(
      "location",
    )!,
    /github=failed$/,
  );
  assert.match(
    (
      await github.callback(
        request(
          "/api/github/callback?code=code&state=wrong",
          cookieHeader(start),
        ),
        actor,
      )
    ).headers.get("location")!,
    /github=failed$/,
  );
});

test("GitHub user tokens remain encrypted, expire, and repository selection rechecks installation access", async (t) => {
  let challenge = "";
  let allowRepository = true;
  let calls = 0;
  mockMethod(
    globalThis,
    "fetch",
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      calls++;
      assert.equal(init?.redirect, "error");
      assert.equal(init?.cache, "no-store");
      if (url === "https://github.com/login/oauth/access_token") {
        const body = new URLSearchParams(String(init?.body));
        assert.equal(
          createHash("sha256")
            .update(body.get("code_verifier")!)
            .digest("base64url"),
          challenge,
        );
        assert.equal(
          body.get("redirect_uri"),
          `${options.origin}/api/github/callback`,
        );
        return Response.json({
          access_token: "ghu_secret_user_token",
          token_type: "bearer",
          scope: "",
          expires_in: 28800,
          refresh_token: "ghr_never-store",
        });
      }
      assert.equal(
        new Headers(init?.headers).get("authorization"),
        "Bearer ghu_secret_user_token",
      );
      if (url === "https://api.github.com/user")
        return Response.json({ id: 42, login: "operator-github" });
      if (
        url === "https://api.github.com/user/installations?per_page=50&page=1"
      )
        return Response.json({
          total_count: 1,
          installations: [
            {
              id: 7,
              app_slug: options.appSlug,
              account: { login: "my-org" },
              suspended_at: null,
              permissions: { metadata: "read" },
            },
          ],
        });
      if (
        url ===
        "https://api.github.com/user/installations/7/repositories?per_page=50&page=1"
      )
        return Response.json({
          total_count: allowRepository ? 1 : 0,
          repositories: allowRepository
            ? [
                {
                  id: 11,
                  full_name: "my-org/site",
                  private: true,
                  html_url: "https://github.com/my-org/site",
                },
              ]
            : [],
        });
      throw Error(`Unexpected request: ${url}`);
    },
  );
  const github = configureGitHub(options);
  const start = await github.connect(
    request("/api/github/connect", "", "POST"),
    actor,
  );
  const url = new URL(start.headers.get("location")!);
  challenge = url.searchParams.get("code_challenge")!;
  const connected = await github.callback(
    request(
      `/api/github/callback?state=${url.searchParams.get("state")}&code=code`,
      cookieHeader(start),
    ),
    actor,
  );
  assert.match(connected.headers.get("location")!, /github=connected$/);
  assert.ok(!connected.headers.get("set-cookie")!.includes("ghu_"));
  assert.ok(!connected.headers.get("set-cookie")!.includes("ghr_"));
  assert.match(
    connected.headers
      .getSetCookie()
      .find((c) => c.startsWith("__Host-webdock-github="))!,
    /Max-Age=28800/,
  );
  const headers = new Headers({ cookie: cookieHeader(connected) });
  const session = await github.session(headers, actor);
  assert.equal(session?.login, "operator-github");
  assert.equal(await github.session(headers, "another-operator"), null);
  assert.equal(
    await github.session(
      new Headers({
        cookie: `${headers.get("cookie")}; ${headers.get("cookie")}`,
      }),
      actor,
    ),
    null,
  );
  const repository = await github.repository(session!, {
    installation: 7,
    installationPage: 1,
    page: 1,
    repository: 11,
  });
  assert.equal(repository.url, "https://github.com/my-org/site");
  const before = calls;
  await assert.rejects(() =>
    github.repository(session!, {
      installation: 999,
      installationPage: 1,
      page: 1,
      repository: 11,
    }),
  );
  assert.equal(calls, before + 1);
  allowRepository = false;
  await assert.rejects(() =>
    github.repository(session!, {
      installation: 7,
      installationPage: 1,
      page: 1,
      repository: 11,
    }),
  );
  const currentTime = Date.now();
  const clock = mockMethod(Date, "now", () => currentTime + 28801 * 1000);
  assert.equal(await github.session(headers, actor), null);
  clock.mock.restore();
  const disconnected = github.disconnect(
    request("/api/github/disconnect", "", "POST"),
  );
  assert.equal(disconnected.status, 303);
  assert.ok(
    disconnected.headers.getSetCookie().every((c) => c.includes("Max-Age=0")),
  );
});

test("GitHub configuration rejects unsafe origins and weak cookie secrets", () => {
  for (const change of [
    { origin: "https://evil.test/path" },
    { origin: "http://remote.test" },
    { cookieSecret: "short" },
    { appSlug: "../../evil" },
  ]) {
    assert.throws(() => configureGitHub({ ...options, ...change }));
  }
});

async function sourceFixture(t: TestContext, returnTo = "/integrations") {
  const commit = "a".repeat(40);
  const repository = {
    id: 11,
    full_name: "my-org/site",
    private: true,
    html_url: "https://github.com/my-org/site",
  };
  const state = {
    permissions: { metadata: "read", contents: "read" } as Record<
      string,
      string
    >,
    repositories: [repository],
    commit,
    location: `https://codeload.github.com/my-org/site/legacy.zip/${commit}?token=private-download-token`,
    download: () =>
      Promise.resolve(new Response(new Uint8Array([80, 75, 3, 4]))),
    requests: [] as { url: string; init?: RequestInit }[],
  };
  mockMethod(
    globalThis,
    "fetch",
    async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input);
      state.requests.push({ url, init });
      if (url === "https://github.com/login/oauth/access_token")
        return Response.json({
          access_token: "ghu_private_user_token",
          token_type: "bearer",
          scope: "",
          expires_in: 28800,
        });
      if (url === "https://api.github.com/user")
        return Response.json({ id: 42, login: "operator-github" });
      if (
        url === "https://api.github.com/user/installations?per_page=50&page=1"
      )
        return Response.json({
          total_count: 1,
          installations: [
            {
              id: 7,
              app_slug: options.appSlug,
              account: { login: "my-org" },
              permissions: state.permissions,
            },
          ],
        });
      if (
        url ===
        "https://api.github.com/user/installations/7/repositories?per_page=50&page=1"
      )
        return Response.json({
          total_count: state.repositories.length,
          repositories: state.repositories,
        });
      if (url.startsWith("https://api.github.com/repos/my-org/site/commits/"))
        return Response.json({ sha: state.commit });
      if (url === `https://api.github.com/repos/my-org/site/zipball/${commit}`)
        return new Response(null, {
          status: 302,
          headers: { location: state.location },
        });
      if (url.startsWith("https://codeload.github.com/"))
        return state.download();
      throw Error(`Unexpected request: ${url}`);
    },
  );
  const github = configureGitHub(options);
  const start = await github.connect(
    request("/api/github/connect", "", "POST"),
    actor,
    returnTo,
  );
  const nonce = new URL(start.headers.get("location")!).searchParams.get(
    "state",
  );
  const connected = await github.callback(
    request(
      `/api/github/callback?state=${nonce}&code=code`,
      cookieHeader(start),
    ),
    actor,
  );
  const headers = new Headers({ cookie: cookieHeader(connected) });
  state.requests.length = 0;
  return {
    github,
    state,
    connected,
    start,
    nonce,
    headers,
    selection: {
      installation: 7,
      installationPage: 1,
      page: 1,
      repository: 11,
    },
  };
}

test("GitHub source pins the selected private repository to a commit without forwarding credentials to codeload", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  const result = await github.source(headers, actor, selection);
  assert.deepEqual(result, {
    repository: {
      id: 11,
      name: "my-org/site",
      url: "https://github.com/my-org/site",
      private: true,
    },
    commit: state.commit,
    archive: new Uint8Array([80, 75, 3, 4]),
  });
  assert.ok(state.requests.some(({ url }) => url.endsWith("/commits/HEAD")));
  for (const { url, init } of state.requests) {
    const authorization = new Headers(init?.headers).get("authorization");
    assert.equal(
      authorization,
      url.startsWith("https://api.github.com/")
        ? "Bearer ghu_private_user_token"
        : null,
    );
    assert.equal(init?.cache, "no-store");
    assert.equal(
      init?.redirect,
      url.includes("/zipball/") ? "manual" : "error",
    );
    assert.ok(init?.signal);
  }
  await github.source(headers, actor, selection, "release/v1");
  assert.ok(
    state.requests.some(({ url }) => url.endsWith("/commits/release%2Fv1")),
  );
});

test("GitHub source rechecks owner, installation permissions and repository access", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  await assert.rejects(() =>
    github.source(headers, "another-operator", selection),
  );
  await assert.rejects(() => github.source(new Headers(), actor, selection));
  assert.equal(state.requests.length, 0);
  await assert.rejects(() =>
    github.source(headers, actor, { ...selection, installation: 99 }),
  );
  await assert.rejects(() =>
    github.source(headers, actor, { ...selection, repository: 99 }),
  );
  assert.ok(state.requests.every(({ url }) => !url.includes("/commits/")));
  state.permissions = { metadata: "read" };
  const session = (await github.session(headers, actor))!;
  assert.equal((await github.repositories(session, 7)).items.length, 1);
  await assert.rejects(
    () => github.source(headers, actor, selection),
    /Contents: read-only/,
  );
  state.permissions = { metadata: "read", contents: "write" };
  await assert.rejects(() => github.source(headers, actor, selection));
  state.permissions = { metadata: "read", contents: "read", issues: "read" };
  await assert.rejects(() => github.repositories(session, 7));
  state.permissions = { metadata: "read", contents: "read" };
  state.repositories = [];
  await assert.rejects(() => github.source(headers, actor, selection));
  assert.ok(state.requests.every(({ url }) => !url.includes("/commits/")));
});

test("GitHub source supports the existing Hosting App permissions without accepting unrelated writes", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  state.permissions = {
    metadata: "read",
    contents: "read",
    actions: "read",
    checks: "write",
  };
  const result = await github.source(headers, actor, selection);
  assert.equal(result.repository.private, true);
  assert.equal(result.commit, state.commit);
  state.permissions.actions = "write";
  assert.equal(
    (await github.source(headers, actor, selection)).commit,
    state.commit,
  );
  for (const extra of [
    { contents: "write" },
    { actions: "admin" },
    { issues: "write" },
    { deployments: "write" },
    { checks: "admin" },
  ]) {
    state.permissions = {
      metadata: "read",
      contents: "read",
      actions: "read",
      checks: "write",
      ...extra,
    };
    state.requests.length = 0;
    await assert.rejects(() => github.source(headers, actor, selection));
    assert.ok(
      state.requests.every(
        ({ url }) => !url.includes("/commits/") && !url.includes("/zipball/"),
      ),
    );
  }
});

test("GitHub source rejects malformed refs and commit hashes before downloading", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  for (const ref of [
    "",
    "x".repeat(256),
    ".",
    "..",
    "../main",
    "main?token=secret",
    "main\n",
  ]) {
    await assert.rejects(() => github.source(headers, actor, selection, ref));
  }
  assert.equal(state.requests.length, 0);
  for (const commit of [
    "main",
    "a".repeat(39),
    "g".repeat(40),
    "a".repeat(40) + "/evil",
    "a".repeat(40) + "\n",
  ]) {
    state.commit = commit;
    await assert.rejects(() => github.source(headers, actor, selection));
  }
  assert.ok(state.requests.every(({ url }) => !url.includes("/zipball/")));
});

test("GitHub source accepts only the exact HTTPS codeload origin and sanitizes provider failures", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  for (const location of [
    "https://evil.test/private-download-token",
    "http://codeload.github.com/repo",
    "https://codeload.github.com:444/repo",
    "https://user:pass@codeload.github.com/repo",
    "https://codeload.github.com.evil.test/repo",
    "/relative",
  ]) {
    state.location = location;
    await assert.rejects(
      () => github.source(headers, actor, selection),
      (error: Error) => {
        assert.doesNotMatch(
          error.message,
          /private-download-token|pass@|evil\.test/,
        );
        return true;
      },
    );
  }
  assert.ok(
    state.requests.every(({ url }) =>
      url.startsWith("https://api.github.com/"),
    ),
  );
  state.location =
    "https://codeload.github.com/repo?token=private-download-token";
  state.download = async () => {
    throw Error(`${state.location} ghu_private_user_token`);
  };
  await assert.rejects(
    () => github.source(headers, actor, selection),
    (error: Error) => {
      assert.doesNotMatch(
        error.message,
        /private-download-token|ghu_|codeload/,
      );
      return true;
    },
  );
});

test("GitHub source enforces the 32 MiB compressed limit both from headers and while streaming", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  let cancelled = 0;
  state.download = async () =>
    new Response(
      new ReadableStream({
        cancel() {
          cancelled++;
        },
      }),
      { headers: { "content-length": String(32 * 1024 * 1024 + 1) } },
    );
  await assert.rejects(() => github.source(headers, actor, selection));
  assert.equal(cancelled, 1);
  state.download = async () =>
    new Response(
      new ReadableStream({
        pull(controller) {
          controller.enqueue(new Uint8Array(16 * 1024 * 1024));
        },
        cancel() {
          cancelled++;
        },
      }),
    );
  await assert.rejects(() => github.source(headers, actor, selection));
  assert.equal(cancelled, 2);
});

test("GitHub OAuth seals plugin return paths and rejects untrusted redirect targets", async (t) => {
  const { github, connected, start, nonce, state } = await sourceFixture(
    t,
    "/plugins/123",
  );
  assert.equal(
    connected.headers.get("location"),
    `${options.origin}/plugins/123?github=connected`,
  );
  assert.equal(
    github
      .disconnect(request("/api/github/disconnect", "", "POST"), "/plugins")
      .headers.get("location"),
    `${options.origin}/plugins?github=disconnected`,
  );
  for (const returnTo of [
    "https://evil.test",
    "//evil.test",
    "/plugins/../evil",
    "/plugins?returnTo=//evil.test",
    "/plugins/%31",
    "/plugins/0",
    "/plugins/123/other",
    "/plugins\n",
  ]) {
    assert.equal(
      (
        await github.connect(
          request("/api/github/connect", "", "POST"),
          actor,
          returnTo,
        )
      ).status,
      400,
    );
    assert.equal(
      github.disconnect(request("/api/github/disconnect", "", "POST"), returnTo)
        .status,
      400,
    );
  }
  const declined = await github.callback(
    request(
      `/api/github/callback?state=${nonce}&error=access_denied`,
      cookieHeader(start),
    ),
    actor,
  );
  assert.equal(
    declined.headers.get("location"),
    `${options.origin}/plugins/123?github=failed`,
  );
  assert.equal(state.requests.length, 0);
});

test("GitHub callback revalidates sealed return paths and ignores callback redirect overrides", async (t) => {
  const { github, start, nonce, state } = await sourceFixture(t, "/plugins");
  const password = createHash("sha256")
    .update("webdock-github-cookie\0")
    .update(options.cookieSecret)
    .digest("hex");
  const flowCookie = cookieHeader(start);
  const token = flowCookie.slice(flowCookie.indexOf("=") + 1);
  const flow = await unsealData<Record<string, unknown>>(token, {
    password,
    ttl: 600,
  });
  const invalid = await sealData(
    { ...flow, returnTo: "//evil.test" },
    { password, ttl: 600 },
  );
  const denied = await github.callback(
    request(
      `/api/github/callback?state=${nonce}&code=code`,
      `__Host-webdock-github-flow=${invalid}`,
    ),
    actor,
  );
  assert.equal(
    denied.headers.get("location"),
    `${options.origin}/integrations?github=failed`,
  );
  assert.equal(state.requests.length, 0);
  const connected = await github.callback(
    request(
      `/api/github/callback?state=${nonce}&code=code&returnTo=//evil.test`,
      flowCookie,
    ),
    actor,
  );
  assert.equal(
    connected.headers.get("location"),
    `${options.origin}/plugins?github=connected`,
  );
});

test("GitHub OAuth returns to the exact project installer path", async (t) => {
  const { github, connected, start, nonce } = await sourceFixture(
    t,
    "/plugins/123/install",
  );
  assert.equal(
    connected.headers.get("location"),
    `${options.origin}/plugins/123/install?github=connected`,
  );
  assert.equal(
    github
      .disconnect(
        request("/api/github/disconnect", "", "POST"),
        "/plugins/123/install",
      )
      .headers.get("location"),
    `${options.origin}/plugins/123/install?github=disconnected`,
  );
  assert.equal(
    (
      await github.connect(
        request("/api/github/connect", "", "POST"),
        actor,
        "/plugins/install",
      )
    ).status,
    400,
  );
  const declined = await github.callback(
    request(
      `/api/github/callback?state=${nonce}&error=access_denied`,
      cookieHeader(start),
    ),
    actor,
  );
  assert.equal(
    declined.headers.get("location"),
    `${options.origin}/plugins/123/install?github=failed`,
  );
});

test("GitHub source rejects redirecting, failed or empty archive responses", async (t) => {
  const { github, state, headers, selection } = await sourceFixture(t);
  for (const response of [
    new Response(null, {
      status: 302,
      headers: { location: "https://evil.test" },
    }),
    new Response("Not found", { status: 404 }),
    new Response(new Uint8Array()),
  ]) {
    state.download = async () => response;
    await assert.rejects(() => github.source(headers, actor, selection));
  }
  assert.ok(state.requests.every(({ url }) => !url.includes("evil.test")));
});

test("GitHub rejects PATs, non-expiring tokens and over-permissioned installations", async (t) => {
  let responseToken: Record<string, unknown> = {};
  let installationPermissions: Record<string, string> = {
    metadata: "read",
    contents: "write",
  };
  mockMethod(globalThis, "fetch", async (input: Request | URL | string) => {
    if (String(input) === "https://github.com/login/oauth/access_token")
      return Response.json(responseToken);
    if (String(input).startsWith("https://api.github.com/user/installations?"))
      return Response.json({
        total_count: 1,
        installations: [
          {
            id: 7,
            app_slug: options.appSlug,
            account: { login: "my-org" },
            permissions: installationPermissions,
          },
        ],
      });
    throw Error("Invalid token must not reach the user API");
  });
  const github = configureGitHub(options);
  for (const token of [
    {
      access_token: "ghp_pat",
      token_type: "bearer",
      scope: "",
      expires_in: 28800,
    },
    { access_token: "ghu_no_expiry", token_type: "bearer", scope: "" },
    {
      access_token: "ghu_too_long",
      token_type: "bearer",
      scope: "",
      expires_in: 28801,
    },
    {
      access_token: "ghu_has_scope",
      token_type: "bearer",
      scope: "repo",
      expires_in: 28800,
    },
  ]) {
    responseToken = token;
    const start = await github.connect(
      request("/api/github/connect", "", "POST"),
      actor,
    );
    const state = new URL(start.headers.get("location")!).searchParams.get(
      "state",
    );
    const failed = await github.callback(
      request(
        `/api/github/callback?code=code&state=${state}`,
        cookieHeader(start),
      ),
      actor,
    );
    assert.match(failed.headers.get("location")!, /github=failed$/);
    assert.ok(
      failed.headers
        .getSetCookie()
        .every((cookie) => cookie.startsWith("__Host-webdock-github-flow=")),
    );
  }
  const session = {
    accessToken: "ghu_fake",
    exp: Math.floor(Date.now() / 1000) + 300,
  } as never;
  await assert.rejects(
    () => github.installations(session),
    /Metadata: read-only/,
  );
  installationPermissions = { metadata: "read" };
  assert.equal((await github.installations(session)).items[0]?.id, 7);
});

test("GitHub callback rejects duplicated parameters, wrong hosts and expired flows before token exchange", async (t) => {
  let exchanges = 0;
  mockMethod(globalThis, "fetch", async () => {
    exchanges++;
    throw Error("Invalid flow must never exchange a token");
  });
  const github = configureGitHub(options);
  const start = await github.connect(
    request("/api/github/connect", "", "POST"),
    actor,
  );
  const state = new URL(start.headers.get("location")!).searchParams.get(
    "state",
  )!;
  const flowCookie = cookieHeader(start);
  const path = `/api/github/callback?code=code&state=${state}`;
  for (const incoming of [
    request(`${path}&state=${state}`, flowCookie),
    request(`${path}&code=second`, flowCookie),
    request(`${path}&error=access_denied`, flowCookie),
    request(path, `${flowCookie}; ${flowCookie}`),
    new Request(`https://attacker.invalid${path}`, {
      headers: { cookie: flowCookie },
    }),
    request(path, flowCookie, "POST"),
  ]) {
    const response = await github.callback(incoming, actor);
    assert.equal(
      response.headers.get("location"),
      `${options.origin}/integrations?github=failed`,
    );
    assert.ok(
      response.headers
        .getSetCookie()
        .every((cookie) => cookie.includes("Max-Age=0")),
    );
  }
  assert.match(
    (await github.callback(request(path, flowCookie), "")).headers.get(
      "location",
    )!,
    /github=failed$/,
  );
  const currentTime = Date.now();
  const clock = mockMethod(Date, "now", () => currentTime + 601_000);
  assert.match(
    (await github.callback(request(path, flowCookie), actor)).headers.get(
      "location",
    )!,
    /github=failed$/,
  );
  clock.mock.restore();
  assert.equal(exchanges, 0);
});
