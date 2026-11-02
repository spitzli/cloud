import test from "node:test";
import assert from "node:assert/strict";
import {
  gitConnectionRoute,
  readGitFlowCookie,
} from "../../src/modules/integrations/github/byok-routes";
const origin = "https://studio.example",
  state = "a".repeat(43);
test("Git callback rejects forged, duplicate and missing state without exchanging code", async () => {
  let calls = 0;
  for (const suffix of [
    "state=wrong&code=x",
    `state=${state}&state=${state}&code=x`,
    "code=x",
  ]) {
    const r = await gitConnectionRoute(
      new Request(`${origin}/callback?${suffix}`, {
        headers: { cookie: `__Host-webdock-git-flow=${state}` },
      }),
      "callback",
      {
        origin,
        call: async () => {
          calls++;
          return {};
        },
      },
    );
    assert.equal(
      r.headers.get("location"),
      `${origin}/hosting/git/connection-error`,
    );
  }
  assert.equal(calls, 0);
  assert.throws(() =>
    readGitFlowCookie(
      new Headers({
        cookie: `__Host-webdock-git-flow=${state}; __Host-webdock-git-flow=${state}`,
      }),
      origin,
    ),
  );
});
test("Git connect is same-origin POST and leaves authorization to hosting", async () => {
  const calls: unknown[] = [];
  const deps = {
    origin,
    call: async (cmd: unknown) => {
      calls.push(cmd);
      return {
        state,
        url: `https://github.com/login/oauth/authorize?state=${state}`,
      };
    },
  };
  const request = (requestOrigin: string) =>
    new Request(`${origin}/connect`, {
      method: "POST",
      headers: {
        origin: requestOrigin,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: "customerID=123",
    });
  assert.equal(
    (await gitConnectionRoute(request("https://evil.example"), "connect", deps))
      .status,
    403,
  );
  assert.equal(calls.length, 0);
  const result = await gitConnectionRoute(request(origin), "connect", deps);
  assert.equal(result.status, 303);
  assert.match(
    result.headers.get("set-cookie")!,
    /HttpOnly; SameSite=Lax; Max-Age=600; Secure/,
  );
  assert.deepEqual(calls, [{ action: "git.oauth.begin", customerID: "123" }]);
});

test("Git setup return is restricted to a verified project draft path", async () => {
  const request = () =>
    new Request(origin + "/select", {
      method: "POST",
      headers: {
        origin,
        "Content-Type": "application/x-www-form-urlencoded",
        cookie: `__Host-webdock-git-flow=${state}`,
      },
      body: "installationID=9&repositoryID=42",
    });
  const returned = await gitConnectionRoute(request(), "select", {
    origin,
    call: async () => ({
      customerID: "123",
      returnPath: "/hosting/projects/456/setup/789",
    }),
  });
  assert.equal(
    returned.headers.get("location"),
    origin + "/hosting/projects/456/setup/789?git=connected",
  );
  const bad = await gitConnectionRoute(request(), "select", {
    origin,
    call: async () => ({
      customerID: "123",
      returnPath: "https://evil.example/",
    }),
  });
  assert.equal(
    bad.headers.get("location"),
    origin + "/hosting/git/connection-error",
  );
});
