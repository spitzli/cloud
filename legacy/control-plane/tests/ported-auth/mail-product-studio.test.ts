import { configureFullProducts } from "../../src/full-products";
configureFullProducts();
import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { handleStudioRequest } =
  await import("../../src/modules/portal/studio-api");
const { startTenantPreview, exitTenantPreview } =
  await import("../../src/modules/access/tenant-preview");
test.after(() => database.end());
test("registered Mail bridge operations retain live membership and deny hidden preview writes", async () => {
  await withMailProductFixture(async (f) => {
    const previous = process.env.WEBDOCK_STUDIO_CLIENT_ID;
    process.env.WEBDOCK_STUDIO_CLIENT_ID = "mail-studio-fixture";
    const call = (
      principal: typeof f.admin.principal,
      operation: string,
      args: unknown[],
    ) =>
      handleStudioRequest(
        new Request("http://localhost:3125/api/studio", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization:
              "Basic " +
              Buffer.from("mail-studio-fixture:fixture-secret").toString(
                "base64",
              ),
          },
          body: JSON.stringify({
            operation,
            args,
            accessToken: "fixture-introspection-token",
          }),
        }),
        {
          introspect: async () =>
            Response.json({
              active: true,
              client_id: "mail-studio-fixture",
              sub: principal.userID,
              sid: principal.sessionID,
              exp: Math.floor(Date.now() / 1000) + 3600,
            }),
        },
      );
    try {
      assert.equal(
        (
          await call(f.admin.principal, "mailMessages", [
            f.actorA.customerID,
            f.actorA.connectionID,
            {},
          ])
        ).status,
        200,
      );
      assert.equal(
        (
          await call(f.admin.principal, "mailMessages", [
            f.actorB.customerID,
            f.actorB.connectionID,
            {},
          ])
        ).status,
        404,
      );
      assert.equal(
        (
          await call(f.member.principal, "mailKeys", [
            f.actorA.customerID,
            f.actorA.connectionID,
            {},
          ])
        ).status,
        404,
      );
      const who = {
        userID: f.operator.id,
        sessionID: f.operator.principal.sessionID,
      };
      await startTenantPreview(who, f.actorA.customerID);
      try {
        assert.equal(
          (
            await call(f.operator.principal, "mailTestSend", [
              f.actorA.customerID,
              f.actorA.connectionID,
              {
                from: "sender@" + f.domainA.name,
                to: "delivered@test.mail.webdock.dev",
                subject: "test",
                text: "test",
                idempotencyKey: "preview",
              },
            ])
          ).status,
          403,
        );
      } finally {
        await exitTenantPreview(who);
      }
    } finally {
      if (previous === undefined) delete process.env.WEBDOCK_STUDIO_CLIENT_ID;
      else process.env.WEBDOCK_STUDIO_CLIENT_ID = previous;
    }
  });
});
