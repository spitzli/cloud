import assert from "node:assert/strict";
import test from "node:test";
import {
  sealMailAttachments,
  openMailAttachments,
} from "../../src/modules/mail/product/content";
test("attachment ciphertext is bound to tenant, message and index with independent size limits", () => {
  const context = { customerID: "123", messageID: "456" },
    secret = "x".repeat(48),
    items = [{ content: "aGVsbG8=", filename: "test.txt" }];
  const sealed = sealMailAttachments(context, items, secret);
  assert.ok(!sealed.includes("aGVsbG8="));
  assert.deepEqual(openMailAttachments(context, sealed, secret), ["aGVsbG8="]);
  assert.throws(() =>
    openMailAttachments({ ...context, customerID: "789" }, sealed, secret),
  );
  assert.throws(() =>
    openMailAttachments({ ...context, messageID: "789" }, sealed, secret),
  );
  assert.throws(() =>
    openMailAttachments(context, sealed.slice(0, -2) + "xx", secret),
  );
  assert.throws(() =>
    openMailAttachments(context, "x".repeat(4_000_001), secret),
  );
});
