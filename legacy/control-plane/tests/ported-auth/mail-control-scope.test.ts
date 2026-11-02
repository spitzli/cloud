import test from "node:test";
import assert from "node:assert/strict";
import { database } from "../../src/db/runtime";
import { mailGateway } from "../../src/modules/mail/mail-gateway";
import { outboxSchemaSQL } from "@webdock/mail-core/outbox";
const url = new URL(process.env.DATABASE_URL!);
if (
  url.hostname !== "127.0.0.1" ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local database required");
test.after(() => database.end());
test("browser gateway credentials cannot claim sends or access worker credentials", async () => {
  await database.query(outboxSchemaSQL);
  const before = [
    process.env.MAIL_GATEWAY_SECRET,
    process.env.MAIL_SEND_WORKER_SECRET,
  ];
  process.env.MAIL_GATEWAY_SECRET = "g".repeat(43);
  process.env.MAIL_SEND_WORKER_SECRET = "w".repeat(43);
  const call = (secret: string, body: unknown) =>
    mailGateway(
      new Request("https://auth.example.test/api/mail/gateway", {
        method: "POST",
        headers: {
          Authorization: "Bearer " + secret,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(body),
      }),
    );
  try {
    assert.equal(
      (await call("g".repeat(43), { action: "send.cleanup" })).status,
      403,
    );
    assert.equal(
      (await call("w".repeat(43), { action: "send.cleanup" })).status,
      200,
    );
    assert.equal(
      (await call("w".repeat(43), { action: "exchange", code: "c".repeat(43) }))
        .status,
      403,
    );
    assert.equal(
      (await call("wrong-secret", { action: "send.cleanup" })).status,
      401,
    );
    process.env.MAIL_SEND_WORKER_SECRET = process.env.MAIL_GATEWAY_SECRET;
    assert.equal(
      (await call("g".repeat(43), { action: "send.cleanup" })).status,
      403,
    );
  } finally {
    for (const [index, name] of [
      "MAIL_GATEWAY_SECRET",
      "MAIL_SEND_WORKER_SECRET",
    ].entries()) {
      if (before[index] === undefined) delete process.env[name];
      else process.env[name] = before[index];
    }
  }
});
