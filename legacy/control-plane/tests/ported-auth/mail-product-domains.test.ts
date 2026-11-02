import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { assertVerifiedSender, claimMailDomain, removeMailDomain } =
  await import("../../src/modules/mail/product/domains");
const { createMailKey, authorizeMailKey } =
  await import("../../src/modules/mail/product/api-keys");
test.after(() => database.end());
test("ownership is mandatory and domain deletion never widens a restricted key", async () => {
  await withMailProductFixture(async (f) => {
    const key = await createMailKey(f.actorA, {
      name: "orders",
      permission: "sending_access",
      domainID: f.domainA.id,
    });
    const actor = await authorizeMailKey(
      new Request("https://auth.webdock.dev/api/mail/v1/emails", {
        headers: { Authorization: "Bearer " + key.key },
      }),
      "sending_access",
    );
    assert.equal(
      (await assertVerifiedSender(actor, f.domainA.name)).domainID,
      f.domainA.id,
    );
    await f.db.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='pending' WHERE id=$1",
      [f.domainA.id],
    );
    await assert.rejects(assertVerifiedSender(actor, f.domainA.name));
    await f.db.query(
      "UPDATE webdock_auth.mail_sender_domain SET status='ownership_verified' WHERE id=$1",
      [f.domainA.id],
    );
    await removeMailDomain(f.actorA, f.domainA.id);
    const second = await claimMailDomain(f.actorA, f.domainA.name);
    assert.notEqual(second.id, f.domainA.id);
    assert.equal(
      (
        await f.db.query(
          "SELECT domain_id FROM webdock_auth.mail_api_key WHERE id=$1",
          [key.id],
        )
      ).rows[0].domain_id,
      f.domainA.id,
    );
    await assert.rejects(assertVerifiedSender(actor, f.domainA.name));
    await assert.rejects(assertVerifiedSender(actor, "another.example"));
    await assert.rejects(removeMailDomain(f.actorB, second.id));
  });
});

test("connection-scoped deletion cannot invalidate a shared tenant domain", async () => {
  await withMailProductFixture(async (f) => {
    const second = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id,state) VALUES($1,'byok',$2,'ready') RETURNING id",
        [f.actorA.customerID, "secondary-" + f.suffix],
      )
    ).rows[0].id;
    await f.db.query(
      "INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id) VALUES($1,$2,$3)",
      [f.actorA.customerID, second, f.domainA.id],
    );
    await assert.rejects(removeMailDomain(f.actorA, f.domainA.id), {
      code: "domain_in_use",
    });
    assert.equal(
      (
        await f.db.query(
          "SELECT deleted_at FROM webdock_auth.mail_sender_domain WHERE id=$1",
          [f.domainA.id],
        )
      ).rows[0].deleted_at,
      null,
    );
  });
});

test("verification shares the legacy operation fence and stale registration cannot authorize sends", async () => {
  await withMailProductFixture(async (f) => {
    const { verifyMailDomain } =
      await import("../../src/modules/mail/product/domains");
    let calls = 0;
    const deps = {
      lookup: async () => ({
        root: [],
        dkim: [],
        dmarc: [],
        ownership: ["webdock=" + f.suffix],
      }),
      provider: async () => ({
        registerSenderDomain: async () => {
          calls++;
          return {
            id: "fixture-domain",
            domain: f.domainA.name,
            spf_verified: true,
            dkim_verified: true,
            dmarc_verified: true,
          };
        },
        listSenderDomains: async () => [],
      }),
    };
    await f.db.query(
      "UPDATE webdock_auth.mail_sender_domain SET provider_operation='legacy-unregister',provider_operation_started_at=now() WHERE id=$1",
      [f.domainA.id],
    );
    await assert.rejects(verifyMailDomain(f.actorA, f.domainA.id, deps), {
      code: "domain_busy",
    });
    await assert.rejects(assertVerifiedSender(f.actorA, f.domainA.name));
    assert.equal(calls, 0);
    await f.db.query(
      "UPDATE webdock_auth.mail_sender_domain SET provider_operation=NULL,provider_id=NULL WHERE id=$1",
      [f.domainA.id],
    );
    await assert.rejects(assertVerifiedSender(f.actorA, f.domainA.name));
  });
});
