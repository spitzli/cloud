import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { sealData, unsealData } from "iron-session";
import {
  configureVercelOAuth,
  vercelCompletionURL,
} from "../../src/modules/integrations/vercel/vercel-oauth";

const options = {
  clientID: "oac_fixture",
  clientSecret: "fixture-secret",
  slug: "webdock-studio",
  origin: "https://admin.example.test",
  teamID: "team_fixture",
  teamSlug: "fixture-team",
  cookieSecret: "fixture-cookie-secret-at-least-thirty-two-characters",
};
const actor = "1001";
const completionURL =
  "https://vercel.com/integrations/webdock-studio/complete?configurationId=icfg_fixture";
const token = {
  token_type: "Bearer",
  access_token: "vcp_fixture_token",
  installation_id: "icfg_fixture",
  team_id: options.teamID,
};
const scopes = [
  "read:integration-configuration",
  "read:project",
  "read:deployment",
  "read:domain",
];
const installation = {
  id: token.installation_id,
  teamId: options.teamID,
  integrationId: options.clientID,
  slug: options.slug,
  scopes,
};
const failed = {
  message: "Vercel connection could not be verified. Start again.",
};
const password = createHash("sha256")
  .update("webdock-vercel-integration-cookie\0")
  .update(options.cookieSecret)
  .digest("hex");
function rig(
  tokenResult: unknown = token,
  configResult: unknown = installation,
) {
  const calls: { url: string; init?: RequestInit }[] = [];
  const fetcher: typeof fetch = async (input, init) => {
    const url = String(input);
    calls.push({ url, init });
    assert.equal(init?.redirect, "error");
    assert.equal(init?.cache, "no-store");
    assert.ok(init?.signal instanceof AbortSignal);
    if (url === "https://api.vercel.com/v2/oauth/access_token")
      return Response.json(tokenResult);
    assert.equal(
      url,
      `https://api.vercel.com/v1/integrations/configuration/icfg_fixture?teamId=${options.teamID}`,
    );
    assert.equal(
      new Headers(init?.headers).get("authorization"),
      `Bearer ${token.access_token}`,
    );
    return Response.json(configResult);
  };
  return { oauth: configureVercelOAuth({ ...options, fetcher }), calls };
}
async function callback(
  oauth: ReturnType<typeof configureVercelOAuth>,
  edit?: (url: URL) => void,
) {
  const start = await oauth.start(actor),
    url = new URL(`${options.origin}/api/vercel/callback`);
  url.search = new URLSearchParams({
    code: "fixture-code",
    state: new URL(start.url).searchParams.get("state")!,
    teamId: options.teamID,
    configurationId: token.installation_id,
    next: completionURL,
  }).toString();
  edit?.(url);
  const cookie = start.cookie.split(";")[0]!;
  return {
    start,
    url,
    cookie,
    request: new Request(url, { headers: { cookie } }),
  };
}

test("private dashboard installation URL and host-only sealed flow bind operator/client/team for 600 seconds", async () => {
  const { oauth } = rig();
  const first = await oauth.start(actor),
    second = await oauth.start(actor),
    url = new URL(first.url);
  assert.equal(url.origin, "https://vercel.com");
  assert.equal(url.pathname, "/fixture-team/~/integrations/webdock-studio");
  assert.deepEqual([...url.searchParams.keys()], ["state"]);
  assert.notEqual(first.url, second.url);
  assert.match(first.cookie, /^__Host-webdock-vercel-flow=/);
  assert.match(
    first.cookie,
    /Path=\/; HttpOnly; Secure; SameSite=Lax; Max-Age=600$/,
  );
  assert.ok(!first.cookie.includes("Domain="));
  const flow = await unsealData<Record<string, unknown>>(
    first.cookie.split(";")[0]!.split("=")[1]!,
    { password, ttl: 600 },
  );
  assert.equal(flow.operatorID, actor);
  assert.equal(flow.clientID, options.clientID);
  assert.equal(flow.teamID, options.teamID);
  assert.equal(flow.origin, options.origin);
  assert.equal(flow.state, url.searchParams.get("state"));
  assert.equal(Number(flow.exp) - Number(flow.iat), 600);
  assert.match(
    oauth.clearCookie(),
    /^__Host-webdock-vercel-flow=; Path=\/; HttpOnly; Secure; SameSite=Lax; Max-Age=0$/,
  );
  await assert.rejects(oauth.start(""), failed);
});

test("exchanges the code with exact redirect URI and verifies read-only installation", async () => {
  const { oauth, calls } = rig();
  const { request } = await callback(oauth);
  assert.deepEqual(await oauth.finish(request, actor), {
    accessToken: token.access_token,
    teamID: options.teamID,
    configurationID: token.installation_id,
    completionURL,
  });
  assert.equal(calls.length, 2);
  assert.equal(calls[0]!.init?.method, "POST");
  assert.equal(
    new Headers(calls[0]!.init?.headers).get("content-type"),
    "application/x-www-form-urlencoded",
  );
  assert.deepEqual(
    Object.fromEntries(calls[0]!.init?.body as URLSearchParams),
    {
      client_id: options.clientID,
      client_secret: options.clientSecret,
      code: "fixture-code",
      redirect_uri: `${options.origin}/api/vercel/callback`,
    },
  );
});

test("wrong actor, cookie, state, duplicate parameters and callback team fail before provider calls", async () => {
  const { oauth, calls } = rig(),
    flow = await callback(oauth);
  await assert.rejects(oauth.finish(flow.request, "another-operator"), failed);
  for (const cookie of [
    "",
    "__Host-webdock-vercel-flow=garbage",
    `${flow.cookie}; ${flow.cookie}`,
  ])
    await assert.rejects(
      oauth.finish(new Request(flow.url, { headers: { cookie } }), actor),
      failed,
    );
  const changes: ((u: URL) => void)[] = [
    (u) => u.searchParams.set("state", "x".repeat(43)),
    (u) => u.searchParams.set("state", "é".repeat(43)),
    (u) => u.searchParams.delete("state"),
    (u) => u.searchParams.delete("code"),
    (u) => u.searchParams.set("code", "code\nsecret"),
    (u) => u.searchParams.set("teamId", "team_other"),
    (u) => u.searchParams.delete("teamId"),
    (u) => u.searchParams.set("configurationId", "../../other"),
    (u) => u.searchParams.set("configurationId", "icfg_"),
    (u) => u.searchParams.set("error", "provider-secret"),
    ...["state", "code", "teamId", "configurationId", "next"].map(
      (key) => (u: URL) => {
        u.searchParams.append(key, "duplicate");
        u.searchParams.append(key, "duplicate");
      },
    ),
  ];
  for (const edit of changes)
    await assert.rejects(
      oauth.finish((await callback(oauth, edit)).request, actor),
      failed,
    );
  assert.equal(calls.length, 0);
});

test("expired, future, excessive-age and cross-client/team/origin flow seals fail", async () => {
  const { oauth, calls } = rig(),
    { url, cookie } = await callback(oauth);
  const flow = await unsealData<Record<string, unknown>>(
    cookie.split("=")[1]!,
    { password, ttl: 600 },
  );
  const time = Math.floor(Date.now() / 1000);
  for (const changed of [
    { iat: time - 601, exp: time - 1 },
    { iat: time + 60, exp: time + 660 },
    { exp: time + 3600 },
    { clientID: "another" },
    { teamID: "team_other" },
    { origin: "https://other.test" },
    { slug: "other" },
    { kind: "github-flow" },
  ]) {
    const sealed = `__Host-webdock-vercel-flow=${await sealData({ ...flow, ...changed }, { password, ttl: 600 })}`;
    await assert.rejects(
      oauth.finish(new Request(url, { headers: { cookie: sealed } }), actor),
      failed,
    );
  }
  assert.equal(calls.length, 0);
});

test("exact method/path/origin and raw Host required; proxy loopback accepted but forwarded headers ignored", async () => {
  const { oauth, calls } = rig(),
    { url, cookie } = await callback(oauth);
  const cases: [string, Record<string, string>, string][] = [
    [`${options.origin}/api/vercel/callback/${url.search}`, { cookie }, "GET"],
    [`https://evil.test/api/vercel/callback${url.search}`, { cookie }, "GET"],
    [
      `https://evil.test/api/vercel/callback${url.search}`,
      { cookie, host: "admin.example.test" },
      "GET",
    ],
    [url.href, { cookie, host: "evil.test" }, "GET"],
    [
      `http://localhost:3120/api/vercel/callback${url.search}`,
      { cookie, "x-forwarded-host": "admin.example.test" },
      "GET",
    ],
    [url.href, { cookie }, "POST"],
    [`${url.href}#fragment`, { cookie }, "GET"],
  ];
  for (const [target, headers, method] of cases)
    await assert.rejects(
      oauth.finish(new Request(target, { headers, method }), actor),
      failed,
    );
  assert.equal(calls.length, 0);
  assert.deepEqual(
    await oauth.finish(
      new Request(`http://localhost:3120/api/vercel/callback${url.search}`, {
        headers: { cookie, host: "admin.example.test" },
      }),
      actor,
    ),
    {
      accessToken: token.access_token,
      teamID: options.teamID,
      configurationID: token.installation_id,
      completionURL,
    },
  );
});

test("malformed token type, installation, team or credential never reaches configuration endpoint", async () => {
  for (const result of [
    null,
    [],
    {},
    { ...token, token_type: "Basic" },
    { ...token, access_token: "" },
    { ...token, access_token: "secret\r\nheader" },
    { ...token, installation_id: "icfg_other" },
    { ...token, team_id: null },
    { ...token, team_id: "team_other" },
    { ...token, error: "secret-error" },
  ]) {
    const { oauth, calls } = rig(result);
    await assert.rejects(
      oauth.finish((await callback(oauth)).request, actor),
      failed,
    );
    assert.equal(calls.length, 1);
  }
});

test("installation identity and exact common scopes plus one project scope required", async () => {
  for (const result of [
    null,
    [],
    {},
    { ...installation, id: "icfg_other" },
    { ...installation, teamId: "team_other" },
    { ...installation, integrationId: "oac_other" },
    { ...installation, slug: "other" },
    { ...installation, disabledAt: 1 },
    { ...installation, deletedAt: 1 },
    { ...installation, deleteRequestedAt: 1 },
    { ...installation, status: "suspended" },
    { ...installation, status: "unknown" },
    { ...installation, status: null },
    { ...installation, installationType: "marketplace" },
    ...[
      undefined,
      [],
      scopes.slice(1),
      [...scopes, "read:team"],
      [...scopes, "read-write:project"],
      [...scopes, "write:deployment"],
      [...scopes, "write:project"],
      scopes.map((scope) =>
        scope === "read:project" ? "write:project" : scope,
      ),
      scopes.map((scope) =>
        scope === "read:domain" ? "read-write:project" : scope,
      ),
      [...scopes, "write:project", "write:project"],
      [scopes[0], scopes[0], scopes[2], scopes[3]],
      scopes.join(" "),
      [...scopes.slice(0, 3), 42],
    ].map((value) => ({ ...installation, scopes: value })),
  ]) {
    const { oauth } = rig(token, result);
    await assert.rejects(
      oauth.finish((await callback(oauth)).request, actor),
      failed,
    );
  }
});

test("optional project write permission preserves existing read-only installation support", async () => {
  for (const granted of [
    scopes,
    [
      "read:integration-configuration",
      "read:deployment",
      "read:domain",
      "read-write:project",
    ],
  ]) {
    const { oauth } = rig(token, { ...installation, scopes: granted });
    const result = await oauth.finish((await callback(oauth)).request, actor);
    assert.equal(result.configurationID, installation.id);
  }
});

test("network/provider/JSON errors become one nonsecret message", async () => {
  for (const fetcher of [
    async () => {
      throw Error("provider leaked token-and-client-secret");
    },
    async () => new Response("provider leaked secret", { status: 500 }),
    async () => new Response("private malformed JSON", { status: 200 }),
  ]) {
    const oauth = configureVercelOAuth({ ...options, fetcher });
    await assert.rejects(
      oauth.finish((await callback(oauth)).request, actor),
      failed,
    );
  }
});

test("invalid/insecure configuration rejected; HTTP only on nonproduction loopback", async (t) => {
  for (const changed of [
    { origin: "not-a-url" },
    { origin: "http://remote.test" },
    { origin: "https://admin.test/subpath" },
    { origin: "https://user:pass@admin.test" },
    { origin: "https://admin.test?redirect=evil" },
    { cookieSecret: "short" },
    { slug: "../evil" },
    { clientID: "" },
    { clientSecret: "" },
    { teamID: "personal" },
  ])
    assert.throws(() => configureVercelOAuth({ ...options, ...changed }), {
      message: "Invalid Vercel integration configuration.",
    });
  const previous = process.env.NODE_ENV;
  t.after(() => {
    if (previous === undefined) Reflect.deleteProperty(process.env, "NODE_ENV");
    else Object.assign(process.env, { NODE_ENV: previous });
  });
  Object.assign(process.env, { NODE_ENV: "development" });
  const local = configureVercelOAuth({
    ...options,
    origin: "http://127.0.0.1:3120",
  });
  assert.match((await local.start(actor)).cookie, /^webdock-vercel-flow=/);
  assert.ok(!(await local.start(actor)).cookie.includes("Secure"));
  assert.match(local.clearCookie(), /Max-Age=0$/);
  Object.assign(process.env, { NODE_ENV: "production" });
  assert.throws(() =>
    configureVercelOAuth({ ...options, origin: "http://127.0.0.1:3120" }),
  );
});

test("callback diagnostics expose only a fixed failure stage, never credentials", async () => {
  const { oauth } = rig();
  const missing = await callback(oauth, (url) =>
    url.searchParams.delete("state"),
  );
  await assert.rejects(oauth.finish(missing.request, actor), {
    name: "VercelOAuthError",
    message: failed.message,
    stage: "callback-parameters",
  });
  const unsafe = rig({
    token_type: "Bearer",
    access_token: "private-secret-not-for-logs",
  });
  const request = await callback(unsafe.oauth);
  await assert.rejects(unsafe.oauth.finish(request.request, actor), (error) => {
    assert.equal((error as { stage: string }).stage, "token-response");
    assert.equal(JSON.stringify(error).includes("private-secret"), false);
    assert.equal((error as Error).message, failed.message);
    return true;
  });
});

test("Vercel installation completion is required and restricted to the exact HTTPS provider origin", async () => {
  assert.equal(vercelCompletionURL(completionURL), completionURL);
  for (const next of [
    null,
    "",
    "https://evil.example/done",
    "https://vercel.com.evil.example/done",
    "https://user:secret@vercel.com/done",
    "http://vercel.com/done",
    "//vercel.com/done",
    "javascript:alert(1)",
    "https://vercel.com:444/done",
    "https://vercel.com/done#secret",
  ]) {
    const { oauth, calls } = rig();
    const value = await callback(oauth, (url) =>
      next === null
        ? url.searchParams.delete("next")
        : url.searchParams.set("next", next),
    );
    await assert.rejects(oauth.finish(value.request, actor), {
      name: "VercelOAuthError",
      stage: "completion-url",
    });
    assert.equal(
      calls.length,
      0,
      "Invalid completion must fail before consuming the OAuth code",
    );
  }
});
