import { test } from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync, sign } from "node:crypto";
import { verifyInvocation } from "../src/server/invocation";
const { publicKey, privateKey } = generateKeyPairSync("ed25519");
const key = publicKey.export({ type: "spki", format: "pem" }).toString();
const claims = {
  v: 1,
  id: "123e4567-e89b-42d3-a456-426614174000",
  pluginID: "managed-analytics",
  generation: 1,
  action: "stats",
  projectID: "123",
  customerID: "101",
  iat: 1000,
  exp: 1060,
};
function token(payload: any) {
  const body = Buffer.from(JSON.stringify(payload)).toString("base64url");
  return (
    body + "." + sign(null, Buffer.from(body), privateKey).toString("base64url")
  );
}
test("verifies host-issued invocation identity and expiration", () => {
  assert.deepEqual(
    verifyInvocation(token(claims), {
      publicKey: key,
      pluginID: "managed-analytics",
      generation: 1,
      now: 1001,
    }),
    claims,
  );
});
test("rejects tampering, other runtimes, expired and long-lived invocations", () => {
  const opts = {
    publicKey: key,
    pluginID: "managed-analytics",
    generation: 1,
    now: 1001,
  };
  for (const c of [
    { ...claims, pluginID: "other" },
    { ...claims, generation: 2 },
    { ...claims, exp: 1000 },
    { ...claims, exp: 1061 },
    { ...claims, iat: 1002 },
    { ...claims, customerID: undefined },
    { ...claims, customerID: "01" },
    { ...claims, customerID: "other" },
  ])
    assert.throws(() => verifyInvocation(token(c), opts));
  assert.throws(() => verifyInvocation(token(claims) + "x", opts));
  assert.throws(() => verifyInvocation("x".repeat(16385), opts));
});
