import { test } from "node:test";
import assert from "node:assert/strict";
import { generateKeyPair, exportJWK, CompactSign } from "jose";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { verifyRelease } from "./integrity";
const hash = (s: string) => createHash("sha256").update(s).digest("hex");
async function fixture() {
  const pair = await generateKeyPair("EdDSA");
  const jwk = await exportJWK(pair.publicKey);
  const raw = readFileSync(new URL("../../../../tools/plugins/fixtures/project-notes.json", import.meta.url), "utf8");
  const m = JSON.parse(raw);
  const payload = {
    v: 1,
    registry: "https://registry.example.com",
    plugin_slug: m.name,
    release_version: m.version,
    kid: "trusted",
    manifest_sha256: hash(raw),
    assets: [
      {
        name: m.name + "-universal.json",
        sha256: hash(raw),
        size: Buffer.byteLength(raw),
      },
    ],
  };
  const jws = await new CompactSign(Buffer.from(JSON.stringify(payload)))
    .setProtectedHeader({ alg: "EdDSA", kid: "trusted" })
    .sign(pair.privateKey);
  return { raw, jws, jwk, payload, pair, m };
}
test("accepts only matching signed identity and exact descriptor bytes", async () => {
  const f = await fixture();
  const r = await verifyRelease({
    registryURL: f.payload.registry,
    id: f.m.name,
    version: f.m.version,
    kid: "trusted",
    key: f.jwk,
    jws: f.jws,
    manifestRaw: f.raw,
    descriptor: f.raw,
  });
  assert.equal(r.manifest.name, f.m.name);
  assert.equal(r.manifestSha256, hash(f.raw));
});
test("rejects tampering, unknown keys, wrong registry and wrong versions", async () => {
  const f = await fixture();
  const input = {
    registryURL: f.payload.registry,
    id: f.m.name,
    version: f.m.version,
    kid: "trusted",
    key: f.jwk,
    jws: f.jws,
    manifestRaw: f.raw,
    descriptor: f.raw,
  };
  for (const change of [
    { descriptor: f.raw + " " },
    { manifestRaw: f.raw + " " },
    { kid: "other" },
    { registryURL: "https://other.test" },
    { version: "99.0.0" },
    { jws: f.jws + "x" },
  ])
    await assert.rejects(() => verifyRelease({ ...input, ...change }));
});
test("rejects duplicate JSON properties even with a valid signature", async () => {
  const f = await fixture();
  const raw = f.raw.replace("{", '{"name":"other",');
  f.payload.manifest_sha256 = hash(raw);
  f.payload.assets[0] = {
    name: f.m.name + "-universal.json",
    sha256: hash(raw),
    size: Buffer.byteLength(raw),
  };
  const jws = await new CompactSign(Buffer.from(JSON.stringify(f.payload)))
    .setProtectedHeader({ alg: "EdDSA", kid: "trusted" })
    .sign(f.pair.privateKey);
  await assert.rejects(() =>
    verifyRelease({
      registryURL: f.payload.registry,
      id: f.m.name,
      version: f.m.version,
      kid: "trusted",
      key: f.jwk,
      jws,
      manifestRaw: raw,
      descriptor: raw,
    }),
  );
});
