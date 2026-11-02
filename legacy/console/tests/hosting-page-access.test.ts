import test from "node:test";
import assert from "node:assert/strict";
import { HostingError } from "@webdock/hosting-contracts";
import { withHostingPageAccess, HostingSessionRequiredError } from "../src/lib/hosting-page-access";
const digest = (error: unknown) =>
  error instanceof Error && "digest" in error ? String(error.digest) : "";
test("missing and expired hosting sessions redirect to sign-in with the requested page", async () => {
  const path = "/hosting/projects/123?sort=-name";
  await assert.rejects(
    withHostingPageAccess(path, async () => {
      throw new HostingSessionRequiredError();
    }),
    (error) =>
      digest(error).startsWith("NEXT_REDIRECT;") &&
      digest(error).includes(
        "/api/sso/login?returnTo=" + encodeURIComponent(path),
      ),
  );
});
test("foreign and absent hosting targets become not-found, not another login", async () => {
  for (const status of [403, 404])
    await assert.rejects(
      withHostingPageAccess("/hosting/apps/123", async () => {
        throw new HostingError(status, "Unavailable.");
      }),
      (error) => digest(error) === "NEXT_HTTP_ERROR_FALLBACK;404",
    );
});
test("genuine outages stay errors and successful page reads remain unchanged", async () => {
  const failure = new HostingError(503, "Unavailable");
  await assert.rejects(
    withHostingPageAccess("/tenants/123/hosting", async () => {
      throw failure;
    }),
    (error) => error === failure,
  );
  assert.deepEqual(
    await withHostingPageAccess("/hosting/apps/123", async () => ({
      id: "123",
    })),
    { id: "123" },
  );
});
test("page recovery cannot redirect sign-in to an external or unsupported destination", async () => {
  for (const path of [
    "https://evil.invalid",
    "//evil.invalid",
    "/\\evil.invalid",
    "/api/hosting/apps",
  ])
    await assert.rejects(
      withHostingPageAccess(path, async () => {
        throw new HostingSessionRequiredError();
      }),
      (error) => digest(error).includes("/api/sso/login?returnTo=%2Ftenants"),
    );
});


test("an already delegated session rejected by the backend requires manual recovery", async () => {
  for (const status of [401, 429, 503]) {
    const failure = new HostingError(status, "Unavailable");
    await assert.rejects(withHostingPageAccess("/hosting/apps/123", async () => { throw failure; }), error => error === failure);
  }
});
