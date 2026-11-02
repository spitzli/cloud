import assert from "node:assert/strict";
import test from "node:test";
import { sendTurboMail } from "../../src/modules/mail/product/turbo-send";
const credential = {
  region: "eu" as const,
  consumerKey: "child-key",
  consumerSecret: "child-secret",
};
const body = {
  from: "Sender <sender@example.com>",
  to: ["recipient@example.com"],
  subject: "Test",
  text: "Hello",
  reply_to: "reply@example.com",
};
const reference = "11111111-1111-4111-8111-111111111111";
test("provider send uses bound EU consumer credentials and preserves large numeric MIDs", async () => {
  let calls = 0;
  const outcome = await sendTurboMail(
    credential,
    body,
    reference,
    AbortSignal.timeout(1000),
    async (url, init) => {
      calls++;
      assert.equal(url, "https://api.eu.turbo-smtp.com/api/v2/mail/send");
      const headers = new Headers(init?.headers);
      assert.equal(headers.get("consumerKey"), "child-key");
      assert.equal(headers.has("authorization"), false);
      const sent = JSON.parse(String(init?.body));
      assert.equal(sent.to, "recipient@example.com");
      assert.equal(sent.content, "Hello");
      assert.equal(sent.reference_id, reference);
      assert.equal(sent.custom_headers["Reply-To"], "reply@example.com");
      return new Response('{"message":"OK","mid":18446744073709551615}');
    },
  );
  assert.equal(calls, 1);
  assert.deepEqual(outcome, { kind: "accepted", mid: "18446744073709551615" });
});
test("ambiguous responses and network failures never retry or leak secrets", async () => {
  for (const status of [429, 500, 502])
    assert.deepEqual(
      await sendTurboMail(
        credential,
        body,
        reference,
        AbortSignal.timeout(1000),
        async () => new Response("secret", { status }),
      ),
      { kind: "unknown", code: "provider_response_uncertain" },
    );
  for (const status of [400, 401, 403, 422])
    assert.equal(
      (
        await sendTurboMail(
          credential,
          body,
          reference,
          AbortSignal.timeout(1000),
          async () => new Response("secret", { status }),
        )
      ).kind,
      "rejected",
    );
  let count = 0;
  assert.equal(
    (
      await sendTurboMail(
        credential,
        body,
        reference,
        AbortSignal.timeout(1000),
        async () => {
          count++;
          throw Error("child-secret");
        },
      )
    ).kind,
    "unknown",
  );
  assert.equal(count, 1);
  assert.equal(
    (
      await sendTurboMail(
        credential,
        body,
        reference,
        AbortSignal.timeout(1000),
        async () => new Response("{}"),
      )
    ).kind,
    "unknown",
  );
});
