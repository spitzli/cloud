import assert from "node:assert/strict";
import test from "node:test";
import {
  parseEmail,
  parseID,
  parsePageQuery,
} from "../../../../packages/mail-contracts/src/validation";
const now = new Date("2026-10-12T12:00:00Z");
const email = {
  from: "Webdock <sender@example.com>",
  to: ["A@example.com"],
  subject: "Order",
  text: "Hello",
};
test("validates normalized recipients and preserves first category", () => {
  const parsed = parseEmail(
    { ...email, cc: ["a@example.com", "B@example.com"], bcc: "b@example.com" },
    now,
  );
  assert.deepEqual(parsed.recipients, [
    { email: "a@example.com", category: "to" },
    { email: "b@example.com", category: "cc" },
  ]);
  assert.equal(parsed.fromEmail, "sender@example.com");
  assert.throws(() =>
    parseEmail(
      {
        ...email,
        to: Array.from({ length: 51 }, (_, i) => `a${i}@example.com`),
      },
      now,
    ),
  );
});
test("rejects header injection, reserved headers, unbounded content and bad attachments", () => {
  for (const bad of [
    { subject: "x\r\nBcc: secret@example.com" },
    { headers: { Bcc: "secret@example.com" } },
    { headers: { "X-Test": "x\r\ny" } },
    { headers: { "Content-Type": "text/plain" } },
    { from: "a@example.com\n" },
    { html: "ü".repeat(300000) },
    { attachments: [{ content: "https://example.com/file" }] },
    { attachments: [{ content: "Zg=" }] },
    { attachments: [{ content: "Zg==", filename: "../secret" }] },
    {
      attachments: [
        { content: Buffer.alloc(2 * 1024 * 1024 + 1).toString("base64") },
      ],
    },
    { tags: [{ name: "space tag", value: "x" }] },
    { scheduled_at: "2026-10-13T12:00:00" },
    { scheduled_at: "2026-11-13T12:00:00Z" },
    { scheduled_at: "2026-10-11T12:00:00Z" },
    { to: ["delivered@test.mail.webdock.dev", "a@example.com"] },
  ])
    assert.throws(() => parseEmail({ ...email, ...bad }, now));
  const good = parseEmail(
    { ...email, attachments: [{ content: "Zg==", filename: "test.txt" }] },
    now,
  );
  assert.equal(good.attachmentBytes, 1);
});
test("boundary IDs and pagination reject ambiguous values", () => {
  assert.equal(parseID("123"), "123");
  for (const id of [
    "0",
    "-1",
    "1.1",
    "1e3",
    123,
    null,
    "999999999999999999999",
  ])
    assert.throws(() => parseID(id));
  assert.deepEqual(parsePageQuery({}), { limit: 20 });
  assert.throws(() => parsePageQuery({ limit: 101 }));
});
test("schedule validation rejects normalized calendar mistakes and invalid clocks", () => {
  for (const scheduled_at of [
    "2027-02-29T12:00:00Z",
    "2026-04-31T12:00:00+02:00",
    "2026-10-13T24:00:00Z",
  ])
    assert.throws(() =>
      parseEmail({ ...email, scheduled_at }, now, { ignoreScheduleTime: true }),
    );
  assert.doesNotThrow(() =>
    parseEmail({ ...email, scheduled_at: "2028-02-29T12:00:00+02:00" }, now, {
      ignoreScheduleTime: true,
    }),
  );
});
