import { expect, test } from "bun:test";
import { createIntegrationCallbackHandoff } from "../src/protocols/integration-callback-handoff";
import {
  configureProductIntegrationCallbacks,
  createIntegrationResumeHandler,
} from "../src/protocols/integration-resume";
const origin = "https://studio.example.test";
function fixture(path = "/api/github/callback") {
  configureProductIntegrationCallbacks(origin, {});
  const handoff = createIntegrationCallbackHandoff({
    origin,
    consoleOrigin: origin,
    secret: "test-handoff-secret-with-32-characters",
  });
  const begun = handoff.begin(
    new Request(origin + path + "?code=secret&state=state"),
  )!;
  const cookie =
    begun.headers.get("set-cookie")!.split(";")[0] +
    "; webdock-github-flow=sealed-flow";
  let calls = 0;
  const handler = createIntegrationResumeHandler(
    origin,
    handoff,
    async (request, options) => {
      calls++;
      expect(options.refreshed).toBe(true);
      expect(request.url).toBe(
        origin + "/api/github/callback?code=secret&state=state",
      );
      expect(request.headers.get("cookie")).toContain(
        "webdock-github-flow=sealed-flow",
      );
      expect(request.headers.get("authorization")).toBe("Bearer verified");
      return new Response(null, {
        status: 303,
        headers: { location: origin + "/plugins/123/install?github=connected" },
      });
    },
  );
  return {
    handler,
    calls: () => calls,
    request: (headers: Record<string, string> = {}) =>
      new Request("https://api.example.test/v1/integrations/resume", {
        method: "POST",
        headers: {
          origin,
          authorization: "Bearer verified",
          cookie,
          ...headers,
        },
      }),
  };
}
test("Core resumes only its GitHub callback with refreshed verified proof and clears the handoff", async () => {
  const f = fixture();
  const response = await f.handler(f.request());
  expect(response.status).toBe(303);
  expect(response.headers.get("location")).toBe(
    origin + "/plugins/123/install?github=connected",
  );
  expect(response.headers.get("set-cookie")).toContain("Max-Age=0");
  expect(response.headers.get("cache-control")).toBe("no-store");
  expect(f.calls()).toBe(1);
  const invalidHeaders: Record<string, string>[] = [
    { origin: "https://evil.test" },
    { authorization: "Basic legacy" },
    { cookie: "invalid" },
  ];
  for (const headers of invalidHeaders)
    expect((await f.handler(f.request(headers))).status).toBeGreaterThanOrEqual(
      400,
    );
  expect(f.calls()).toBe(1);
});
test("Hosting callbacks are unavailable in Core and retain their own admission when registered", async () => {
  const f = fixture("/api/vercel/callback");
  expect((await f.handler(f.request())).status).toBe(400);
  let productCalls = 0;
  configureProductIntegrationCallbacks(origin, {
    "/api/vercel/callback": async (_request, options) => {
      productCalls++;
      expect(options.refreshed).toBe(true);
      return new Response(null, { status: 503 });
    },
  });
  expect((await f.handler(f.request())).status).toBe(503);
  expect(productCalls).toBe(1);
  expect(f.calls()).toBe(0);
  configureProductIntegrationCallbacks(origin, {});
});
