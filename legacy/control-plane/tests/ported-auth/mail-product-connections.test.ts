import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { ensureManagedSendingCredential, resolveSendingCredential } =
  await import("../../src/modules/mail/product/provider-credentials");
test.after(() => database.end());
test("private SEND_API lifecycle persists encrypted exact child credentials and never blindly retries", async () => {
  await withMailProductFixture(async (f) => {
    const operator = { ...f.actorA, principal: f.operator.principal };
    const account = {
      id: "fixture-" + f.actorA.customerID,
      email: f.suffix + "@example.invalid",
      active: true,
    };
    await f.db.query(
      "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready',$4)",
      [
        f.actorA.customerID,
        account.id,
        account.email,
        JSON.stringify({ active: true }),
      ],
    );
    let creates = 0;
    const deps = {
      master: async () => ({
        getSubaccount: async (id: string | number) => {
          assert.equal(id, account.id);
          return account;
        },
        authorizeSubaccount: async (email: string) => {
          assert.equal(email, account.email);
          return "child";
        },
      }),
      child: (token: string) => {
        assert.equal(token, "child");
        return {
          createConsumerKey: async (
            token: string,
            label: string,
            options?: { permissions?: string[] },
          ) => {
            assert.equal(token, "child");
            assert.deepEqual(options?.permissions, ["SEND_API"]);
            assert.match(label, /^Webdock internal mail /);
            creates++;
            return {
              consumerKey: "internal-key",
              consumerSecret: "internal-secret",
            };
          },
        };
      },
    };
    try {
      await ensureManagedSendingCredential(operator, deps);
      await ensureManagedSendingCredential(operator, deps);
      assert.equal(creates, 1);
      assert.deepEqual(await resolveSendingCredential(f.actorA), {
        consumerKey: "internal-key",
        consumerSecret: "internal-secret",
        region: "eu",
      });
      const secret = (
        await f.db.query(
          "SELECT encrypted_secret FROM webdock_auth.mail_connection_secret WHERE connection_id=$1",
          [f.actorA.connectionID],
        )
      ).rows[0].encrypted_secret;
      assert.ok(!secret.includes("internal-secret"));
      await assert.rejects(resolveSendingCredential(f.actorB));
      await f.db.query(
        "DELETE FROM webdock_auth.mail_connection_secret WHERE connection_id=$1",
        [f.actorA.connectionID],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_provider_operation WHERE connection_id=$1",
        [f.actorA.connectionID],
      );
      await assert.rejects(
        ensureManagedSendingCredential(operator, {
          ...deps,
          child: () => ({
            createConsumerKey: async () => {
              creates++;
              throw Error("lost one-time response");
            },
          }),
        }),
      );
      await assert.rejects(ensureManagedSendingCredential(operator, deps));
      assert.equal(creates, 2);
      assert.equal(
        (
          await f.db.query(
            "SELECT state FROM webdock_auth.mail_connection WHERE id=$1",
            [f.actorA.connectionID],
          )
        ).rows[0].state,
        "needs_review",
      );
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
        [f.actorA.customerID],
      );
    }
  });
});
test("managed credential rotation stores the new version before revoking the old child key", async () =>
  withMailProductFixture(async (f) => {
    const { rotateManagedSendingCredential } =
      await import("../../src/modules/mail/product/provider-credentials");
    const operator = { ...f.actorA, principal: f.operator.principal },
      account = {
        id: "fixture-" + f.actorA.customerID,
        email: f.suffix + "@example.invalid",
        active: true,
      };
    await f.db.query(
      "INSERT INTO webdock_auth.mail_tenant_account(customer_id,provider_id,email,state,snapshot) VALUES($1,$2,$3,'ready',$4)",
      [
        f.actorA.customerID,
        account.id,
        account.email,
        JSON.stringify({ active: true }),
      ],
    );
    let version = 0,
      revoked = 0;
    const deps = {
      master: async () => ({
        getSubaccount: async () => account,
        authorizeSubaccount: async () => "child",
      }),
      child: () => ({
        createConsumerKey: async () => ({
          consumerKey: "key-" + ++version,
          consumerSecret: "secret-" + version,
        }),
        deleteConsumerKey: async (key: string) => {
          assert.equal(key, "key-1");
          assert.equal(
            (await resolveSendingCredential(f.actorA)).consumerKey,
            "key-2",
          );
          revoked++;
        },
      }),
    };
    try {
      await ensureManagedSendingCredential(operator, deps);
      await rotateManagedSendingCredential(operator, deps);
      assert.equal(revoked, 1);
      const rows = (
        await f.db.query(
          "SELECT version,active,revoked_at IS NOT NULL AS revoked FROM webdock_auth.mail_connection_secret WHERE connection_id=$1 ORDER BY version",
          [f.actorA.connectionID],
        )
      ).rows;
      assert.deepEqual(rows, [
        { version: 1, active: false, revoked: true },
        { version: 2, active: true, revoked: false },
      ]);
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_tenant_account WHERE customer_id=$1",
        [f.actorA.customerID],
      );
    }
  }));
