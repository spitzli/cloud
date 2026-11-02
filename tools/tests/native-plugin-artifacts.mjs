// Mail-only signed OCI fixtures. No top-level execution or ambient credentials.
import assert from 'node:assert/strict';
import { generateKeyPairSync, sign, verify } from 'node:crypto';
import { readFile, mkdir, chmod } from 'node:fs/promises';
import { join } from 'node:path';
import { ROOT, NETWORK, sha256, custodyIntent, custodyConfirm, requireDockerMissing, stageBuildx } from './native-plugin-fixture.mjs';

export function imageIdentity(image) {
  const match = /^([a-zA-Z0-9./:_-]+)@(sha256:[a-f0-9]{64})$/.exec(image);
  assert(match && match[2] !== 'sha256:' + '0'.repeat(64), 'digest_image_required');
  return { repository: match[1], digest: match[2] };
}
export function signedRelease(template, version, image, privateKey, registryURL, kid, recipe) {
  imageIdentity(image);
  assert.equal(template.name, 'mail'); assert(['0.1.0', '0.2.0'].includes(version));
  assert.equal(template.schemaVersion, 1); assert.deepEqual(template.readableSchema, { min: 1, max: 1 });
  const manifestRaw = JSON.stringify({ ...template, version, image }, null, 2) + '\n';
  const header = Buffer.from(JSON.stringify({ alg: 'EdDSA', kid })).toString('base64url');
  const payload = Buffer.from(JSON.stringify({ v: 1, registry: registryURL, plugin_slug: 'mail', release_version: version, kid, manifest_sha256: sha256(manifestRaw), assets: [{ name: 'mail-universal.json', sha256: sha256(manifestRaw), size: Buffer.byteLength(manifestRaw) }] })).toString('base64url');
  const jws = header + '.' + payload + '.' + sign(null, Buffer.from(header + '.' + payload), privateKey).toString('base64url');
  return { id: 'mail', version, registryURL, kid, jws, manifestRaw, descriptor: manifestRaw, recipe, supportedFeatures: ['add-domains', 'byok-turbosmtp', 'create-mailboxes', 'manage-incoming'], targetIDs: ['managed-eu'] };
}
export function verifyRelease(release, publicKey) {
  assert.equal(release.id, 'mail'); assert.equal(release.manifestRaw, release.descriptor);
  const parts = release.jws.split('.'); assert.equal(parts.length, 3);
  const [header, payload, signature] = parts, h = JSON.parse(Buffer.from(header, 'base64url')), p = JSON.parse(Buffer.from(payload, 'base64url'));
  assert.equal(h.alg, 'EdDSA'); assert.equal(h.kid, release.kid);
  assert(verify(null, Buffer.from(header + '.' + payload), publicKey, Buffer.from(signature, 'base64url')), 'release_signature_invalid');
  assert.equal(p.registry, release.registryURL); assert.equal(p.kid, release.kid); assert.equal(p.plugin_slug, 'mail'); assert.equal(p.release_version, release.version);
  assert.equal(p.manifest_sha256, sha256(release.manifestRaw));
  assert.deepEqual(p.assets, [{ name: 'mail-universal.json', sha256: sha256(release.manifestRaw), size: Buffer.byteLength(release.manifestRaw) }]);
  const manifest = JSON.parse(release.manifestRaw); assert.equal(manifest.name, 'mail'); assert.equal(manifest.version, release.version); imageIdentity(manifest.image);
  assert.equal(manifest.schemaVersion, 1); assert.deepEqual(manifest.readableSchema, { min: 1, max: 1 });
}
export async function ownImage(f, name, build) {
  const before = await f.docker(['image', 'inspect', '--format', '{{json .Id}}', name], { allowFailure: true });
  requireDockerMissing(before, 'image', name);
  const item = await custodyIntent(f.custody, f.saveCustody, 'docker-image', name, { absentBeforeCreate: true });
  await build(); const id = JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', name])).output);
  assert(/^sha256:[a-f0-9]{64}$/.test(id), 'owned_image_identity_unavailable');
  await custodyConfirm(item, f.saveCustody, { id }); return { Id: id };
}
export async function buildArtifacts(f) {
  await stageBuildx(f);
  const baseImage = 'wd-mail-reviewed:' + f.input.runID;
  await ownImage(f, baseImage, () => f.docker(['build', '--file', 'plugins/mail/Dockerfile', '--tag', baseImage, '.'], { timeout: 1800000 }));
  const registry = await f.ownContainer('wd-mail-registry-' + f.input.runID, ['--network', NETWORK, '--publish', '127.0.0.1::5000', f.input.images.registry]);
  const host = '127.0.0.1:' + registry.NetworkSettings.Ports['5000/tcp'][0].HostPort;
  const request = async (path, accept) => {
    const response = await fetch(`http://${host}${path}`, { headers: accept ? { accept } : {}, redirect: 'error', signal: AbortSignal.timeout(30000) });
    assert(response.ok, 'owned_registry_read_failed'); return response;
  };
  await f.wait(async () => { try { await request('/v2/'); return true; } catch { return false; } });
  const template = JSON.parse(await readFile(join(ROOT, 'plugins/mail/manifest.json'), 'utf8'));
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  await f.write('release-signing.key', privateKey.export({ type: 'pkcs8', format: 'pem' }));
  const jwk = publicKey.export({ format: 'jwk' }), registryURL = `https://owned-mail-${f.input.runID}.invalid`, kid = 'owned-mail-' + f.input.runID;
  const key = { registryURL, kid, key: { kty: jwk.kty, crv: jwk.crv, x: jwk.x } };
  const releases = [], proof = [], context = f.path('release-context'); await mkdir(context, { mode: 0o700 });
  for (const version of ['0.1.0', '0.2.0']) {
    const tag = `${host}/webdock/mail:${f.input.runID}-${version}`;
    await ownImage(f, tag, () => f.docker(['build', '--file', '-', '--tag', tag, context], { input: `FROM ${baseImage}\nLABEL org.opencontainers.image.version="${version}"\nLABEL webdock.native-mail="${f.input.runID}"\n` }));
    // No product process is run: hash the actual executable in the reviewed artifact.
    const hasher = await f.ownContainer(`wd-mail-hash-${version}-${f.input.runID}`, ['--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--entrypoint', '/usr/bin/sha256sum', tag, '/usr/local/bin/webdock-mail']);
    await f.wait(async () => !(await f.inspect(hasher.Id)).State.Running, 10000);
    assert.equal((await f.inspect(hasher.Id)).State.ExitCode, 0);
    const hashOutput = (await f.docker(['logs', hasher.Id])).output;
    assert(/^[a-f0-9]{64}\s+\/usr\/local\/bin\/webdock-mail$/.test(hashOutput), 'artifact_binary_hash_unavailable');
    await f.docker(['push', tag]);
    const media = 'application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json';
    let response = await request(`/v2/webdock/mail/manifests/${f.input.runID}-${version}`, media), bytes = Buffer.from(await response.arrayBuffer()), digest = 'sha256:' + sha256(bytes), manifest = JSON.parse(bytes), indexDigest = null;
    assert.equal(response.headers.get('docker-content-digest'), digest);
    if (manifest.manifests) {
      indexDigest = digest; const children = manifest.manifests.filter(m => m.platform?.os === 'linux' && m.platform.architecture === 'amd64'); assert.equal(children.length, 1, 'single_amd64_child_required');
      const child = children[0]; response = await request('/v2/webdock/mail/manifests/' + child.digest, child.mediaType); bytes = Buffer.from(await response.arrayBuffer()); digest = 'sha256:' + sha256(bytes); assert.equal(digest, child.digest); assert.equal(bytes.length, child.size); manifest = JSON.parse(bytes);
    }
    assert(manifest.config?.digest && Array.isArray(manifest.layers), 'runnable_manifest_required');
    const image = `${host}/webdock/mail@${digest}`; imageIdentity(image);
    const directory = f.path('oci-' + version); await mkdir(join(directory, 'blobs/sha256'), { recursive: true, mode: 0o700 });
    let config;
    for (const descriptor of [manifest.config, ...manifest.layers]) {
      assert(/^sha256:[a-f0-9]{64}$/.test(descriptor.digest));
      const blob = Buffer.from(await (await request('/v2/webdock/mail/blobs/' + descriptor.digest)).arrayBuffer());
      assert.equal('sha256:' + sha256(blob), descriptor.digest); assert.equal(blob.length, descriptor.size);
      await f.write(`oci-${version}/blobs/sha256/${descriptor.digest.slice(7)}`, blob);
      if (descriptor === manifest.config) config = JSON.parse(blob);
    }
    assert.equal(config.config.Labels['org.opencontainers.image.version'], version);
    await f.write(`oci-${version}/blobs/sha256/${digest.slice(7)}`, bytes);
    await f.write(`oci-${version}/oci-layout`, { imageLayoutVersion: '1.0.0' });
    await f.write(`oci-${version}/index.json`, { schemaVersion: 2, manifests: [{ mediaType: manifest.mediaType ?? 'application/vnd.docker.distribution.manifest.v2+json', digest, size: bytes.length, annotations: { 'org.opencontainers.image.ref.name': image } }] });
    const archive = await f.write('mail-' + version + '.oci.tar', Buffer.alloc(0)); await f.command('tar', ['-C', directory, '-cf', archive, '.']); await chmod(archive, 0o600);
    const release = signedRelease(template, version, image, privateKey, registryURL, kid, {}); verifyRelease(release, publicKey); releases.push(release);
    proof.push({ version, image, digest, indexDigest, binarySha256: hashOutput.split(/\s+/)[0], configDigest: manifest.config.digest, layerDigests: manifest.layers.map(l => l.digest), archive, archiveSha256: sha256(await readFile(archive)), manifestSha256: sha256(release.manifestRaw) });
  }
  assert.notEqual(proof[0].digest, proof[1].digest); assert.notEqual(proof[0].configDigest, proof[1].configDigest);
  assert.equal(proof[0].binarySha256, proof[1].binarySha256); assert.deepEqual(proof[0].layerDigests, proof[1].layerDigests);
  await f.write('artifacts.json', { version: 1, runID: f.input.runID, baseImage, key, releases, proof });
  f.report.artifacts = proof.map(({ archive, archiveSha256, ...value }) => ({ ...value, archive, archiveSha256 }));
  f.report.paths.artifacts = f.path('artifacts.json');
}
