import { test } from "node:test";
import assert from "node:assert/strict";
import { requestAPI } from "../../../packages/api-client/src/index";
test("Clerk credentials only go to the separate Console boundary", async () => {
  const input = {
    apiOrigin: "https://api.example.test",
    uiOrigin: "https://console.example.test",
    cookie: "webdock-sso=old",
    path: "/v1/console",
    body: { operation: "session", args: [] },
    consoleSession: { token: "signed-token", serviceKey: "s".repeat(32) },
  };
  let sent: Headers | undefined;
  await requestAPI(input, async (_url, init) => {
    sent = new Headers(init?.headers);
    return Response.json({ data: {} });
  });
  assert.equal(sent?.get("authorization"), "Bearer signed-token");
  assert.equal(sent?.get("x-webdock-service-key"), "s".repeat(32));
  assert.equal(sent?.get("cookie"), "");
  await assert.rejects(
    requestAPI({ ...input, path: "/v1/studio" }, async () => {
      throw Error("must not send");
    }),
    /Console/,
  );
});
