import assert from "node:assert/strict";
import test from "node:test";
import { readBoundedJson } from "../../src/modules/mail/product/http";
import { redactMailLog } from "../../src/modules/mail/product/request-logs";
test("JSON body is bounded while streaming and validates content type/encoding", async () => {
  const request = (body: string, contentType = "application/json") =>
    new Request("https://example.com", {
      method: "POST",
      headers: { "content-type": contentType },
      body,
    });
  assert.deepEqual(await readBoundedJson(request('{"ok":true}'), 100), {
    ok: true,
  });
  await assert.rejects(readBoundedJson(request("x", "text/plain"), 100), {
    status: 415,
  });
  await assert.rejects(readBoundedJson(request("{"), 100), { status: 422 });
  await assert.rejects(
    readBoundedJson(request('"' + "x".repeat(100) + '"'), 100),
    { status: 413 },
  );
  let canceled = false;
  const stream = new ReadableStream({
    pull(controller) {
      controller.enqueue(new Uint8Array(80));
    },
    cancel() {
      canceled = true;
    },
  });
  const streamed = new Request("https://example.com", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: stream,
    duplex: "half",
  } as RequestInit);
  await assert.rejects(readBoundedJson(streamed, 100), { status: 413 });
  assert.equal(canceled, true);
});
test("logs never retain free text, Bcc, nested secrets, encoded attachments or URL credentials", () => {
  const sentinel = "private-secret-DO-NOT-LOG";
  const redacted = JSON.stringify(
    redactMailLog({
      consumerSecret: sentinel,
      nested: { accessToken: sentinel, signature: sentinel },
      bcc: [sentinel],
      attachments: [{ content: sentinel }],
      html: sentinel,
      text: sentinel,
      url: "https://u:" + sentinel + "@example.com/?token=" + sentinel,
      error: sentinel,
      headers: { "X-Custom": sentinel },
    }),
  );
  assert.ok(!redacted.includes(sentinel));
  assert.ok(!redacted.includes("https://u:"));
  assert.ok(redacted.length <= 32768);
});
