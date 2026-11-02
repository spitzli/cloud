import test from "node:test";
import assert from "node:assert/strict";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { hostingSchemaSQL } = await import("../../src/modules/hosting/schema");
const { connectByokMail } =
  await import("../../src/modules/mail/product/onboarding");
const { getTenantTracking, manageTenantTracking } =
  await import("../../src/modules/mail/mail-tracking");
test.after(() => database.end());
test("BYOK tracking uses the bound pair and never leaks into managed snapshots", async () => {
  await database.query(hostingSchemaSQL);
  await withMailProductFixture(async (f) => {
    await f.db.query(
      "INSERT INTO webdock_auth.hosting_byok_policy(customer_id,policy,revision) VALUES($1,$2,1)",
      [
        f.customers[0],
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
    try {
      const connection = await connectByokMail(
        f.adminHeaders,
        f.customers[0],
        {
          consumerKey: "tracking-pair",
          consumerSecret: "tracking-secret",
          expectedRevision: 0,
        },
        {
          verify: async () => ({ accountID: "tracking-" + f.suffix }),
          domains: async () => [],
        },
      );
      await f.db.query(
        `INSERT INTO webdock_auth.mail_tracking_domain(customer_id,sender_domain_id,domain,provider_id,connection_id) VALUES($1,$2,$3,'123',$4)`,
        [
          f.customers[0],
          f.domainA.id,
          "links." + f.domainA.name,
          connection.id,
        ],
      );
      await f.db.query(
        `INSERT INTO webdock_auth.mail_tracking_domain(customer_id,sender_domain_id,domain,provider_id) VALUES($1,$2,$3,'123')`,
        [f.customers[0], f.domainA.id, "managed." + f.domainA.name],
      );
      assert.equal(
        (
          await getTenantTracking(
            f.adminHeaders,
            f.customers[0],
            f.actorA.connectionID,
          )
        ).domains.length,
        1,
      );
      let calls = 0;
      const fetcher: typeof fetch = async (url, init) => {
        calls++;
        const headers = new Headers(init?.headers);
        assert.equal(headers.get("consumerKey"), "tracking-pair");
        assert.equal(headers.get("consumerSecret"), "tracking-secret");
        assert.equal(headers.has("Authorization"), false);
        const path = new URL(String(url)).pathname;
        if (path.endsWith("/domains_info"))
          return Response.json({
            all_domains_disabled: true,
            match_sender: false,
            no_default_domain: true,
          });
        if (path.endsWith("/tools"))
          return Response.json([
            { id: "clickTracking", enabled: false },
            { id: "openingTracking", enabled: false },
            { id: "link_branding", enabled: false },
          ]);
        if (path.endsWith("/link_branding"))
          return Response.json({ count: 0, results: [] });
        throw Error("Unexpected path");
      };
      await f.db.query(
        `UPDATE webdock_auth.platform_settings SET settings=jsonb_set(settings,'{mailEnabled}','false') WHERE id=true`,
      );
      await manageTenantTracking(
        f.adminHeaders,
        f.customers[0],
        { action: "refresh" },
        { fetcher },
        connection.id,
      );
      assert.ok(calls > 0);
      assert.equal(
        (await getTenantTracking(f.adminHeaders, f.customers[0], connection.id))
          .domains.length,
        1,
      );
      assert.equal(
        (await getTenantTracking(f.adminHeaders, f.customers[0])).domains
          .length,
        1,
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*) FROM webdock_auth.mail_tracking_settings WHERE customer_id=$1",
            [f.customers[0]],
          )
        ).rows[0].count,
        "0",
      );
      await assert.rejects(
        getTenantTracking(f.other.headers, f.customers[1], connection.id),
      );
      const before = calls;
      await f.db.query(
        "UPDATE webdock_auth.hosting_turbosmtp SET encrypted_secret=NULL WHERE customer_id=$1",
        [f.customers[0]],
      );
      await assert.rejects(
        manageTenantTracking(
          f.adminHeaders,
          f.customers[0],
          { action: "refresh" },
          { fetcher },
          connection.id,
        ),
      );
      assert.equal(calls, before);
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",
        [f.customers[0]],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_byok_policy WHERE customer_id=$1",
        [f.customers[0]],
      );
    }
  });
});
