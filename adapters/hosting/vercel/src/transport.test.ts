import { test } from "node:test";
import assert from "node:assert/strict";
import { createVercelJSONTransport } from "./transport";
import { HostingProviderError } from "@spitzli/hosting-provider-contracts";
test("Vercel transport pins origin/team and never forwards redirects", async () => {
  let calls = 0;
  const transport = createVercelJSONTransport(async (input, init) => {
    calls++;
    const url = new URL(String(input));
    assert.equal(url.origin, "https://api.vercel.com");
    assert.equal(url.searchParams.get("teamId"), "team_one");
    assert.equal(
      new Headers(init?.headers).get("authorization"),
      "Bearer test-token",
    );
    assert.equal(init?.redirect, "error");
    assert.equal(init?.cache, "no-store");
    return Response.json({ id: "prj_one" });
  });
  assert.deepEqual(
    await transport("/v9/projects/prj_one", "test-token", "team_one"),
    { id: "prj_one" },
  );
  await assert.rejects(
    transport("https://other.example/token", "test-token", "team_one"),
    (error: unknown) =>
      error instanceof HostingProviderError && error.status === 400,
  );
  assert.equal(calls, 1);
});
test("provider response and network failures retain redacted status/error contract", async () => {
  const secret = "upstream-secret";
  for (const status of [403, 401, 500]) {
    const transport = createVercelJSONTransport(
      async () => new Response(secret, { status }),
    );
    await assert.rejects(
      transport("/v9/projects"),
      (error: unknown) =>
        error instanceof HostingProviderError &&
        error.status === (status === 403 ? 403 : 502) &&
        !error.message.includes(secret),
    );
  }
  for (const fetcher of [
    async () => {
      throw Error(secret);
    },
    async () => new Response(secret),
    async () => new Response("x".repeat(2_000_001)),
  ]) {
    await assert.rejects(
      createVercelJSONTransport(fetcher)("/v9/projects"),
      (error: unknown) =>
        error instanceof HostingProviderError &&
        error.status === 502 &&
        !error.message.includes(secret),
    );
  }
});
