// Explicit disposable native acceptance. Importing this module performs no work.
import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync, sign, X509Certificate } from 'node:crypto';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { chmod, mkdir, open, readFile, readdir, realpath, lstat, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Fixture, ROOT, sha256, random, readPrivate, writePrivate, writePinnedPrivate, processIdentity, settleProcessIdentity, ownEnvironment, privateIP, stageBuildx, delay } from './native-plugin-fixture.mjs';

const RELEASE_VERSIONS = ['0.5.0', '0.6.0'];
const LABEL = 'webdock.native-object-storage';
const BASE = join(ROOT, '.runtime-test/native-object-storage');
const IMAGES = {
  k3s: 'rancher/k3s@sha256:057cfd5a9d51b89f1f0ee139c28d5d5fdd5aa7ddeac2742a0ba90c6dbc0329bb',
  postgres: 'postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911',
  registry: 'registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8',
};
export function runPath(runID) {
  assert(/^[a-f0-9]{12}$/.test(runID), 'owned_run_id_required');
  return join(BASE, runID);
}
export function checkContainer(item, actual, runID) {
  assert(item.status === 'owned' && item.id === actual.Id && actual.Name === '/' + item.name && actual.Config.Labels[LABEL] === runID, 'container_custody_changed');
}
export function signedStorageRelease(template, version, image, privateKey, registryURL, kid, recipe) {
  assert.equal(template.name, 'object-storage');
  assert(['0.1.0', '0.2.0', '0.3.0', '0.4.0', ...RELEASE_VERSIONS].includes(version));
  assert(/^[a-z0-9.:/-]+@sha256:[a-f0-9]{64}$/.test(image) && !image.endsWith('0'.repeat(64)));
  const manifestRaw = JSON.stringify({ ...template, version, image }, null, 2) + '\n';
  const header = Buffer.from(JSON.stringify({ alg: 'EdDSA', kid })).toString('base64url');
  const payload = Buffer.from(JSON.stringify({ v: 1, registry: registryURL, plugin_slug: 'object-storage', release_version: version, kid, manifest_sha256: sha256(manifestRaw), assets: [{ name: 'object-storage-universal.json', sha256: sha256(manifestRaw), size: Buffer.byteLength(manifestRaw) }] })).toString('base64url');
  return { id: 'object-storage', version, registryURL, kid, manifestRaw, descriptor: manifestRaw, jws: `${header}.${payload}.${sign(null, Buffer.from(header + '.' + payload), privateKey).toString('base64url')}`, recipe, supportedFeatures: ['manage-buckets'], targetIDs: ['managed-eu'] };
}
class StorageFixture extends Fixture {
  binary(name) { return this.report.binaries?.[name]?.path ?? join(ROOT, 'target/debug', name); }
  async inspect(name) {
    const format = '{"Id":{{json .Id}},"Name":{{json .Name}},"State":{{json .State}},"Config":{"Labels":{{json .Config.Labels}}},"Mounts":{{json .Mounts}},"NetworkSettings":{"Networks":{{json .NetworkSettings.Networks}},"Ports":{{json .NetworkSettings.Ports}}}}';
    return JSON.parse((await this.docker(['inspect', '--format', format, name])).output);
  }
  async ownContainer(name, args, env = {}, start = true) {
    assert(!(await this.docker(['ps', '-a', '--filter', 'name=^/' + name + '$', '--format', '{{.ID}}'])).output, 'container_preexists');
    const item = { kind: 'container', name, status: 'intent' }; this.custody.entries.push(item); await this.saveCustody();
    const result = await this.docker(['create', '--name', name, '--label', `${LABEL}=${this.input.runID}`, ...args], { env });
    item.id = result.output;
    assert(/^[a-f0-9]{64}$/.test(item.id));
    const actual = await this.inspect(item.id);
    item.volumes = actual.Mounts.filter(m => m.Type === 'volume').map(m => m.Name);
    item.status = 'owned'; await this.saveCustody(); checkContainer(item, actual, this.input.runID);
    if (start) await this.docker(['start', item.id]);
    return this.inspect(item.id);
  }
  async startProcess(name, program, args, env = {}) {
    const executable = await realpath(program), executableSha256 = sha256(await readFile(executable));
    const item = { kind: 'process', name, executable, executableSha256, status: 'intent' };
    this.custody.entries.push(item); await this.saveCustody();
    item.logPrefix = name + '-' + Date.now(); await this.saveCustody();
    const stdout = await open(this.path('logs/' + item.logPrefix + '.stdout'), 'wx', 0o600), stderr = await open(this.path('logs/' + item.logPrefix + '.stderr'), 'wx', 0o600);
    try {
      const child = spawn(executable, args, { cwd: ROOT, env: ownEnvironment(this.directory, env), stdio: ['ignore', stdout.fd, stderr.fd], detached: true });
      await new Promise((done, reject) => { child.once('spawn', done); child.once('error', reject); });
      item.pid = child.pid; item.startTime = processIdentity(child.pid).startTime; await this.saveCustody();
      const identity = await settleProcessIdentity(child.pid, { startTime: item.startTime, executable, executableSha256, argv: [executable, ...args], uid: process.getuid() });
      Object.assign(item, identity, { status: 'owned' }); await this.saveCustody(); child.unref(); return item;
    } finally { await stdout.close(); await stderr.close(); }
  }
  async kube(args, options) { return this.command('/usr/bin/kubectl', ['--kubeconfig', this.path('kubeconfig.yaml'), '--context', 'default', '--request-timeout=15s', ...args], options); }
}
export async function load(runID, create = false) {
  const directory = runPath(runID);
  const parent = join(ROOT, '.runtime-test');
  if (create) { try { await mkdir(parent, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; } }
  const parentInfo = await lstat(parent);
  assert(parentInfo.isDirectory() && !parentInfo.isSymbolicLink() && parentInfo.uid === process.getuid() && !(parentInfo.mode & 0o022) && await realpath(parent) === parent, 'canonical_fixture_parent_required');
  if (create) {
    await mkdir(BASE, { recursive: true, mode: 0o700 });
    const info = await lstat(BASE); assert(info.isDirectory() && !info.isSymbolicLink() && info.uid === process.getuid() && !(info.mode & 0o077) && await realpath(BASE) === BASE, 'private_fixture_base_required');
    await mkdir(directory, { mode: 0o700 });
    for (const name of ['logs', 'home', 'tmp']) await mkdir(join(directory, name), { mode: 0o700 });
    await writePrivate(join(directory, 'report.json'), { runID, directory, fixture: true, deadline: Date.now() + 4 * 3600000 });
    await writePrivate(join(directory, 'custody.json'), { runID, entries: [] });
  }
  const info = await lstat(directory); assert(info.isDirectory() && !info.isSymbolicLink() && info.uid === process.getuid() && !(info.mode & 0o077) && await realpath(directory) === directory, 'canonical_private_run_required');
  const report = JSON.parse(await readPrivate(join(directory, 'report.json'))), custody = JSON.parse(await readPrivate(join(directory, 'custody.json')));
  assert(report.runID === runID && report.directory === directory && custody.runID === runID);
  return new StorageFixture({ directory, runID }, report, custody);
}
async function tls(f, name, sans) {
  const directory = f.path(name); await mkdir(directory, { mode: 0o700 });
  await f.command('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', join(directory, 'ca.key'), '-out', join(directory, 'ca.crt'), '-days', '2', '-subj', '/CN=Owned native object storage fixture', '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign']);
  await f.command('openssl', ['req', '-new', '-newkey', 'rsa:2048', '-nodes', '-keyout', join(directory, 'server.key'), '-out', join(directory, 'server.csr'), '-subj', '/CN=localhost']);
  await f.write(name + '/server.ext', `subjectAltName=${sans}\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n`);
  await f.command('openssl', ['x509', '-req', '-in', join(directory, 'server.csr'), '-CA', join(directory, 'ca.crt'), '-CAkey', join(directory, 'ca.key'), '-CAcreateserial', '-out', join(directory, 'server.crt'), '-days', '2', '-extfile', join(directory, 'server.ext')]);
  for (const file of await readdir(directory)) await chmod(join(directory, file), 0o600);
  return directory;
}
async function startPG(f, name) {
  const directory = await tls(f, name, 'DNS:localhost,IP:127.0.0.1'), password = random();
  const script = 'mkdir -p /tmp/owned-tls; cp /fixture-tls/server.crt /fixture-tls/server.key /tmp/owned-tls/; chown -R postgres:postgres /tmp/owned-tls; chmod 0600 /tmp/owned-tls/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/owned-tls/server.crt -c ssl_key_file=/tmp/owned-tls/server.key';
  let container = await f.ownContainer(`wd-object-${name}-${f.input.runID}`, ['--network', f.report.network.name, '--env', 'POSTGRES_PASSWORD', '--env', 'POSTGRES_DB=webdock_object_fixture', '--publish', '127.0.0.1::5432', '--mount', `type=bind,source=${directory},target=/fixture-tls,readonly`, '--entrypoint', 'sh', IMAGES.postgres, '-ec', script], { POSTGRES_PASSWORD: password });
  const ip = container.NetworkSettings.Networks[f.report.network.name].IPAddress; assert(privateIP(ip));
  await f.write(name + '/server.ext', `subjectAltName=DNS:localhost,IP:127.0.0.1,IP:${ip}\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n`, true);
  await f.command('openssl', ['x509', '-req', '-in', join(directory, 'server.csr'), '-CA', join(directory, 'ca.crt'), '-CAkey', join(directory, 'ca.key'), '-CAcreateserial', '-out', join(directory, 'server.crt'), '-days', '2', '-extfile', join(directory, 'server.ext')]);
  await chmod(join(directory, 'server.crt'), 0o600); await f.docker(['restart', container.Id]); container = await f.inspect(container.Id);
  const port = Number(container.NetworkSettings.Ports['5432/tcp'][0].HostPort), ca = await readPrivate(join(directory, 'ca.crt'));
  const url = new URL(`postgresql://postgres@127.0.0.1:${port}/webdock_object_fixture`); url.password = password;
  const { Client } = await import('pg');
  await f.wait(async () => { const db = new Client({ connectionString: url.href, ssl: { ca }, connectionTimeoutMillis: 1000 }); try { await db.connect(); await db.query('SELECT 1'); return true; } catch { return false; } finally { await db.end().catch(() => {}); } });
  const result = { id: container.Id, ip, port, directory, url: url.href, ca }; await f.write(name + '.json', result); return result;
}
async function chartHash(directory) {
  const hash = createHash('sha256');
  const walk = async (path, prefix = '') => { const files = []; for (const item of await readdir(path, { withFileTypes: true })) { assert(!item.isSymbolicLink()); const name = prefix + item.name; if (item.isDirectory()) files.push(...await walk(join(path, item.name), name + '/')); else if (item.isFile()) files.push(name); } return files; };
  const length = value => { const bytes = Buffer.alloc(8); bytes.writeBigUInt64BE(BigInt(value)); return bytes; };
  for (const name of (await walk(directory)).sort()) { const bytes = await readFile(join(directory, name)); hash.update(length(Buffer.byteLength(name))); hash.update(name); hash.update(length(bytes.length)); hash.update(bytes); } return hash.digest('hex');
}
async function prepare(f) {
  const runID = f.input.runID;
  // Read only the named existing MinIO's identity; do not change its container or network.
  const minio = await f.inspect('webdock-dev-minio-1'); assert(minio.State.Running, 'existing_minio_required');
  f.report.retained = { minioID: minio.Id, containers: (await f.docker(['ps', '-a', '--format', '{{.ID}} {{.Names}}'])).output };
  const network = { kind: 'network', name: 'wd-object-' + runID, status: 'intent' }; f.custody.entries.push(network); await f.saveCustody();
  network.id = (await f.docker(['network', 'create', '--label', `${LABEL}=${runID}`, network.name])).output;
  network.status = 'owned'; await f.saveCustody();
  const details = JSON.parse((await f.docker(['network', 'inspect', network.id])).output)[0];
  assert(details.Labels[LABEL] === runID); const gateway = details.IPAM.Config[0].Gateway; assert(privateIP(gateway));
  f.report.network = { name: network.name, id: network.id, gateway }; await f.saveReport();
  const node = await f.ownContainer('wd-object-k3s-' + runID, ['--network', network.name, '--privileged', '--tmpfs', '/run', '--tmpfs', '/var/run', '--publish', '127.0.0.1::6443', IMAGES.k3s, 'server', '--disable=metrics-server', '--kubelet-arg=image-gc-high-threshold=99', '--kubelet-arg=image-gc-low-threshold=98', '--write-kubeconfig=/tmp/fixture-kubeconfig']);
  f.report.node = { id: node.Id, ip: node.NetworkSettings.Networks[network.name].IPAddress }; await f.saveReport();
  await finishPrepare(f);
}
async function finishPrepare(f) {
  const runID = f.input.runID, gateway = f.report.network.gateway;
  const node = await f.inspect(f.report.node.id);
  checkContainer(f.custody.entries.find(e => e.id === node.Id), node, runID);
  await f.wait(async () => (await f.docker(['exec', node.Id, '/bin/kubectl', 'get', '--raw', '/readyz'], { allowFailure: true, timeout: 5000 })).output === 'ok', 180000);
  const config = (await f.docker(['exec', node.Id, 'cat', '/tmp/fixture-kubeconfig'])).output;
  await f.write('kubeconfig.yaml', config.replace('https://127.0.0.1:6443', 'https://127.0.0.1:' + node.NetworkSettings.Ports['6443/tcp'][0].HostPort));
  await f.wait(async () => (await f.kubeJSON(['get', 'nodes'])).items.length === 1, 120000);
  await f.kube(['wait', '--for=condition=Ready', 'nodes', '--all', '--timeout=120s']);
  await f.wait(async () => (await f.kube(['-n', 'kube-system', 'get', 'deployment', 'traefik', '-o', 'json'], { allowFailure: true })).status === 0, 180000);
  await f.kube(['-n', 'kube-system', 'rollout', 'status', 'deployment/traefik', '--timeout=180s']);
  const domain = runID + '.native-object-storage.test', proxyTLS = await tls(f, 'proxy-tls', 'DNS:' + domain);
  await mkdir(f.path('public-image-context'), { mode: 0o700 }); await f.write('public-image-context/ca.crt', await readPrivate(join(proxyTLS, 'ca.crt')));
  const minio = await f.inspect('webdock-dev-minio-1'); assert.equal(minio.Id, f.report.retained.minioID);
  const upstreamIP = minio.NetworkSettings.Networks['webdock-dev_default'].IPAddress; assert(privateIP(upstreamIP));
  await f.write('proxy.json', { listen: '0.0.0.0', port: 19443, domain, certificate: join(proxyTLS, 'server.crt'), key: join(proxyTLS, 'server.key'), deadline: Math.floor(f.report.deadline / 1000), upstreamIP });
  f.report.proxy = { domain, origin: `https://${domain}:19443`, ip: gateway, ca: await readPrivate(join(proxyTLS, 'ca.crt')) }; await f.saveReport();
  await startPG(f, 'platform'); await startPG(f, 'metadata');
}
async function bootstrap(f) {
  const migrationDirectory = join(ROOT, 'crates/persistence/migrations');
  const schema = Object.fromEntries(await Promise.all((await readdir(migrationDirectory)).filter(name => name.endsWith('.sql')).sort().map(async name => [name, sha256(await readFile(join(migrationDirectory, name)))])));
  f.report.schema = schema; await f.saveReport();
  const platform = await f.json(f.path('platform.json')), metadata = await f.json(f.path('metadata.json'));
  const roles = ['webdock_auth_bootstrap', 'webdock_auth_runtime', 'webdock_worker_runtime', 'webdock_admin_runtime', 'webdock_registry_runtime', 'webdock_runtime', 'webdock_browser_website', 'webdock_cms_archive_runtime', 'webdock_offline_migrate'];
  const passwords = existsSync(f.path('roles.json')) ? await f.json(f.path('roles.json')) : Object.fromEntries(roles.map(name => [name, random()]));
  if (!existsSync(f.path('roles.json'))) await f.write('roles.json', passwords);
  const dbURL = role => { const url = new URL(platform.url); url.username = role; if (role !== 'postgres') url.password = passwords[role]; url.searchParams.set('sslmode', 'verify-full'); url.searchParams.set('sslrootcert', join(platform.directory, 'ca.crt')); return url.href; };
  const ports = { core: 19640, console: 19641, account: 19642, callback: 19643 };
  const origins = { core: `http://127.0.0.1:${ports.core}`, console: `http://127.0.0.1:${ports.console}`, account: `http://localhost:${ports.account}`, studio: `http://localhost:${ports.console}` };
  const env = { NODE_ENV: 'development', PORT: String(ports.core), WEBDOCK_BIND_ADDRESS: '127.0.0.1', WEBDOCK_API_ORIGIN: origins.core, WEBDOCK_AUTH_ORIGIN: origins.account, WEBDOCK_STUDIO_ORIGIN: origins.studio, WEBDOCK_CONSOLE_ORIGIN: origins.console, WEBDOCK_AUTH_SECRET: random(), DATABASE_RLS_SECRET: random(), DATABASE_CA_CERT: platform.ca, WEBDOCK_FEATURES: JSON.stringify({ hosting: 'disabled', databases: 'disabled', mail: 'disabled' }), WEBDOCK_WORKLOADS_ENABLED: 'true' };
  const target = `127.0.0.1:${platform.port}/webdock_object_fixture`, binary = name => f.binary(name);
  await f.command(binary('webdock-migrate'), ['--target', target, '--initialize-empty', '--provision-roles-from', f.path('roles.json')], { env: { ...env, WEBDOCK_MIGRATION_DATABASE_URL: dbURL('postgres') }, timeout: 180000 });
  const offline = { ...env, DATABASE_URL_UNPOOLED: dbURL('webdock_offline_migrate') };
  await f.write('operator-input.json', { email: `object-${f.input.runID}@example.test`, name: 'Owned storage operator', password: 'Owned-' + random() + '!', authOrigin: origins.account, studioOrigin: origins.studio, consoleOrigin: origins.console });
  await f.command(binary('webdock-bootstrap-operator'), ['--target', target, '--config', f.path('operator-input.json'), '--output', f.path('operator-output.json')], { env: offline });
  const platformID = 'object-' + f.input.runID;
  await f.write('enrollment-input.json', { platformID, agentID: 'object-agent', name: 'Owned isolated storage fixture', country: 'de', region: 'eu', locationEvidence: 'Disposable local fixture only; no production location claim', transport: { kind: 'unix', uid: process.getuid() } });
  await f.command(binary('webdock-enroll-agent'), ['--target', target, '--config', f.path('enrollment-input.json'), '--output', f.path('enrollment.json')], { env: offline });
  await f.write('catalog.json', { version: 1, platformID, keys: [], targets: [], releases: [] });
  const callbackHost = 'core-' + f.input.runID + '.test';
  await f.write('kernel-input.json', { platformID, installationCatalog: f.path('catalog.json'), callbackOrigin: `https://${callbackHost}:${ports.callback}`, listenAddress: `0.0.0.0:${ports.callback}` });
  await f.command(binary('webdock-bootstrap-plugins'), ['--target', target, '--config', f.path('kernel-input.json'), '--output', f.path('kernel')], { env: offline });
  const kernel = f.path('kernel'), config = await f.json(join(kernel, 'plugin-config.json'));
  await mkdir(f.path('staging'), { mode: 0o700 }); config.storage = { stagingDirectory: f.path('staging') }; await writePrivate(join(kernel, 'plugin-config.json'), config, true);
  Object.assign(env, (await f.json(f.path('operator-output.json'))).environment, { WEBDOCK_NATIVE_PLUGIN_CONFIG: join(kernel, 'plugin-config.json') });
  for (const [key, role] of [['WEBDOCK_DATABASE_URL', 'webdock_auth_bootstrap'], ['WEBDOCK_SESSION_DATABASE_URL', 'webdock_auth_runtime'], ['WEBDOCK_WORKER_DATABASE_URL', 'webdock_worker_runtime'], ['WEBDOCK_REGISTRY_DATABASE_URL', 'webdock_registry_runtime']]) env[key] = dbURL(role);
  await f.write('core-environment.json', env);
  await f.write('worker.json', { storageTargets: [{ id: 'metadata', adminUrl: metadata.url, authority: join(metadata.directory, 'ca.crt'), runtimeHost: metadata.ip, runtimePort: 5432 }] });
  const socketDirectory = '/tmp/wdos-' + f.input.runID;
  const socketCustody = { kind: 'directory', name: socketDirectory, status: 'intent' }; f.custody.entries.push(socketCustody); await f.saveCustody();
  await mkdir(socketDirectory, { mode: 0o700 }); const socketInfo = await lstat(socketDirectory);
  Object.assign(socketCustody, { inode: socketInfo.ino, device: socketInfo.dev, status: 'owned' }); await f.saveCustody();
  const socket = join(socketDirectory, 'control.sock');
  await f.write('agent-control.json', { kind: 'unix', path: socket, installation: platformID, agent_id: 'object-agent', uid: process.getuid() });
  await mkdir(f.path('agent-state'), { mode: 0o700 }); const chart = join(ROOT, 'deploy/helm/webdock-plugin'), enrollment = await f.json(f.path('enrollment.json'));
  await f.write('agent.json', { version: 1, installation: platformID, agentId: 'object-agent', generation: enrollment.identity.generation, stateDirectory: f.path('agent-state'), control: { kind: 'unix', path: socket }, runtime: { kind: 'kubernetes', kubeconfig: f.path('kubeconfig.yaml'), context: 'default', helm: '/usr/local/bin/helm', chart, chartSha256: await chartHash(chart) }, pluginRuntimeEnabled: true, workloads: true });
  Object.assign(f.report, { platformID, clusterID: enrollment.clusterID, ports, origins, kernel, callbackHost }); await f.saveReport();
  for (const [name, hash] of Object.entries(schema)) assert.equal(sha256(await readFile(join(migrationDirectory, name))), hash, 'migration_changed_during_bootstrap');
}
async function start(f) {
  const env = await f.json(f.path('core-environment.json')), binary = name => f.binary(name);
  const callbackEnv = await f.json(f.path('callback-environment.json'));
  const callback = await f.inspect(f.report.callback.id); checkContainer(f.custody.entries.find(e => e.id === callback.Id), callback, f.input.runID);
  await f.startProcess('callback-exec', '/usr/bin/docker', ['--host', 'unix:///var/run/docker.sock', 'exec', ...Object.keys(callbackEnv).flatMap(key => ['--env', key]), callback.Id, '/bin/sh', '-ec', 'umask 077; mkdir -p /tmp/staging; chmod 700 /tmp/staging; exec /reviewed-plugin-control --config "$1"', 'sh', f.path('callback-plugin-config.json')], callbackEnv);
  for (const [name, program, args, processEnv] of [
    ['core', 'webdock-control-plane', [], env],
    ['agent-control', 'webdock-agent-control', ['--config', f.path('agent-control.json')], env],
    ['controller', 'webdock-plugin-controller', ['--config', join(f.report.kernel, 'plugin-config.json'), '--installation-config', f.path('worker.json')], env],
    ['agent', 'webdock-agent', ['--config', f.path('agent.json'), 'run'], {}],
  ]) await f.startProcess(name, binary(program), args, processEnv);
  await f.wait(async () => { try { return (await fetch(f.report.origins.core + '/readyz', { signal: AbortSignal.timeout(1000) })).ok; } catch { return false; } });
  f.report.coreReadyStatus = 200;
}
async function prepareTransports(f) {
  const suffix = existsSync(f.path('proxy-image')) ? '-r2' : '', folder = 'proxy-image' + suffix;
  const base = f.report.versionImages[RELEASE_VERSIONS[0]], context = f.path(folder); await mkdir(context, { mode: 0o700 });
  await f.write(folder + '/proxy.py', await readFile(join(ROOT, 'tools/tests/native-object-storage-proxy.py')));
  await f.write(folder + '/Dockerfile', `FROM ${base}\nUSER 0\nRUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*\nCOPY --chmod=0444 proxy.py /proxy.py\nUSER ${process.getuid()}:${process.getgid()}\nENTRYPOINT ["/usr/bin/python3", "/proxy.py"]\n`);
  const image = { kind: 'docker-image', name: 'webdock-native-object-proxy:' + f.input.runID + suffix, status: 'intent' }; f.custody.entries.push(image); await f.saveCustody();
  const tool = '/home/newt/.docker/cli-plugins/docker-buildx'; f.input.review = { binaries: { 'docker-buildx': { path: tool, sha256: sha256(await readFile(tool)) } } }; await stageBuildx(f);
  await f.docker(['build', '-t', image.name, context], { timeout: 180000 });
  image.id = JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', image.name])).output); image.status = 'owned'; await f.saveCustody();
  const mounts = [f.path('proxy.json'), f.path('proxy-tls/server.crt'), f.path('proxy-tls/server.key')].flatMap(path => ['--mount', `type=bind,source=${path},target=${path},readonly`]);
  const proxy = await f.ownContainer('wd-object-proxy-' + f.input.runID + suffix, ['--network', f.report.network.name, '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', ...mounts, image.id, f.path('proxy.json')], {}, false);
  await f.docker(['network', 'connect', 'webdock-dev_default', proxy.Id]); await f.docker(['start', proxy.Id]);
  const active = await f.inspect(proxy.Id); f.report.proxy.id = active.Id; f.report.proxy.ip = active.NetworkSettings.Networks[f.report.network.name].IPAddress; assert(privateIP(f.report.proxy.ip));
  const kernel = f.report.kernel, config = await f.json(join(kernel, 'plugin-config.json'));
  config.storage = { stagingDirectory: '/tmp/staging' }; await f.write('callback-plugin-config.json', config);
  const env = await f.json(f.path('core-environment.json')), platform = await f.json(f.path('platform.json'));
  for (const key of ['WEBDOCK_DATABASE_URL', 'WEBDOCK_SESSION_DATABASE_URL', 'WEBDOCK_WORKER_DATABASE_URL', 'WEBDOCK_REGISTRY_DATABASE_URL']) { const url = new URL(env[key]); url.hostname = platform.ip; url.port = '5432'; env[key] = url.href; }
  env.WEBDOCK_NATIVE_PLUGIN_CONFIG = f.path('callback-plugin-config.json'); await f.write('callback-environment.json', env);
  const files = [f.path('callback-plugin-config.json'), f.path('catalog.json'), join(platform.directory, 'ca.crt'), ...['invocation.key', 'host-bindings.json', 'connections.json', 'core.crt', 'core.key', 'ca.crt'].map(name => join(kernel, name))];
  const callback = await f.ownContainer('wd-object-callback-' + f.input.runID, ['--network', f.report.network.name, '--user', `${process.getuid()}:${process.getgid()}`, '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--tmpfs', '/tmp:rw,nosuid,nodev,size=32m,mode=1777', ...files.flatMap(path => ['--mount', `type=bind,source=${path},target=${path},readonly`]), '--mount', `type=bind,source=${f.binary('webdock-plugin-control')},target=/reviewed-plugin-control,readonly`, '--entrypoint', '/bin/sh', base, '-ec', 'exec sleep infinity']);
  await f.docker(['exec', callback.Id, '/reviewed-plugin-control', '--help']);
  f.report.callback = { id: callback.Id, ip: callback.NetworkSettings.Networks[f.report.network.name].IPAddress }; assert(privateIP(f.report.callback.ip)); await f.saveReport();
}
async function startCore(f) {
  const env = await f.json(f.path('core-environment.json'));
  await f.startProcess('setup-core', f.binary('webdock-control-plane'), [], env);
  await f.wait(async () => { try { return (await fetch(f.report.origins.core + '/readyz', { signal: AbortSignal.timeout(1000) })).ok; } catch { return false; } });
  f.report.coreReadyStatus = 200;
}
export async function stopProcess(f, item) {
  if (existsSync(`/proc/${item.pid}`)) {
    assert.deepEqual(processIdentity(item.pid), { startTime: item.startTime, commandSha256: item.commandSha256 }, 'process_custody_changed');
    process.kill(-item.pid, 'SIGTERM');
    await f.wait(() => !existsSync(`/proc/${item.pid}`), 30000);
  }
  item.status = 'removed'; await f.saveCustody();
}
async function finalizeCatalog(f) {
  assert(f.report.project && f.report.foreignProject && f.report.realOperatorSetup?.registryAPI);
  const setupCore = f.custody.entries.find(e => e.name === 'setup-core' && e.status === 'owned');
  if (setupCore) await stopProcess(f, setupCore);
  const kernel = f.report.kernel, artifact = await f.json(f.path('artifacts.json')), metadata = await f.json(f.path('metadata.json'));
  const values = Object.fromEntries((await readPrivate(join(ROOT, '.dev-local/minio.env'))).split('\n').filter(line => line && !line.startsWith('#') && line.includes('=')).map(line => { const at = line.indexOf('='); return [line.slice(0, at), line.slice(at + 1).trim().replace(/^["']|["']$/g, '')]; }));
  assert(values.MINIO_ROOT_USER && values.MINIO_ROOT_PASSWORD);
  const backend = { id: 'owned-native-minio-' + f.input.runID, kind: 'minio-native-fixture', endpoint: f.report.proxy.origin, region: 'us-east-1', account: 'owned-local-fixture', accessKey: values.MINIO_ROOT_USER, secretKey: values.MINIO_ROOT_PASSWORD, minio: { executable: '/usr/local/bin/mc', configDir: '/tmp/webdock-minio' } };
  const recipe = { storage: true, sealingKey: true, privateConfiguration: { backend } };
  const storageBinding = { origin: f.report.proxy.origin, addresses: [f.report.proxy.ip], ca: f.report.proxy.ca, pathPrefix: '/' };
  const storage = Object.fromEntries([f.report.project, f.report.foreignProject].map(project => [project, { 'managed-objects': storageBinding }]));
  const pin = (await f.json(join(kernel, 'bootstrap.json'))).coreCertificateSha256;
  assert.equal(pin, sha256(new X509Certificate(await readPrivate(join(kernel, 'core.crt'))).raw));
  const gateway = f.report.callback.ip, ports = f.report.ports;
  const policy = { storage, runtimeDomain: 'runtime-' + f.input.runID + '.test', runtimePort: 8443, entryPoint: 'websecure', runtimeOriginPort: 443, runtimeIngressHost: f.report.node.ip,
    core: { origin: `https://${f.report.callbackHost}:${ports.callback}`, domain: f.report.callbackHost, certificate: join(kernel, 'core.crt'), privateKey: join(kernel, 'core.key'), authority: join(kernel, 'ca.crt'), serverCertificateSha256: pin },
    runtimeIdentity: { authority: join(kernel, 'ca.crt'), privateKey: join(kernel, 'ca.key') }, storageID: 'metadata', resources: { cpuMillicores: 500, memoryBytes: 536870912 },
    egress: [{ cidr: gateway + '/32', port: ports.callback }, { cidr: f.report.proxy.ip + '/32', port: 19443 }, { cidr: metadata.ip + '/32', port: 5432 }], hostAliases: [{ ip: gateway, hostnames: [f.report.callbackHost] }, { ip: f.report.proxy.ip, hostnames: [f.report.proxy.domain] }] };
  assert(!existsSync(f.path('install-request.json')), 'accepted_catalog_cannot_be_replaced');
  if (!existsSync(f.path('catalog-initial.json'))) await f.write('catalog-initial.json', await readPrivate(f.path('catalog.json')));
  const releases = artifact.releases.map(release => ({ ...release, recipe }));
  await writePinnedPrivate(f.path('catalog.json'), { version: 1, platformID: f.report.platformID, keys: [artifact.key], targets: [{ id: 'managed-eu', label: 'Owned isolated local object storage fixture', clusterID: f.report.clusterID, policy }], releases });
  f.report.catalogSha256 = sha256(await readPrivate(f.path('catalog.json'))); await f.saveReport();
}
async function serveUI(f) {
  const { createServer } = await import('vite'), servers = [];
  for (const [name, host] of [['console', '127.0.0.1'], ['account', 'localhost']]) {
    const server = await createServer({ root: join(ROOT, 'apps/console'), configFile: join(ROOT, 'apps/console/vite.config.ts'), cacheDir: f.path('vite-cache'), envDir: false, server: { host, port: f.report.ports[name], strictPort: true } });
    await server.listen(); servers.push(server);
  }
  const stop = async () => { for (const server of servers) await server.close(); process.exit(0); };
  process.on('SIGTERM', stop); process.on('SIGINT', stop);
}
async function startUI(f) {
  await f.startProcess('console', '/home/newt/.bun/bin/bun', ['--no-env-file', fileURLToPath(import.meta.url), 'serve-ui', f.input.runID], { WEBDOCK_DEV_API_ORIGIN: f.report.origins.core });
  for (const origin of [f.report.origins.console, f.report.origins.account]) await f.wait(async () => { try { return (await fetch(origin + '/console-config.json', { signal: AbortSignal.timeout(1000) })).ok; } catch { return false; } }, 30000);
}
async function artifacts(f) {
  assert(RELEASE_VERSIONS.every(version => f.report.versionImages?.[version]), 'separately_compiled_versions_required');
  assert.equal(sha256(await readFile(join(ROOT, 'plugins/object-storage/manifest.json'))), f.report.imageManifestSha256, 'compiled_manifest_changed');
  const baseImage = f.report.versionImages[RELEASE_VERSIONS[0]];
  const baseID = JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', baseImage])).output);
  assert(/^sha256:[a-f0-9]{64}$/.test(baseID)); f.report.fixtureImage = { name: baseImage, id: baseID };
  const registry = await f.ownContainer('wd-object-registry-' + f.input.runID, ['--network', f.report.network.name, '--publish', '127.0.0.1::5000', IMAGES.registry]);
  const host = '127.0.0.1:' + registry.NetworkSettings.Ports['5000/tcp'][0].HostPort;
  const request = async (path, accept) => { const reply = await fetch(`http://${host}${path}`, { headers: accept ? { accept } : {}, redirect: 'error', signal: AbortSignal.timeout(30000) }); assert(reply.ok, 'owned_registry_read_failed'); return reply; };
  await f.wait(async () => { try { await request('/v2/'); return true; } catch { return false; } });
  const template = JSON.parse(await readFile(join(ROOT, 'plugins/object-storage/manifest.json'), 'utf8'));
  const { privateKey, publicKey } = generateKeyPairSync('ed25519'); await f.write('release-signing.key', privateKey.export({ type: 'pkcs8', format: 'pem' }));
  const jwk = publicKey.export({ format: 'jwk' }), registryURL = `https://owned-object-${f.input.runID}.test`, kid = 'owned-object-' + f.input.runID;
  const key = { registryURL, kid, key: { kty: jwk.kty, crv: jwk.crv, x: jwk.x } }, releases = [], proof = [];
  const context = f.path('release-context'); await mkdir(context, { mode: 0o700 });
  for (const version of RELEASE_VERSIONS) {
    const tag = `${host}/webdock/object-storage:${f.input.runID}-${version}`;
    assert((await f.docker(['image', 'inspect', tag], { allowFailure: true })).status !== 0, 'release_tag_preexists');
    const item = { kind: 'docker-image', name: tag, status: 'intent' }; f.custody.entries.push(item); await f.saveCustody();
    await f.docker(['build', '--file', '-', '--tag', tag, context], { input: `FROM ${f.report.versionImages[version]}\nLABEL org.opencontainers.image.version="${version}"\nLABEL webdock.native-object-storage="${f.input.runID}"\n`, timeout: 120000 });
    item.id = JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', tag])).output); item.status = 'owned'; await f.saveCustody();
    await f.docker(['push', tag]);
    const media = 'application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json';
    let reply = await request(`/v2/webdock/object-storage/manifests/${f.input.runID}-${version}`, media), bytes = Buffer.from(await reply.arrayBuffer()), digest = 'sha256:' + sha256(bytes), manifest = JSON.parse(bytes);
    assert.equal(reply.headers.get('docker-content-digest'), digest);
    if (manifest.manifests) { const children = manifest.manifests.filter(m => m.platform?.os === 'linux' && m.platform.architecture === 'amd64'); assert.equal(children.length, 1); const child = children[0]; bytes = Buffer.from(await (await request('/v2/webdock/object-storage/manifests/' + child.digest, child.mediaType)).arrayBuffer()); digest = 'sha256:' + sha256(bytes); assert.equal(digest, child.digest); manifest = JSON.parse(bytes); }
    const image = `${host}/webdock/object-storage@${digest}`, directory = f.path('oci-' + version); await mkdir(join(directory, 'blobs/sha256'), { recursive: true, mode: 0o700 });
    for (const descriptor of [manifest.config, ...manifest.layers]) {
      const blob = Buffer.from(await (await request('/v2/webdock/object-storage/blobs/' + descriptor.digest)).arrayBuffer()); assert.equal('sha256:' + sha256(blob), descriptor.digest); assert.equal(blob.length, descriptor.size);
      await f.write(`oci-${version}/blobs/sha256/${descriptor.digest.slice(7)}`, blob);
    }
    await f.write(`oci-${version}/blobs/sha256/${digest.slice(7)}`, bytes); await f.write(`oci-${version}/oci-layout`, { imageLayoutVersion: '1.0.0' });
    await f.write(`oci-${version}/index.json`, { schemaVersion: 2, manifests: [{ mediaType: manifest.mediaType, digest, size: bytes.length, annotations: { 'org.opencontainers.image.ref.name': image } }] });
    const archive = await f.write('object-' + version + '.oci.tar', Buffer.alloc(0)); await f.command('tar', ['-C', directory, '-cf', archive, '.']); await chmod(archive, 0o600);
    const nodeArchive = '/tmp/object-' + f.input.runID + '-' + version + '.tar';
    await f.docker(['cp', archive, f.report.node.id + ':' + nodeArchive]);
    await f.docker(['exec', f.report.node.id, '/bin/ctr', '-n', 'k8s.io', 'images', 'import', '--digests', nodeArchive]);
    await f.docker(['exec', f.report.node.id, 'rm', nodeArchive]);
    const cri = JSON.parse((await f.docker(['exec', f.report.node.id, '/bin/crictl', 'inspecti', image])).output); assert(cri.status.repoDigests.includes(image) && cri.status.id === manifest.config.digest);
    releases.push(signedStorageRelease(template, version, image, privateKey, registryURL, kid, {}));
    proof.push({ version, image, digest, configDigest: manifest.config.digest, layerDigests: manifest.layers.map(v => v.digest), manifestSha256: sha256(releases.at(-1).manifestRaw), archiveSha256: sha256(await readFile(archive)) });
  }
  await f.write('artifacts.json', { key, releases, proof }); f.report.artifacts = proof; await f.saveReport();
}
export async function reimportArtifacts(f) {
  const artifact = await f.json(f.path('artifacts.json'));
  for (const release of artifact.proof) {
    const archive = f.path('object-' + release.version + '.oci.tar'); assert.equal(sha256(await readFile(archive)), release.archiveSha256, 'owned_archive_changed');
    const path = '/tmp/object-reimport-' + f.input.runID + '-' + release.version + '.tar';
    await f.docker(['exec', f.report.node.id, '/bin/sh', '-ec', 'test ! -e "$1" && test ! -L "$1"', 'sh', path]);
    const item = { kind: 'node-file', name: path, nodeID: f.report.node.id, archiveSha256: release.archiveSha256, status: 'intent' }; f.custody.entries.push(item); await f.saveCustody();
    await f.docker(['cp', archive, f.report.node.id + ':' + path]);
    assert((await f.docker(['exec', f.report.node.id, 'sha256sum', path])).output.startsWith(release.archiveSha256 + ' ')); item.status = 'owned'; await f.saveCustody();
    await f.docker(['exec', f.report.node.id, '/bin/ctr', '-n', 'k8s.io', 'images', 'import', '--digests', path]); await f.docker(['exec', f.report.node.id, 'rm', path]); item.status = 'removed'; await f.saveCustody();
    const cri = JSON.parse((await f.docker(['exec', f.report.node.id, '/bin/crictl', 'inspecti', release.image])).output); assert(cri.status.repoDigests.includes(release.image) && cri.status.id === release.configDigest);
  }
}
async function buildVersionImages(f) {
  const manifest = await readFile(join(ROOT, 'plugins/object-storage/manifest.json'));
  const tool = '/home/newt/.docker/cli-plugins/docker-buildx';
  f.input.review = { binaries: { 'docker-buildx': { path: tool, sha256: sha256(await readFile(tool)) } } }; await stageBuildx(f);
  const versionImages = {};
  for (const version of RELEASE_VERSIONS) {
    const name = `webdock-native-object-storage:${f.input.runID}-final-${version}`;
    assert((await f.docker(['image', 'inspect', name], { allowFailure: true })).status !== 0);
    const item = { kind: 'docker-image', name, status: 'intent' }; f.custody.entries.push(item); await f.saveCustody();
    await f.docker(['buildx', 'build', '-f', 'plugins/object-storage/fixtures/Dockerfile.native', '--build-arg', 'FIXTURE_VERSION=' + version, '--build-context', 'fixture=' + f.path('public-image-context'), '--tag', name, '--load', '.'], { timeout: 1800000 });
    item.id = JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', name])).output); item.status = 'owned'; await f.saveCustody();
    versionImages[version] = name;
  }
  assert.equal(sha256(await readFile(join(ROOT, 'plugins/object-storage/manifest.json'))), sha256(manifest), 'manifest_changed_during_build');
  f.report.versionImages = versionImages; f.report.imageManifestSha256 = sha256(manifest); await f.saveReport();
}
async function stopServices(f) {
  for (const item of [...f.custody.entries].reverse().filter(e => e.kind === 'process' && ['core', 'plugin-control', 'agent-control', 'controller', 'agent'].includes(e.name) && e.status === 'owned')) await stopProcess(f, item);
}
async function pinBinaries(f) {
  await mkdir(f.path('binaries'), { mode: 0o700 }); const binaries = {};
  for (const name of ['webdock-migrate', 'webdock-bootstrap-operator', 'webdock-enroll-agent', 'webdock-bootstrap-plugins', 'webdock-control-plane', 'webdock-plugin-control', 'webdock-plugin-controller', 'webdock-agent-control', 'webdock-agent']) {
    const bytes = await readFile(join(ROOT, 'target/debug', name)); assert(bytes.subarray(0, 4).equals(Buffer.from([127, 69, 76, 70])));
    const path = await f.write('binaries/' + name, bytes); await chmod(path, 0o500); binaries[name] = { path, sha256: sha256(bytes) };
  }
  f.report.binaries = binaries; await f.saveReport();
}
async function cleanup(f) {
  assert(f.custody.entries.every(e => e.status !== 'intent'), 'uncertain_custody_blocks_cleanup');
  for (const item of [...f.custody.entries].reverse().filter(e => e.kind === 'process' && e.status === 'owned')) {
    if (item.pid === process.pid) { item.status = 'removed'; await f.saveCustody(); }
    else await stopProcess(f, item);
  }
  for (const item of [...f.custody.entries].reverse().filter(e => e.kind === 'container' && e.status === 'owned')) {
    const actual = await f.inspect(item.id); checkContainer(item, actual, f.input.runID);
    assert.deepEqual(actual.Mounts.filter(m => m.Type === 'volume').map(m => m.Name), item.volumes, 'container_volume_custody_changed');
    await f.docker(['rm', '--force', '--volumes', item.id]);
    const absent = await f.docker(['inspect', item.id], { allowFailure: true }); assert(absent.status === 1 && /No such (object|container)/i.test(absent.stderr));
    for (const volume of item.volumes) { const absent = await f.docker(['volume', 'inspect', volume], { allowFailure: true }); assert(absent.status === 1 && /no such volume/i.test(absent.stderr)); }
    item.status = 'removed'; await f.saveCustody();
  }
  for (const item of f.custody.entries.filter(e => e.kind === 'docker-image' && e.status === 'owned')) {
    const actual = JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', item.name])).output); assert.equal(actual, item.id);
    await f.docker(['image', 'rm', item.name]); item.status = 'removed'; await f.saveCustody();
  }
  for (const item of f.custody.entries.filter(e => e.kind === 'network' && e.status === 'owned')) {
    const actual = JSON.parse((await f.docker(['network', 'inspect', item.id])).output)[0]; assert(actual.Id === item.id && actual.Name === item.name && actual.Labels[LABEL] === f.input.runID && Object.keys(actual.Containers).length === 0);
    await f.docker(['network', 'rm', item.id]); item.status = 'removed'; await f.saveCustody();
  }
  for (const item of f.custody.entries.filter(e => e.kind === 'directory' && e.status === 'owned')) {
    assert.equal(item.name, '/tmp/wdos-' + f.input.runID); const info = await lstat(item.name); assert(info.isDirectory() && !info.isSymbolicLink() && info.uid === process.getuid() && !(info.mode & 0o077));
    if (item.inode) assert(info.ino === item.inode && info.dev === item.device);
    await rm(item.name, { recursive: true }); item.status = 'removed'; await f.saveCustody();
  }
  const minio = await f.inspect('webdock-dev-minio-1'); assert(minio.Id === f.report.retained.minioID && minio.State.Running);
  assert(!(await f.docker(['ps', '-a', '--filter', `label=${LABEL}=${f.input.runID}`, '--format', '{{.ID}}'])).output);
  f.report.cleanup = { exactContainerIDsRemoved: true, anonymousVolumesRemoved: true, ownedNetworkRemoved: true, recordedProcessesStopped: true, existingMinioStillRunning: true, privateEvidenceRetained: true }; await f.saveReport();
}
export async function main(args) {
  const [stage, runID] = args, f = await load(runID, stage === 'prepare');
  try {
    if (!['cleanup', 'watch-deadline'].includes(stage)) assert(Date.now() < f.report.deadline, 'fixture_deadline_expired');
    if (stage === 'prepare') await prepare(f);
    else if (stage === 'finish-prepare') await finishPrepare(f);
    else if (stage === 'bootstrap') await bootstrap(f);
    else if (stage === 'artifacts') await artifacts(f);
    else if (stage === 'reimport-artifacts') await reimportArtifacts(f);
    else if (stage === 'build-version-images') await buildVersionImages(f);
    else if (stage === 'prepare-transports') await prepareTransports(f);
    else if (stage === 'pin-binaries') await pinBinaries(f);
    else if (stage === 'stop-services') await stopServices(f);
    else if (stage === 'cleanup') await cleanup(f);
    else if (stage === 'start-watchdog') await f.startProcess('deadline-guardian', process.execPath, [fileURLToPath(import.meta.url), 'watch-deadline', runID]);
    else if (stage === 'watch-deadline') { while (Date.now() < f.report.deadline) await delay(Math.min(30000, f.report.deadline - Date.now())); await cleanup(await load(runID)); return; }
    else if (stage === 'start-core') await startCore(f);
    else if (stage === 'finalize-catalog') await finalizeCatalog(f);
    else if (stage === 'start-ui') await startUI(f);
    else if (stage === 'serve-ui') { await serveUI(f); return; }
    else if (stage === 'start') await start(f);
    else throw Error('unsupported_fixture_stage');
    (f.report.completedStages ??= []).push(stage); await f.saveReport();
    process.stdout.write(`native_object_storage_${stage}_passed ${runID}\n`);
  } catch (error) { await f.write('failure-' + Date.now() + '.json', { stage, error: String(error), stack: error.stack }); throw Error('native_object_storage_stage_failed_private_diagnostics_retained'); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main(process.argv.slice(2)).catch(error => { process.stderr.write(error.message + '\n'); process.exitCode = 1; });
