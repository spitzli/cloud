import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { hostingSchemaSQL } = await import("../../src/modules/hosting/schema");
const { connectByokMail } =
  await import("../../src/modules/mail/product/onboarding");
const { resolveSendingCredential } =
  await import("../../src/modules/mail/product/provider-credentials");
test.after(() => database.end());
test("two BYOK pairs from one real account cannot cross tenants; rotation preserves identity and changed legacy secrets block sends", async () => {
  await database.query(hostingSchemaSQL);
  await withMailProductFixture(async (f) => {
    for (const customerID of f.customers)
      await f.db.query(
        "INSERT INTO webdock_auth.hosting_byok_policy(customer_id,policy,revision) VALUES($1,$2,1)",
        [
          customerID,
          JSON.stringify({
            kubernetes: false,
            vercel: false,
            turbosmtp: true,
            maxClusters: 0,
            manageExisting: false,
            namespaces: [],
          }),
        ],
      );
    const deps = {
      verify: async () => ({ accountID: "byok-" + f.suffix }),
      domains: async () => [
        {
          id: "verified",
          domain: f.domainA.name,
          spf_verified: true,
          dkim_verified: true,
          dmarc_verified: true,
        },
      ],
    };
    try {
      const connection = await connectByokMail(
        f.adminHeaders,
        f.actorA.customerID,
        {
          consumerKey: "pair-one",
          consumerSecret: "secret-one",
          expectedRevision: 0,
        },
        deps,
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT snapshot FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",
            [f.actorA.customerID],
          )
        ).rows[0].snapshot[0]?.domain,
        f.domainA.name,
      );
      await assert.rejects(
        connectByokMail(
          f.other.headers,
          f.actorB.customerID,
          {
            consumerKey: "pair-two",
            consumerSecret: "secret-two",
            expectedRevision: 0,
          },
          deps,
        ),
        { code: "account_already_bound" },
      );
      const rotated = await connectByokMail(
        f.adminHeaders,
        f.actorA.customerID,
        {
          consumerKey: "pair-three",
          consumerSecret: "secret-three",
          expectedRevision: 1,
        },
        deps,
      );
      assert.equal(rotated.id, connection.id);
      const actor = { ...f.actorA, connectionID: connection.id };
      assert.equal(
        (await resolveSendingCredential(actor)).consumerKey,
        "pair-three",
      );
      await f.db.query(
        "UPDATE webdock_auth.hosting_turbosmtp SET encrypted_secret=NULL,revision=revision+1 WHERE customer_id=$1",
        [f.actorA.customerID],
      );
      await assert.rejects(resolveSendingCredential(actor));
      await assert.rejects(
        connectByokMail(
          f.memberHeaders,
          f.actorA.customerID,
          { consumerKey: "x", consumerSecret: "y", expectedRevision: 3 },
          deps,
        ),
      );
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_turbosmtp WHERE customer_id=ANY($1)",
        [f.customers],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_byok_policy WHERE customer_id=ANY($1)",
        [f.customers],
      );
    }
  });
});
