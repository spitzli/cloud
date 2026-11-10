#!/usr/bin/env bun
/** Native Spitzli plugin release assets and operator-pinned Tabularium verification. */
import { createHash, createPublicKey, verify } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const hash = value => createHash('sha256').update(value).digest('hex');
const requireValue = (condition, message) => { if (!condition) throw Error(message); };
const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;

function parsePluginManifest(input, authoring = false) {
  const manifestPath = process.env.PLUGIN_VALIDATOR_MANIFEST ?? 'tools/manifest-validator/Cargo.toml';
  const args = ['run', '--locked', '--quiet', '--jobs', '3', '--manifest-path', manifestPath];
  if (authoring) args.push('--', '--authoring');
  const env = { ...process.env };
  if (!env.CARGO_TARGET_DIR && env.RUNNER_TOOL_CACHE && existsSync('standalone.json')) {
    const revision = strictJSON(readFileSync('standalone.json', 'utf8')).sdkRevision;
    requireValue(/^[a-f0-9]{40}$/.test(revision), 'Pinned SDK revision required for trusted runner cache');
    env.CARGO_TARGET_DIR = resolve(env.RUNNER_TOOL_CACHE, 'spitzli-native-validator-' + revision);
  }
  const result = spawnSync('cargo', args, { input: JSON.stringify(input), encoding: 'utf8', maxBuffer: 1024 * 1024, env });
  if (result.status !== 0) {
    const logRoot = env.RUNNER_TOOL_CACHE ? resolve(env.RUNNER_TOOL_CACHE, 'spitzli-private-logs') : env.RUNNER_TEMP ?? tmpdir();
    mkdirSync(logRoot, { recursive: true, mode: 0o700 });
    const directory = mkdtempSync(resolve(logRoot, 'native-validator-'));
    let diagnostics = String(result.stderr ?? '') + '\n' + String(result.error?.message ?? '');
    for (const name of ['TABULARIUM_TOKEN', 'GH_TOKEN', 'GITHUB_TOKEN'])
      if (env[name]) diagnostics = diagnostics.replaceAll(env[name], '[REDACTED]');
    const log = resolve(directory, 'cargo.log');
    writeFileSync(log, diagnostics, { mode: 0o600 });
    console.error('Private native validator diagnostics: ' + JSON.stringify(log));
  }
  requireValue(result.status === 0, 'Authoritative native Rust Manifest::parse validation failed');
  return input;
}

export function strictJSON(raw) {
  const result = JSON.parse(raw), stack = [];
  for (const token of raw.match(/"(?:\\.|[^"\\])*"|[{}\[\],:]/g) ?? []) {
    if (token === '{') stack.push({ object: true, keys: new Set(), key: true });
    else if (token === '[') stack.push({ object: false });
    else if (token === '}' || token === ']') stack.pop();
    else if (token === ',') { if (stack.at(-1)?.object) stack.at(-1).key = true; }
    else if (token.startsWith('"') && stack.at(-1)?.key) {
      const key = JSON.parse(token), context = stack.at(-1);
      requireValue(!context.keys.has(key), 'Duplicate JSON key');
      context.keys.add(key); context.key = false;
    }
  }
  return result;
}

function immutableImage(image) {
  requireValue(typeof image === 'string' && /^ghcr\.io\/[a-z0-9._-]+\/[a-z0-9._/-]+@sha256:[a-f0-9]{64}$/.test(image), 'Real immutable GHCR image required');
  requireValue(!/([a-f0-9])\1{63}$/.test(image) && !/(?:example|placeholder|__|\.invalid)/i.test(image), 'Placeholder image forbidden');
}

export function validateDockerfile(raw) {
  const stages = new Set(); let count = 0;
  for (const line of raw.split('\n').filter(line => /^\s*FROM\s/i.test(line))) {
    const match = line.match(/^\s*FROM\s+(?:--platform=\S+\s+)?(\S+)(?:\s+AS\s+([A-Za-z0-9_-]+))?\s*$/i);
    requireValue(match, 'Unsupported Docker FROM syntax');
    const [, image, stage] = match;
    requireValue(image === 'scratch' || stages.has(image.toLowerCase())
      || (/^[a-zA-Z0-9./:_-]+@sha256:[a-f0-9]{64}$/.test(image)
        && !/([a-f0-9])\1{63}$/.test(image) && !/:latest@/.test(image)), 'External Docker base must have an immutable real digest');
    if (stage) stages.add(stage.toLowerCase());
    count++;
  }
  requireValue(count > 0, 'Dockerfile must contain FROM');
  return true;
}

export function validateSDKDependencies(dependencies) {
  const sources = new Set();
  for (const dependency of dependencies.filter(dep => dep.name.startsWith('spitzli-') || dep.name.startsWith('webdock-'))) {
    requireValue(typeof dependency.source === 'string' && /\?rev=[a-f0-9]{40}(?:#|$)/.test(dependency.source), 'Every SDK crate must use a reviewed Git revision');
    sources.add(dependency.source.split('#')[0]);
  }
  requireValue(sources.size <= 1, 'SDK crates must use the same reviewed repository and revision');
}

export function finalizeManifest(input, { image, tag, sourceSHA }) {
  immutableImage(image);
  requireValue(typeof sourceSHA === 'string' && /^[a-f0-9]{40}$/.test(sourceSHA), 'Exact source SHA required');
  const manifest = parsePluginManifest({ ...input, image });
  requireValue(tag === 'v' + manifest.version, 'Release tag must equal v + manifest version');
  const raw = JSON.stringify(canonical(manifest)) + '\n';
  requireValue(Buffer.byteLength(raw) <= 64 * 1024, 'Registry manifest exceeds 64 KiB');
  return { manifest, raw, lock: { name: manifest.name, version: manifest.version, image, sourceSHA,
    manifestFile: 'tabularium.json', descriptorFile: manifest.name + '-universal.json',
    manifestSha256: hash(raw), descriptorSha256: hash(raw), bytes: Buffer.byteLength(raw) } };
}

export function publishRequest(release, repository) {
  requireValue(/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository), 'Exact GitHub owner/repository required');
  const repoUrl = 'https://github.com/' + repository;
  return { manifest: release.raw, version: release.manifest.version, repoUrl,
    assets: [release.lock.manifestFile, release.lock.descriptorFile].map(name => ({ name,
      url: repoUrl + '/releases/download/v' + release.manifest.version + '/' + name,
      sha256: release.lock.manifestSha256, size: Buffer.byteLength(release.raw) })) };
}

export function validateTrust(trust) {
  const url = new URL(trust.registry);
  requireValue(url.protocol === 'https:' && url.origin === trust.registry && !url.username && !url.password && !/(?:example\.|\.invalid$)/.test(url.hostname), 'Exact HTTPS registry origin required');
  const key = trust.key;
  requireValue(key && key.kty === 'OKP' && key.crv === 'Ed25519' && /^[A-Za-z0-9_-]{43}$/.test(key.x ?? '')
    && typeof key.kid === 'string' && key.kid.length > 0 && !('d' in key), 'Pinned public Ed25519 JWK with kid required');
  return createPublicKey({ key, format: 'jwk' });
}

export function validatePreflight({ manifest, tag, sourceSHA, trust }) {
  requireValue(tag === 'v' + manifest.version, 'Tag and authoring manifest version mismatch');
  requireValue(/^[a-f0-9]{40}$/.test(sourceSHA ?? ''), 'Exact source SHA required');
  validateTrust(trust);
}

function decode(segment) {
  requireValue(typeof segment === 'string' && /^[A-Za-z0-9_-]+$/.test(segment), 'Invalid JWS encoding');
  const bytes = Buffer.from(segment, 'base64url');
  requireValue(bytes.toString('base64url') === segment, 'Noncanonical JWS encoding');
  return bytes;
}

export function verifyIntegrity(integrity, release, trust) {
  const key = validateTrust(trust);
  requireValue(typeof integrity.jws === 'string' && integrity.jws.length <= 65536, 'Signed integrity required');
  const parts = integrity.jws.split('.');
  requireValue(parts.length === 3, 'Compact JWS required');
  const header = strictJSON(decode(parts[0]).toString('utf8'));
  requireValue(header.alg === 'EdDSA' && header.kid === trust.key.kid
    && Object.keys(header).every(key => ['alg', 'kid', 'typ'].includes(key)), 'Untrusted JWS header');
  requireValue(verify(null, Buffer.from(parts[0] + '.' + parts[1]), key, decode(parts[2])), 'Registry signature verification failed');
  const payload = strictJSON(decode(parts[1]).toString('utf8'));
  requireValue(payload.v === 1 && payload.kid === trust.key.kid && payload.registry === trust.registry
    && payload.plugin_slug === release.manifest.name && payload.release_version === release.manifest.version, 'Signed release identity mismatch');
  const digest = hash(release.raw), bytes = Buffer.byteLength(release.raw);
  requireValue(integrity.manifest_raw === release.raw && payload.manifest_sha256 === digest, 'Signed manifest bytes mismatch');
  requireValue(Array.isArray(payload.assets) && payload.assets.length <= 32
    && new Set(payload.assets.map(a => a.name)).size === payload.assets.length, 'Invalid signed assets');
  for (const name of [release.lock.manifestFile, release.lock.descriptorFile]) {
    const asset = payload.assets.find(a => a.name === name);
    requireValue(asset?.sha256 === digest && asset?.size === bytes, 'Signed descriptor hash or size mismatch');
  }
  return { verified: true, registry: trust.registry, kid: trust.key.kid, ...release.lock, jws: integrity.jws };
}

async function requestJSON(url, options = {}) {
  const response = await fetch(url, { ...options, redirect: 'error', signal: AbortSignal.timeout(15000) });
  if (!response.ok) {
    await response.body?.cancel();
    const error = Error('Registry HTTP ' + response.status); error.status = response.status; throw error;
  }
  const reader = response.body.getReader(); let length = 0; const chunks = [];
  for (;;) {
    const { done, value } = await reader.read(); if (done) break;
    length += value.byteLength;
    if (length > 1024 * 1024) { await reader.cancel(); throw Error('Registry response exceeds bounds'); }
    chunks.push(Buffer.from(value));
  }
  return strictJSON(Buffer.concat(chunks).toString('utf8'));
}

async function readRelease(directory) {
  const raw = await readFile(resolve(directory, 'tabularium.json'), 'utf8');
  const manifest = parsePluginManifest(strictJSON(raw));
  const lock = strictJSON(await readFile(resolve(directory, 'release-lock.json'), 'utf8'));
  if (process.env.GITHUB_SHA) requireValue(lock.sourceSHA === process.env.GITHUB_SHA, 'Release assets do not belong to this source commit');
  const release = finalizeManifest(manifest, { image: manifest.image, tag: 'v' + manifest.version, sourceSHA: lock.sourceSHA });
  requireValue(raw === release.raw && JSON.stringify(lock) === JSON.stringify(release.lock), 'Final release lock mismatch');
  requireValue(await readFile(resolve(directory, release.lock.descriptorFile), 'utf8') === raw, 'Descriptor bytes differ from manifest');
  return release;
}

async function main() {
  const command = process.argv[2], directory = process.argv[3] ?? 'release';
  if (command === 'validate' || command === 'preflight') {
    const raw = strictJSON(await readFile('manifest.json', 'utf8'));
    const manifest = parsePluginManifest(raw, true);
    const metadata = spawnSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version=1'], { encoding: 'utf8', maxBuffer: 1024 * 1024 });
    requireValue(metadata.status === 0, 'Independent locked Cargo package required');
    const packages = strictJSON(metadata.stdout).packages;
    const pkg = packages.find(pkg => pkg.manifest_path === resolve('Cargo.toml'));
    requireValue(pkg?.version === manifest.version, 'Cargo package and native manifest versions must match');
    validateSDKDependencies(pkg.dependencies);
    validateDockerfile(await readFile('Dockerfile', 'utf8'));
    if (command === 'preflight') {
      validatePreflight({ manifest, tag: process.env.GITHUB_REF_NAME, sourceSHA: process.env.GITHUB_SHA,
        trust: { registry: process.env.PLUGIN_REGISTRY_URL, key: strictJSON(process.env.PLUGIN_REGISTRY_PUBLIC_JWK ?? 'null') } });
    }
    console.log('Native Rust authoring manifest and independent locked package valid');
  } else if (command === 'finalize') {
    const input = strictJSON(await readFile('manifest.json', 'utf8'));
    const release = finalizeManifest(input, { image: process.env.IMAGE_REPOSITORY + '@' + process.env.IMAGE_DIGEST, tag: process.env.GITHUB_REF_NAME, sourceSHA: process.env.GITHUB_SHA });
    await mkdir(directory, { recursive: true });
    for (const name of [release.lock.manifestFile, release.lock.descriptorFile]) await writeFile(resolve(directory, name), release.raw, { flag: 'wx' });
    await writeFile(resolve(directory, 'release-lock.json'), JSON.stringify(release.lock) + '\n', { flag: 'wx' });
    console.log('Final immutable native manifest and descriptor created');
  } else if (command === 'publish' || command === 'verify') {
    const release = await readRelease(directory);
    const trust = { registry: process.env.PLUGIN_REGISTRY_URL, key: strictJSON(process.env.PLUGIN_REGISTRY_PUBLIC_JWK ?? 'null') };
    validateTrust(trust);
    if (command === 'publish') {
      const token = process.env.TABULARIUM_TOKEN;
      requireValue(typeof token === 'string' && /^tpub_[A-Za-z0-9_-]+$/.test(token), 'Scoped publisher token required');
      const result = await requestJSON(trust.registry + '/api/publish/' + release.manifest.name, {
        method: 'POST', headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' },
        body: JSON.stringify(publishRequest(release, process.env.GITHUB_REPOSITORY)) });
      requireValue(result.slug === release.manifest.name && result.version === release.manifest.version, 'Unexpected publication identity');
    }
    const endpoint = trust.registry + '/api/plugins/' + release.manifest.name + '/releases/' + release.manifest.version + '/integrity';
    for (let attempt = 0; attempt < 30; attempt++) {
      let integrity;
      try { integrity = await requestJSON(endpoint); }
      catch (error) { if (![404, 503].includes(error.status)) throw error; }
      if (integrity?.jws) {
        const receipt = verifyIntegrity(integrity, release, trust);
        await writeFile(resolve(directory, 'signed-receipt.json'), JSON.stringify(receipt, null, 2) + '\n');
        console.log('Registry signed exact native release assets with pinned operator key'); return;
      }
      if (attempt < 29) await new Promise(resolve => setTimeout(resolve, 5000));
    }
    throw Error('Registry has not returned signed release assets; hashing or operator approval may still be pending');
  } else throw Error('Usage: release.mjs validate | finalize [release-dir] | publish [release-dir] | verify [release-dir]');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch(() => { console.error('Native plugin release failed. Check configuration, release bytes, registry status and trusted key.'); process.exitCode = 1; });
}
