import assert from "node:assert/strict";
import test from "node:test";
import {
  assertMailTestDatabase,
  withMailProductFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { reserveMailRequest, completeMailRequest, canonicalHash } =
  await import("../../src/modules/mail/product/idempotency");
test.after(() => database.end());
test("canonical hashes ignore object key order but preserve recipient order", () => {
  assert.equal(
    canonicalHash({ a: 1, b: { c: 2 } }),
    canonicalHash({ b: { c: 2 }, a: 1 }),
  );
  assert.notEqual(
    canonicalHash({ to: ["a", "b"] }),
    canonicalHash({ to: ["b", "a"] }),
  );
});
test("parallel reservations never duplicate work and unknown outcomes retain keys", async () => {
  await withMailProductFixture(async (f) => {
    const input = {
      key: "fixture",
      method: "POST",
      path: "/emails",
      body: { subject: "test" },
      now: f.now,
    };
    const results = await Promise.allSettled([
      reserveMailRequest(f.actorA, input),
      reserveMailRequest(f.actorA, input),
    ]);
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    const first = results.find((r) => r.status === "fulfilled");
    assert.ok(first?.status === "fulfilled" && first.value.kind === "reserved");
    await completeMailRequest(
      f.actorA,
      "fixture",
      first.value.token,
      { status: 502, body: { name: "send_outcome_unknown" } },
      true,
      f.now,
    );
    f.setNow(new Date(f.now.getTime() + 8 * 86400_000));
    const replay = await reserveMailRequest(f.actorA, { ...input, now: f.now });
    assert.deepEqual(replay, {
      kind: "replay",
      result: { status: 502, body: { name: "send_outcome_unknown" } },
    });
    await assert.rejects(
      reserveMailRequest(f.actorA, {
        ...input,
        body: { subject: "different" },
        now: f.now,
      }),
    );
    const other = await reserveMailRequest(f.actorB, { ...input, now: f.now });
    assert.equal(other.kind, "reserved");
    if (other.kind !== "reserved") throw Error();
    await completeMailRequest(
      f.actorB,
      "fixture",
      other.token,
      { status: 200, body: { id: "123" } },
      false,
      f.now,
    );
    assert.deepEqual(
      await reserveMailRequest(f.actorB, { ...input, now: f.now }),
      { kind: "replay", result: { status: 200, body: { id: "123" } } },
    );
  });
});

test("completed replays still require identity but survive connection pause and gateway disable", async () => {
  await withMailProductFixture(async (f) => {
    const input = {
      key: "paused-replay",
      method: "POST",
      path: "/emails",
      body: { subject: "test" },
      now: f.now,
    };
    const reserved = await reserveMailRequest(f.actorA, input);
    assert.equal(reserved.kind, "reserved");
    if (reserved.kind !== "reserved") throw Error();
    await completeMailRequest(
      f.actorA,
      input.key,
      reserved.token,
      { status: 200, body: { id: "123" } },
      false,
      f.now,
    );
    await f.db.query(
      "UPDATE webdock_auth.mail_connection SET state='paused' WHERE id=$1",
      [f.actorA.connectionID],
    );
    await f.db.query(
      "UPDATE webdock_auth.platform_settings SET settings=settings || '{\"mailGatewayEnabled\":false}'::jsonb WHERE id=true",
    );
    assert.equal((await reserveMailRequest(f.actorA, input)).kind, "replay");
    await assert.rejects(
      reserveMailRequest(f.actorA, { ...input, key: "new-request" }),
    );
    await f.db.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
      f.admin.id,
    ]);
    await assert.rejects(reserveMailRequest(f.actorA, input));
  });
});
