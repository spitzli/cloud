import test from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync, verify } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { runPath, checkContainer, signedStorageRelease } from './native-object-storage-fixture.mjs';
import { ROOT, sha256 } from './native-plugin-fixture.mjs';
import { publicEvidence } from './native-object-storage-evidence.mjs';

test('run path is fixed and cannot select retained services or traverse', () => {
  assert.equal(runPath('abcdef123456'), join(ROOT, '.runtime-test/native-object-storage/abcdef123456'));
  for (const run of ['../webdock-dev', '', 'ABCDEF123456', 'abcdef123456/..']) assert.throws(() => runPath(run));
});
test('cleanup custody requires exact container ID, name, label and confirmed intent', () => {
  const item = { id: 'owned-id', name: 'owned-name', status: 'owned' };
  const actual = { Id: 'owned-id', Name: '/owned-name', Config: { Labels: { 'webdock.native-object-storage': 'abcdef123456' } } };
  checkContainer(item, actual, 'abcdef123456');
  for (const changed of [{ ...actual, Id: 'foreign' }, { ...actual, Name: '/foreign' }, { ...actual, Config: { Labels: {} } }]) assert.throws(() => checkContainer(item, changed, 'abcdef123456'));
  assert.throws(() => checkContainer({ ...item, status: 'intent' }, actual, 'abcdef123456'));
});
test('release signs exact descriptor bytes, version, digest and unique universal asset', async () => {
  const template = JSON.parse(await readFile(join(ROOT, 'plugins/object-storage/manifest.json'))), { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const image = 'owned.test/object-storage@sha256:' + 'a'.repeat(64);
  const release = signedStorageRelease(template, '0.2.0', image, privateKey, 'https://owned.test', 'owned-key', { storage: true });
  const [header, payload, signature] = release.jws.split('.');
  assert(verify(null, Buffer.from(header + '.' + payload), publicKey, Buffer.from(signature, 'base64url')));
  const claims = JSON.parse(Buffer.from(payload, 'base64url'));
  assert.equal(claims.manifest_sha256, sha256(release.descriptor));
  assert.deepEqual(claims.assets, [{ name: 'object-storage-universal.json', sha256: sha256(release.manifestRaw), size: Buffer.byteLength(release.manifestRaw) }]);
  assert.equal(JSON.parse(release.manifestRaw).image, image);
  assert(!verify(null, Buffer.from(header + '.' + payload + 'x'), publicKey, Buffer.from(signature, 'base64url')));
  assert.throws(() => signedStorageRelease(template, '0.9.0', image, privateKey, 'https://owned.test', 'owned-key', {}));
  assert.throws(() => signedStorageRelease(template, '0.1.0', image.replaceAll('a', '0'), privateKey, 'https://owned.test', 'owned-key', {}));
});
test('public evidence omits private recipes, origins, environment and signed descriptors', () => {
  const secret = 'PRIVATE_SENTINEL';
  const report = { runID: 'abcdef123456', directory: '/owned', deadline: 1, origins: { core: secret }, environment: secret, recipe: secret, artifacts: [{ version: '0.1.0', image: secret, jws: secret, manifestRaw: secret, digest: 'sha256:' + 'a'.repeat(64) }] };
  const result = publicEvidence(report, { entries: [{ kind: 'container', name: 'owned', id: 'owned-id', status: 'removed', environment: secret, volumes: [] }] }, null, []);
  assert(!JSON.stringify(result).includes(secret)); assert.equal(result.containers[0].id, 'owned-id');
});
