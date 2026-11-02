import { test, expect } from "bun:test";
const enabled = process.env.API_TEST_DATABASE_URL;
(enabled ? test : test.skip)(
  "Auth UI bridge authenticates native cookies and ignores caller-supplied identities",
  async () => {
    const { withMailFixture } =
      await import("./ported-auth/helpers/mail-product");
    const { handleNativeStudioRequest } =
      await import("../src/modules/portal/studio-api");
    const origin = process.env.BETTER_AUTH_URL!;
    await withMailFixture(async (f) => {
      const req = (headers: Headers, body: unknown) =>
        new Request(origin + "/v1/auth/studio", {
          method: "POST",
          headers: new Headers([
            ...headers,
            ["content-type", "application/json"],
          ]),
          body: JSON.stringify(body),
        });
      const result = await handleNativeStudioRequest(
        req(f.admin.headers, { operation: "session", args: [] }),
      );
      expect(result.status).toBe(200);
      expect((await result.json()).data.user.id).toBe(f.admin.id);
      expect(
        (
          await handleNativeStudioRequest(
            req(new Headers({ origin }), { operation: "session", args: [] }),
          )
        ).status,
      ).toBe(401);
      expect(
        (
          await handleNativeStudioRequest(
            req(f.admin.headers, {
              operation: "session",
              args: [],
              accessToken: "forged",
            }),
          )
        ).status,
      ).toBe(400);
      const foreign = new Headers(f.admin.headers);
      foreign.set("origin", "https://evil.invalid");
      expect(
        (
          await handleNativeStudioRequest(
            req(foreign, { operation: "session", args: [] }),
          )
        ).status,
      ).toBe(403);
    });
  },
);
