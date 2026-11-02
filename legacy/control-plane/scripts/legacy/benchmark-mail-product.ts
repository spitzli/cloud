/** Synthetic local benchmark only. Never run against any application/provider database. */
import { writeFileSync } from "node:fs";
import { performance } from "node:perf_hooks";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "../../tests/ported-auth/helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
process.env.PGAPPNAME = "webdock-relaykit-benchmark";
const { database } = await import("../../src/db/runtime");
const { auth } = await import("../../src/modules/auth/auth");
const { getMailAnalytics } =
  await import("../../src/modules/mail/product/analytics");
const { listMailMessages } =
  await import("../../src/modules/mail/product/messages");
const { createMailKey } =
  await import("../../src/modules/mail/product/api-keys");
const { GET } =
  await import("../../src/protocols/legacy-auth/api/mail/v1/emails/route");
const output = process.argv[2];
if (!output) throw Error("Pass an explicit local output JSON path");
const sample = async (run: () => Promise<unknown>, count: number) => {
  const times: number[] = [];
  for (let i = 0; i < count; i++) {
    const at = performance.now();
    await run();
    times.push(performance.now() - at);
  }
  times.sort((a, b) => a - b);
  return {
    count,
    p50Ms: times[Math.floor(times.length * 0.5)],
    p95Ms:
      times[Math.min(times.length - 1, Math.ceil(times.length * 0.95) - 1)],
    maxMs: times.at(-1),
  };
};
try {
  await withMailProductFixture(async (f) => {
    const started = Date.now();
    const customers = (
      await f.db.query(
        "INSERT INTO webdock_admin.customers(id,name) SELECT webdock_auth.next_snowflake(),'Mail benchmark' FROM generate_series(1,99) RETURNING id",
      )
    ).rows.map((r) => r.id);
    f.customers.push(...customers);
    f.organizations.push(
      ...(
        await f.db.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
          [customers],
        )
      ).rows.map((r) => r.organization_id),
    );
    await f.db.query(
      "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id,state) SELECT id,'platform','benchmark-'||id,'ready' FROM webdock_admin.customers WHERE id=ANY($1)",
      [customers],
    );
    await f.db.query(
      "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token,status) SELECT id,'benchmark-'||id||'.example.invalid','benchmark','ownership_verified' FROM webdock_admin.customers WHERE id=ANY($1)",
      [customers],
    );
    await f.db.query(
      `INSERT INTO webdock_auth.mail_connection_domain(customer_id,connection_id,domain_id,provider_id,provider_snapshot,provider_checked_at)
 SELECT c.customer_id,c.id,d.id,'benchmark-'||d.id,jsonb_build_object('id','benchmark-'||d.id,'domain',d.domain,'spf_verified',true,'dkim_verified',true,'dmarc_verified',true),now()
 FROM webdock_auth.mail_connection c JOIN webdock_auth.mail_sender_domain d ON d.customer_id=c.customer_id WHERE c.customer_id=ANY($1)`,
      [customers],
    );
    const selected = [f.actorA.customerID, ...customers];
    await f.db.query(
      `INSERT INTO webdock_auth.mail_message(id,customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference,accepted_at,terminal_at,created_at)
 SELECT (7000000000000000000::bigint+row_number() OVER())::text,c.customer_id,c.id,d.id,'sender@'||d.domain,'Synthetic benchmark '||n,'accepted',gen_random_uuid(),now()-(n%30)*interval '1 day',now()-(n%30)*interval '1 day',now()-(n%30)*interval '1 day'
 FROM webdock_auth.mail_connection c JOIN webdock_auth.mail_sender_domain d ON d.customer_id=c.customer_id CROSS JOIN generate_series(1,1000) n WHERE c.customer_id=ANY($1)`,
      [selected],
    );
    await f.db.query(
      `INSERT INTO webdock_auth.mail_recipient(customer_id,connection_id,message_id,email,category,delivery_status,included_in_handoff,first_delivered_at)
 SELECT customer_id,connection_id,id,'benchmark@example.invalid','to','delivered',true,accepted_at FROM webdock_auth.mail_message WHERE customer_id=ANY($1)`,
      [selected],
    );
    for (const [index, customer] of selected.entries())
      await f.db.query(
        `INSERT INTO webdock_auth.mail_event(id,customer_id,connection_id,resource_id,message_id,type,source,dedupe_key,occurred_at,created_at)
 SELECT (7100000000000000000::bigint+$2::bigint*10000+row_number() OVER())::text,m.customer_id,m.connection_id,m.id,m.id,'email.opened','application','benchmark:'||m.id||':'||n,m.accepted_at,m.accepted_at FROM webdock_auth.mail_message m CROSS JOIN generate_series(1,10) n WHERE m.customer_id=$1`,
        [customer, index],
      );
    await f.db.query("ANALYZE webdock_auth.mail_message");
    await f.db.query("ANALYZE webdock_auth.mail_event");
    await f.db.query("ANALYZE webdock_auth.mail_recipient");
    const list = await sample(
        () => listMailMessages(f.actorA, { limit: 20 }),
        30,
      ),
      analytics = await sample(
        () => getMailAnalytics(f.actorA, { days: 90 }),
        20,
      );
    const key = await createMailKey(f.actorA, {
        name: "local benchmark",
        permission: "full_access",
      }),
      apiTimes: number[] = [];
    for (let second = 0; second < 5; second++) {
      const at = Date.now();
      await Promise.all(
        Array.from({ length: 20 }, async () => {
          const before = performance.now();
          const response = await GET(
            new Request("https://auth.webdock.dev/api/mail/v1/emails", {
              headers: { Authorization: "Bearer " + key.key },
            }),
          );
          if (response.status !== 200)
            throw Error("Benchmark request failed: " + response.status);
          await response.arrayBuffer();
          apiTimes.push(performance.now() - before);
        }),
      );
      if (Date.now() - at < 1000)
        await new Promise((resolve) =>
          setTimeout(resolve, 1000 - (Date.now() - at)),
        );
    }
    apiTimes.sort((a, b) => a - b);
    const explain = (
      await f.db.query(
        `EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) SELECT id,subject FROM webdock_auth.mail_message WHERE customer_id=$1 AND connection_id=$2 ORDER BY created_at DESC,id DESC LIMIT 20`,
        [f.actorA.customerID, f.actorA.connectionID],
      )
    ).rows[0]["QUERY PLAN"];
    const report = {
      at: new Date().toISOString(),
      environment:
        "disposable local PostgreSQL 17; Node 24; in-process route handlers",
      tenants: 100,
      messages: 100000,
      events: 1000000,
      setupMs: Date.now() - started,
      list,
      analytics,
      api: {
        requests: 100,
        offeredRequestsPerSecond: 20,
        p95Ms: apiTimes[94],
        maxMs: apiTimes.at(-1),
      },
      explain,
      productionSLA: false,
      dueLag: "not measured by this metadata benchmark",
    };
    writeFileSync(output, JSON.stringify(report, null, 2) + "\n", {
      mode: 0o600,
    });
    console.log(
      JSON.stringify({
        output,
        listP95Ms: list.p95Ms,
        analyticsP95Ms: analytics.p95Ms,
        apiP95Ms: apiTimes[94],
      }),
    );
  });
} finally {
  await auth.$context;
  await database.end();
}
