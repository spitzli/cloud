import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { createMailKey } =
  await import("../../src/modules/mail/product/api-keys");
const emails =
  await import("../../src/protocols/legacy-auth/api/mail/v1/emails/route");
const keys =
  await import("../../src/protocols/legacy-auth/api/mail/v1/api-keys/route");
const cancel =
  await import("../../src/protocols/legacy-auth/api/mail/v1/emails/[id]/cancel/route");
test.after(() => database.end());
test("real route exports enforce permissions, bounded bodies, idempotent sends and redacted logs", async () => {
  await withMailProductFixture(async (f) => {
    const key = await createMailKey(f.actorA, {
      name: "API test",
      permission: "sending_access",
    });
    const full = await createMailKey(f.actorA, {
      name: "API admin",
      permission: "full_access",
    });
    const req = (
      path: string,
      method: string,
      body?: unknown,
      token = key.key,
      extra: Record<string, string> = {},
    ) =>
      new Request("https://auth.webdock.dev/api/mail/v1/" + path, {
        method,
        headers: {
          authorization: "Bearer " + token,
          "content-type": "application/json",
          ...extra,
        },
        ...(body !== undefined ? { body: JSON.stringify(body) } : {}),
      });
    assert.equal((await keys.GET(req("api-keys", "GET"))).status, 403);
    assert.equal(
      (
        await emails.POST(
          new Request("https://example.com", { method: "POST" }),
        )
      ).status,
      401,
    );
    const body = {
      from: "sender@" + f.domainA.name,
      to: "delivered@test.mail.webdock.dev",
      subject: "API",
      text: "body-must-not-appear-in-logs",
    };
    const first = await emails.POST(
      req("emails", "POST", body, key.key, { "idempotency-key": "route-test" }),
    );
    assert.equal(first.status, 200);
    assert.equal(first.headers.get("cache-control"), "no-store");
    const result = await first.json();
    const again = await emails.POST(
      req("emails", "POST", body, key.key, { "idempotency-key": "route-test" }),
    );
    assert.deepEqual(await again.json(), result);
    assert.equal(
      again.headers.get("x-request-id"),
      first.headers.get("x-request-id"),
    );
    assert.equal(
      (await emails.GET(req("emails", "GET", undefined, full.key))).status,
      200,
    );
    assert.equal(
      (
        await cancel.POST(
          req("emails/" + result.id + "/cancel", "POST", {}, full.key),
          { params: Promise.resolve({ id: result.id }) },
        )
      ).status,
      428,
    );
    const rows = (
      await f.db.query(
        "SELECT * FROM webdock_auth.mail_request_log WHERE customer_id=$1",
        [f.actorA.customerID],
      )
    ).rows;
    assert.ok(rows.length >= 3);
    assert.ok(!JSON.stringify(rows).includes(key.key));
    assert.ok(!JSON.stringify(rows).includes(body.text));
  });
});
test("resource search and log filters bind their pagination cursor", async () =>
  withMailProductFixture(async (f) => {
    const { listMailResource } =
      await import("../../src/modules/mail/product/resource-lists");
    for (let i = 0; i < 3; i++)
      await createMailKey(f.actorA, {
        name: "search-match-" + i,
        permission: "sending_access",
      });
    const first = await listMailResource(f.actorA, "keys", {
      search: "search-match",
      limit: 1,
    });
    assert.equal(first.data.length, 1);
    assert.ok(first.nextCursor);
    await assert.rejects(
      listMailResource(f.actorA, "keys", {
        search: "different",
        limit: 1,
        after: first.nextCursor!,
      }),
      { code: "invalid_cursor" },
    );
    await f.db.query(
      "INSERT INTO webdock_auth.mail_request_log(customer_id,connection_id,route,method,status,duration_ms,request_id) VALUES($1,$2,'/emails','POST',0,0,'unfinished-test'),($1,$2,'/emails','POST',200,4,'completed-test')",
      [f.customers[0], f.actorA.connectionID],
    );
    const logs = await listMailResource(f.actorA, "logs", {
      search: "unfinished",
      method: "POST",
      status: 0,
    });
    assert.equal(logs.data.length, 1);
    assert.equal(logs.data[0].status, 0);
    await assert.rejects(listMailResource(f.actorA, "logs", { status: 42 }));
  }));
