import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import test from "node:test";
import { sealSecret, openSecret } from "@webdock/mail-core/secrets";

test("Mail secrets are encrypted, purpose-bound and reject tampering or a wrong key", () => {
  const key = randomBytes(32).toString("base64");
  const value = { username: "operator", secret: "this-must-not-appear-in-the-ciphertext" };
  const sealed = sealSecret(value, "instance:123", key);
  assert.ok(!sealed.includes(value.secret));
  assert.deepEqual(openSecret(sealed, "instance:123", key), value);
  assert.throws(() => openSecret(sealed, "instance:456", key));
  assert.throws(() => openSecret(sealed, "instance:123", randomBytes(32).toString("base64")));
  const parts = sealed.split(".");
  const ciphertext = Buffer.from(parts[3], "base64url"); ciphertext[0] ^= 1;
  parts[3] = ciphertext.toString("base64url");
  assert.throws(() => openSecret(parts.join("."), "instance:123", key));
  assert.throws(() => sealSecret(value, "instance:123", "short"));
});

test('Mail ciphertext rejects shortened GCM tags and non-canonical encodings',()=>{
 const key=randomBytes(32).toString('base64');
 const sealed=sealSecret({password:'fixture'},'strict-envelope',key);
 const short=sealed.split('.');short[2]=Buffer.from(short[2],'base64url').subarray(0,4).toString('base64url');
 assert.throws(()=>openSecret(short.join('.'),'strict-envelope',key));
 const padded=sealed.split('.');padded[1]+='=';
 assert.throws(()=>openSecret(padded.join('.'),'strict-envelope',key));
});
