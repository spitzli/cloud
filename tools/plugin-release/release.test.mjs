import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { finalizeManifest, verifyIntegrity, publishRequest, strictJSON, validateTrust, validatePreflight } from './release.mjs';

process.env.PLUGIN_VALIDATOR_MANIFEST = new URL('../plugin-registry/manifest-validator/Cargo.toml', import.meta.url).pathname;
if (process.env.CARGO_TARGET_DIR) process.env.CARGO_TARGET_DIR = resolve(process.env.CARGO_TARGET_DIR);
const fixture = JSON.parse(await readFile(new URL('../../plugins/echo-demo/manifest.json', import.meta.url), 'utf8'));
const digest = '1234567890abcdef'.repeat(4);
const image = 'ghcr.io/spitzli/echo-demo@sha256:' + digest;
const pack = () => finalizeManifest(fixture, { image, tag: 'v' + fixture.version, sourceSHA: 'b'.repeat(40) });
const { publicKey, privateKey } = generateKeyPairSync('ed25519');
const trust = { registry: 'https://registry.webdock.dev', key: { ...publicKey.export({ format: 'jwk' }), kid: 'test-key' } };
function integrity(release = pack(), alter = (x) => x, key = privateKey) {
  const payload = alter({ v: 1, kid: trust.key.kid, registry: trust.registry, plugin_slug: release.manifest.name,
    release_version: release.manifest.version, manifest_sha256: release.lock.manifestSha256,
    assets: [release.lock.manifestFile, release.lock.descriptorFile].map(name => ({ name, sha256: release.lock.manifestSha256, size: Buffer.byteLength(release.raw) })) });
  const encoded = [ { alg: 'EdDSA', kid: trust.key.kid }, payload ].map(x => Buffer.from(JSON.stringify(x)).toString('base64url')).join('.');
  return { manifest_raw: release.raw, jws: encoded + '.' + sign(null, Buffer.from(encoded), key).toString('base64url') };
}
test('final bytes carry actual digest, exact descriptor and lock hashes', () => {
  const r = pack();
  assert.equal(r.manifest.image, image);
  assert.equal(JSON.parse(r.raw).image, image);
  assert.equal(r.lock.descriptorSha256, r.lock.manifestSha256);
  assert.equal(r.lock.descriptorFile, fixture.name + '-universal.json');
  assert.equal(r.lock.sourceSHA, 'b'.repeat(40));
});
test('finalization rejects placeholders, mutable tags, mismatched versions, invalid native semantics and legacy payloads', () => {
  for (const bad of ['ghcr.io/spitzli/notes:latest', 'ghcr.io/spitzli/notes@sha256:' + '0'.repeat(64), 'registry.example.com/notes@sha256:' + digest, 'ghcr.io/spitzli/notes@sha256:' + 'a'.repeat(64)])
    assert.throws(() => finalizeManifest(fixture, { image: bad, tag: 'v' + fixture.version, sourceSHA: 'b'.repeat(40) }));
  assert.throws(() => finalizeManifest(fixture, { image, tag: 'v9.0.0', sourceSHA: 'b'.repeat(40) }));
  assert.throws(() => finalizeManifest({ ...fixture, protocolVersion: 999 }, { image, tag: 'v' + fixture.version, sourceSHA: 'b'.repeat(40) }));
  assert.throws(() => finalizeManifest({ name: 'legacy', kind: 'plugin', version: '1.0.0' }, { image, tag: 'v1.0.0', sourceSHA: 'b'.repeat(40) }));
});
test('publisher body uses native raw manifest and exact immutable release URLs', () => {
  const r = pack(), body = publishRequest(r, 'spitzli/webdock-extension-echo-demo');
  assert.equal(body.manifest, r.raw);
  assert.equal(body.assets.length, 2);
  assert.equal(body.assets[1].url, 'https://github.com/spitzli/webdock-extension-echo-demo/releases/download/v' + fixture.version + '/' + fixture.name + '-universal.json');
  assert.equal(body.manifestSource, undefined);
  assert.throws(() => publishRequest(r, 'owner/repo/evil'));
});
test('operator-pinned key verifies genuine signed registry bytes', () => {
  const r = pack();
  assert.equal(verifyIntegrity(integrity(r), r, trust).verified, true);
});
test('image preflight requires exact tag, source and registry trust without a publisher token', () => {
  const input = { manifest: fixture, tag: 'v' + fixture.version, sourceSHA: 'b'.repeat(40), trust };
  assert.doesNotThrow(() => validatePreflight(input));
  assert.throws(() => validatePreflight({ ...input, tag: 'main' }));
  assert.throws(() => validatePreflight({ ...input, sourceSHA: 'main' }));
  assert.throws(() => validatePreflight({ ...input, trust: { ...trust, registry: 'http://registry.webdock.dev' } }));
});
test('unsigned, wrong signer, registry, identity, bytes and incomplete assets fail closed', () => {
  const r = pack();
  assert.throws(() => verifyIntegrity({ manifest_raw: r.raw }, r, trust));
  assert.throws(() => verifyIntegrity(integrity(r, x => x, generateKeyPairSync('ed25519').privateKey), r, trust));
  for (const alter of [p => ({ ...p, registry: 'https://attacker.invalid' }), p => ({ ...p, plugin_slug: 'other' }), p => ({ ...p, manifest_sha256: '0'.repeat(64) }), p => ({ ...p, assets: p.assets.slice(1) }), p => ({ ...p, assets: p.assets.map(a => ({ ...a, size: a.size + 1 })) }), p => ({ ...p, assets: [...p.assets, p.assets[0]] })])
    assert.throws(() => verifyIntegrity(integrity(r, alter), r, trust));
  assert.throws(() => verifyIntegrity({ ...integrity(r), manifest_raw: r.raw + ' ' }, r, trust));
  assert.throws(() => validateTrust({ registry: 'http://registry.webdock.dev', key: trust.key }));
  assert.throws(() => validateTrust({ ...trust, key: { ...trust.key, d: 'private' } }));
});
test('signed JSON duplicate keys and malformed JSON are rejected', () => {
  assert.throws(() => strictJSON('{"name":"safe","name":"evil"}'));
  assert.throws(() => strictJSON('{"a":{"x":1,"x":2}}'));
  assert.deepEqual(strictJSON('{"a":{"x":1},"b":{"x":2}}'), { a: { x: 1 }, b: { x: 2 } });
});
test('Docker preflight requires immutable external base images and allows local stages', async () => {
  const { validateDockerfile } = await import('./release.mjs');
  assert.equal(validateDockerfile('FROM oven/bun:1.4.3@sha256:' + digest + ' AS build\nFROM build AS test\nFROM scratch\n'), true);
  for (const text of ['FROM node:latest', 'FROM oven/bun:1.4.3', 'FROM $BASE', 'FROM alpine@sha256:' + 'a'.repeat(64), 'RUN echo no-base'])
    assert.throws(() => validateDockerfile(text));
});
// Several real CLI/Cargo invocations share this bounded integration budget.
test('CLI finalizes exact assets and refuses zero digest without publishing or leaking secrets', { timeout: 30000 }, async () => {
  const { mkdtemp, mkdir, writeFile, rm } = await import('node:fs/promises');
  const { tmpdir } = await import('node:os');
  const { join } = await import('node:path');
  const { spawnSync } = await import('node:child_process');
  const root = await mkdtemp(join(tmpdir(), 'native-release-test-'));
  const cli = new URL('./release.mjs', import.meta.url).pathname;
  const run = (args, extra = {}) => spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: 'utf8', timeout: 10000, env: { ...process.env,
    IMAGE_REPOSITORY: image.split('@')[0], IMAGE_DIGEST: 'sha256:' + digest,
    GITHUB_REF_NAME: 'v' + fixture.version, GITHUB_SHA: 'b'.repeat(40), ...extra } });
  try {
    await writeFile(join(root, 'manifest.json'), JSON.stringify(fixture));
    await mkdir(join(root, 'src'));
    await writeFile(join(root, 'src/main.rs'), 'fn main() {}\n');
    await writeFile(join(root, 'Cargo.toml'), '[package]\nname="release-fixture"\nversion="' + fixture.version + '"\nedition="2024"\n[workspace]\n');
    await writeFile(join(root, 'Dockerfile'), 'FROM oven/bun:1.4.3@sha256:' + digest + '\n');
    assert.equal(spawnSync('cargo', ['generate-lockfile', '--offline'], { cwd: root, timeout: 10000 }).status, 0);
    const authoring = run(['validate']);
    assert.equal(authoring.status, 0, authoring.stderr);
    const preflight = run(['preflight'], { PLUGIN_REGISTRY_URL: trust.registry,
      PLUGIN_REGISTRY_PUBLIC_JWK: JSON.stringify(trust.key), TABULARIUM_TOKEN: '' });
    assert.equal(preflight.status, 0, preflight.stderr);
    const result = run(['finalize']);
    assert.equal(result.status, 0, result.stderr);
    const raw = await readFile(join(root, 'release/tabularium.json'), 'utf8');
    assert.equal(raw, await readFile(join(root, 'release/' + fixture.name + '-universal.json'), 'utf8'));
    assert.equal(JSON.parse(raw).image, image);
    const rejected = run(['finalize', 'bad-release'], { IMAGE_DIGEST: 'sha256:' + '0'.repeat(64) });
    assert.equal(rejected.status, 1);
    await assert.rejects(readFile(join(root, 'bad-release/tabularium.json')));
    const secret = 'tpub_dont_print_me';
    const unconfigured = run(['publish'], { TABULARIUM_TOKEN: secret, PLUGIN_REGISTRY_URL: 'http://localhost', PLUGIN_REGISTRY_PUBLIC_JWK: 'null' });
    assert.equal(unconfigured.status, 1);
    assert.equal((unconfigured.stdout + unconfigured.stderr).includes(secret), false);
    await assert.rejects(readFile(join(root, 'release/signed-receipt.json')));
    const wrongSource = run(['verify'], { GITHUB_SHA: 'c'.repeat(40),
      PLUGIN_REGISTRY_URL: trust.registry, PLUGIN_REGISTRY_PUBLIC_JWK: JSON.stringify(trust.key) });
    assert.equal(wrongSource.status, 1);
    await assert.rejects(readFile(join(root, 'release/signed-receipt.json')));
    await writeFile(join(root, 'release/' + fixture.name + '-universal.json'), raw + ' ');
    const tampered = run(['verify'], { PLUGIN_REGISTRY_URL: trust.registry, PLUGIN_REGISTRY_PUBLIC_JWK: JSON.stringify(trust.key) });
    assert.equal(tampered.status, 1);
    await assert.rejects(readFile(join(root, 'release/signed-receipt.json')));
  } finally { await rm(root, { recursive: true, force: true }); }
});
test('standalone dependencies reject parent paths, mutable Git branches and mixed SDK revisions', async () => {
  const { validateSDKDependencies } = await import('./release.mjs');
  const sdk = { name: 'spitzli-plugin-protocol', source: 'git+https://github.com/spitzli/cloud-plugin-sdk?rev=' + 'b'.repeat(40) };
  assert.doesNotThrow(() => validateSDKDependencies([sdk]));
  for (const source of [null, 'git+https://github.com/spitzli/cloud-plugin-sdk?branch=main'])
    assert.throws(() => validateSDKDependencies([{ ...sdk, source }]));
  assert.throws(() => validateSDKDependencies([sdk, { ...sdk, name: 'spitzli-plugin-runtime', source: sdk.source.replace('b'.repeat(40), 'c'.repeat(40)) }]));
});


test('canonical and legacy SDK namespaces cannot bypass revision validation', async () => {
  const { validateSDKDependencies } = await import('./release.mjs');
  const source = 'git+https://github.com/spitzli/cloud-plugin-sdk?rev=' + 'b'.repeat(40);
  for (const name of ['spitzli-plugin-runtime', 'webdock-plugin-runtime']) {
    assert.doesNotThrow(() => validateSDKDependencies([{ name, source }]));
    assert.throws(() => validateSDKDependencies([{ name, source: null }]));
  }
  assert.throws(() => validateSDKDependencies([{ name: 'spitzli-plugin-runtime', source }, { name: 'webdock-plugin-protocol', source: source.replace('b'.repeat(40), 'c'.repeat(40)) }]));
});
