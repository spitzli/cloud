import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { signWebhook } =
  await import("../../src/modules/mail/product/webhook-signing");
const { webhookEndpointProblem, isPublicAddress, postMailWebhook } =
  await import("../../src/modules/mail/product/webhook-transport");
const {
  createMailWebhook,
  rotateMailWebhook,
  deliverDueWebhooks,
  replayMailWebhook,
  updateMailWebhook,
} = await import("../../src/modules/mail/product/webhooks");
const { recordMailEvent } =
  await import("../../src/modules/mail/product/events");
test.after(() => database.end());
test("HMAC binds exact bytes; endpoint and address checks reject private and mixed-network targets", async () => {
  assert.equal(
    signWebhook(
      "whsec_dGVzdC1zZWNyZXQ=",
      "msg_123",
      1700000000,
      '{"type":"email.sent"}',
    ),
    "v1,HXUcGZWkbn/MURsqiftK38hVyeJ+KkubvJWNLvKL49k=",
  );
  for (const url of [
    "http://example.com",
    "https://127.0.0.1",
    "https://[::1]",
    "https://user:pass@example.com",
    "https://service.internal",
    "https://example.com:8443",
  ])
    assert.notEqual(webhookEndpointProblem(url), null);
  for (const ip of [
    "127.0.0.1",
    "10.1.2.3",
    "169.254.169.254",
    "100.64.0.1",
    "::1",
    "fc00::1",
    "::ffff:127.0.0.1",
    "64:ff9b::a00:1",
  ])
    assert.equal(isPublicAddress(ip), false);
  assert.equal(isPublicAddress("8.8.8.8"), true);
  await assert.rejects(
    postMailWebhook("https://example.com", {}, "{}", {
      resolve: async () => [
        { address: "8.8.8.8", family: 4 },
        { address: "10.0.0.1", family: 4 },
      ],
    }),
  );
});
test("encrypted webhook secrets rotate once, retries preserve body/ID and disabled or foreign replays are fenced", async () => {
  await withMailProductFixture(async (f) => {
    const endpoint = await createMailWebhook(f.actorA, {
      url: "https://example.com/hook",
      events: ["email.sent"],
    });
    const row = (
      await f.db.query("SELECT * FROM webdock_auth.mail_webhook WHERE id=$1", [
        endpoint.id,
      ])
    ).rows[0];
    assert.ok(!JSON.stringify(row).includes(endpoint.secret));
    const msg = (
      await f.db.query(
        "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,provider_reference) VALUES($1,$2,$3,'sender@example.com','test','accepted',$4) RETURNING id",
        [
          f.actorA.customerID,
          f.actorA.connectionID,
          f.domainA.id,
          randomUUID(),
        ],
      )
    ).rows[0];
    await recordMailEvent({
      ...f.actorA,
      resourceID: msg.id,
      type: "email.sent",
      source: "application",
      occurredAt: new Date().toISOString(),
      dedupeKey: "send",
    });
    const attempts: { headers: Record<string, string>; body: string }[] = [];
    const transport = async (
      _url: string,
      headers: Record<string, string>,
      body: string,
    ) => {
      attempts.push({ headers, body });
      return {
        status: attempts.length === 1 ? 500 : 204,
        response: "private-token-must-not-persist",
      };
    };
    const tick = () =>
      deliverDueWebhooks(
        { limit: 10, deadline: new Date(Date.now() + 5000) },
        { transport },
      );
    await Promise.all([tick(), tick()]);
    assert.equal(attempts.length, 1);
    const delivery = (
      await f.db.query(
        "SELECT * FROM webdock_auth.mail_webhook_delivery WHERE webhook_id=$1",
        [endpoint.id],
      )
    ).rows[0];
    assert.equal(delivery.attempts, 1);
    assert.equal(delivery.status, "pending");
    const rotated = await rotateMailWebhook(f.actorA, endpoint.id);
    await assert.rejects(replayMailWebhook(f.actorB, delivery.id), {
      status: 404,
    });
    await updateMailWebhook(f.actorA, endpoint.id, { enabled: false });
    await replayMailWebhook(f.actorA, delivery.id);
    await tick();
    assert.equal(attempts.length, 1);
    await updateMailWebhook(f.actorA, endpoint.id, { enabled: true });
    await tick();
    assert.equal(attempts.length, 2);
    assert.equal(attempts[0].body, attempts[1].body);
    assert.equal(
      attempts[0].headers["webhook-id"],
      attempts[1].headers["webhook-id"],
    );
    assert.equal(
      attempts[1].headers["webhook-signature"],
      signWebhook(
        rotated.secret,
        delivery.id,
        Number(attempts[1].headers["webhook-timestamp"]),
        attempts[1].body,
      ),
    );
    assert.ok(
      !JSON.stringify(
        (
          await f.db.query(
            "SELECT * FROM webdock_auth.mail_webhook_attempt WHERE delivery_id=$1",
            [delivery.id],
          )
        ).rows,
      ).includes("private-token-must-not-persist"),
    );
  });
});
test("a crashed final webhook attempt becomes terminal after its lease expires", async () => {
  await withMailProductFixture(async (f) => {
    const endpoint = await createMailWebhook(f.actorA, {
      url: "https://example.com/hook",
      events: ["domain.created"],
    });
    await recordMailEvent({
      ...f.actorA,
      resourceID: f.domainA.id,
      type: "domain.created",
      source: "application",
      occurredAt: new Date().toISOString(),
      dedupeKey: "created",
    });
    await f.db.query(
      "UPDATE webdock_auth.mail_webhook_delivery SET status='delivering',attempts=7,lease_until=now()-interval '1 second' WHERE webhook_id=$1",
      [endpoint.id],
    );
    await deliverDueWebhooks(
      { limit: 10, deadline: new Date(Date.now() + 1000) },
      {
        transport: async () => {
          throw Error("Final attempt must not be repeated");
        },
      },
    );
    assert.equal(
      (
        await f.db.query(
          "SELECT status FROM webdock_auth.mail_webhook_delivery WHERE webhook_id=$1",
          [endpoint.id],
        )
      ).rows[0].status,
      "failed",
    );
  });
});
test("delivery history has bounded pages, attempt metadata and endpoint-bound cursors", async () =>
  withMailProductFixture(async (f) => {
    const { listMailWebhookDeliveries } =
      await import("../../src/modules/mail/product/webhooks");
    const hook = await createMailWebhook(f.actorA, {
      url: "https://example.com/pagination",
      events: ["domain.updated"],
    });
    for (let i = 0; i < 3; i++)
      await recordMailEvent({
        ...f.actorA,
        resourceID: f.domainA.id,
        type: "domain.updated",
        source: "application",
        occurredAt: new Date().toISOString(),
        dedupeKey: "page:" + i,
      });
    const first = await listMailWebhookDeliveries(f.actorA, hook.id, {
      limit: 2,
    });
    assert.equal(first.data.length, 2);
    assert.ok(first.nextCursor);
    const id = first.data[0].id;
    await f.db.query(
      "INSERT INTO webdock_auth.mail_webhook_attempt(customer_id,connection_id,delivery_id,http_status,response,duration_ms) VALUES($1,$2,$3,503,$4,12)",
      [f.customers[0], f.actorA.connectionID, id, "[redacted]"],
    );
    const details = await listMailWebhookDeliveries(f.actorA, hook.id, {
      limit: 2,
    });
    assert.equal(details.data[0].history[0].http_status, 503);
    const second = await listMailWebhookDeliveries(f.actorA, hook.id, {
      limit: 2,
      after: first.nextCursor!,
    });
    assert.equal(second.data.length, 1);
    assert.equal(second.nextCursor, null);
    const another = await createMailWebhook(f.actorA, {
      url: "https://example.com/another",
      events: ["domain.updated"],
    });
    await assert.rejects(
      listMailWebhookDeliveries(f.actorA, another.id, {
        after: first.nextCursor!,
      }),
      { code: "invalid_cursor" },
    );
    await assert.rejects(listMailWebhookDeliveries(f.actorB, hook.id));
  }));
