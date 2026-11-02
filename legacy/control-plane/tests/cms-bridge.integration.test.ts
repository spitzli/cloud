import { test, expect, spyOn } from "bun:test";
const enabled = process.env.API_TEST_DATABASE_URL;
(enabled ? test : test.skip)(
  "CMS metadata excludes bridge secrets and preview writes never reach the CMS",
  async () => {
    const { withMailFixture } =
      await import("./ported-auth/helpers/mail-product");
    const { registerApplication } =
      await import("../src/modules/auth/bootstrap");
    const { handleNativeStudioRequest } =
      await import("../src/modules/portal/studio-api");
    const { startTenantPreview, exitTenantPreview } =
      await import("../src/modules/access/tenant-preview");
    const previous = process.env.WEBDOCK_DEMO_BRIDGES;
    await withMailFixture(async (f) => {
      const app = await registerApplication({
        label: "CMS fixture",
        origin: "https://fixture.webdock.dev",
        logoutPath: "/login",
        organizationID: f.organizations[0],
        headers: f.operator.headers,
      });
      const call = (headers: Headers, operation: string, args: unknown[]) =>
        handleNativeStudioRequest(
          new Request(process.env.BETTER_AUTH_URL + "/v1/auth/studio", {
            method: "POST",
            headers: new Headers([
              ...headers,
              ["content-type", "application/json"],
            ]),
            body: JSON.stringify({ operation, args }),
          }),
        );
      const actor = {
        userID: f.operator.id,
        sessionID: f.operator.principal.sessionID,
      };
      try {
        process.env.WEBDOCK_DEMO_BRIDGES = JSON.stringify({
          [app.binding]: {
            origin: "https://fixture.webdock.dev",
            secret: "fixture-bridge-secret-must-never-be-returned",
            internalToken: "also-private",
            canvas: true,
          },
        });
        const result = await call(f.admin.headers, "cms.access", [
          app.binding,
          false,
        ]);
        expect(result.status).toBe(200);
        const body = await result.json();
        expect(body.data.site.id).toBe(app.binding);
        expect(body.data.bridge).not.toHaveProperty("secret");
        expect(body.data.bridge).not.toHaveProperty("internalToken");
        expect(
          (await call(f.other.headers, "cms.access", [app.binding, false]))
            .status,
        ).toBe(400);
        await startTenantPreview(actor, f.customers[0]);
        const preview = await call(f.operator.headers, "cms.access", [
          app.binding,
          false,
        ]);
        expect(preview.status).toBe(200);
        expect((await preview.json()).data.readOnly).toBe(true);
        const denyFetch = Object.assign(
          async () => {
            throw Error("Preview attempted an external request");
          },
          { preconnect: () => {} },
        );
        const network = spyOn(globalThis, "fetch").mockImplementation(
          denyFetch,
        );
        try {
          const denied = await call(f.operator.headers, "cms.writeCanvas", [
            app.binding,
            { action: "publish", id: 1, expectedRevision: 1 },
          ]);
          expect(denied.status).toBe(400);
          expect(network).not.toHaveBeenCalled();
        } finally {
          network.mockRestore();
        }
      } finally {
        await exitTenantPreview(actor).catch(() => {});
        await f.db.query(
          "DELETE FROM webdock_auth.project_grant WHERE binding_id=$1",
          [app.binding],
        );
        await f.db.query("DELETE FROM webdock_auth.app_binding WHERE id=$1", [
          app.binding,
        ]);
        await f.db.query(
          'DELETE FROM webdock_auth."oauthClient" WHERE "clientId"=$1',
          [app.clientID],
        );
        if (previous === undefined) delete process.env.WEBDOCK_DEMO_BRIDGES;
        else process.env.WEBDOCK_DEMO_BRIDGES = previous;
      }
    });
  },
);
