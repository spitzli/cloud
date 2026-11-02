// Opt-in Mail acceptance only. Importing this module never starts infrastructure.
import assert from 'node:assert/strict';
import { constants, existsSync, openSync, readFileSync, closeSync } from 'node:fs';
import { readFile, open, lstat, mkdir, rename, unlink, chmod, readdir, rm, realpath } from 'node:fs/promises';
import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes, X509Certificate } from 'node:crypto';
import { dirname, resolve, join, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export const BASE = join(ROOT, '.runtime-test/native-mail-installation');
export const STAGES = ['preflight', 'artifacts', 'prepare', 'import', 'start', 'observe', 'acceptance', 'cleanup'];
export const BINARY_NAMES = ['webdock-migrate', 'webdock-bootstrap-operator', 'webdock-enroll-agent', 'webdock-bootstrap-plugins', 'webdock-control-plane', 'webdock-plugin-control', 'webdock-plugin-controller', 'webdock-agent-control', 'webdock-agent', 'bun', 'docker-buildx'];
export const NETWORK = 'webdock-dev_default', K3S = 'webdock-dev-k3s-1', CONTEXT = 'webdock-dev';
const RETAINED_INSPECT = '{"Id":{{json .Id}},"State":{"Status":{{json .State.Status}},"Running":{{json .State.Running}}},"NetworkSettings":{"Networks":{"webdock-dev_default":{"IPAddress":{{json (index .NetworkSettings.Networks "webdock-dev_default").IPAddress}}}}},"Mounts":[{{range $i,$m := .Mounts}}{{if $i}},{{end}}{"Type":{{json $m.Type}},"Name":{{json (index $m "Name")}},"Source":{{json $m.Source}},"Destination":{{json $m.Destination}}}{{end}}]}';
const OWNED_INSPECT = '{"Id":{{json .Id}},"Name":{{json .Name}},"State":{"Status":{{json .State.Status}},"Running":{{json .State.Running}},"ExitCode":{{json .State.ExitCode}}},"Config":{"Labels":{"webdock.native-mail":{{json (index .Config.Labels "webdock.native-mail")}}}},"NetworkSettings":{"Networks":{{json .NetworkSettings.Networks}},"Ports":{{json .NetworkSettings.Ports}}},"Mounts":[{{range $i,$m := .Mounts}}{{if $i}},{{end}}{"Type":{{json $m.Type}},"Name":{{json (index $m "Name")}},"Source":{{json $m.Source}},"Destination":{{json $m.Destination}},"RW":{{json $m.RW}}}{{end}}]}';
export const sha256 = value => createHash('sha256').update(value).digest('hex');
export const shellQuote = value => "'" + String(value).replaceAll("'", "'\"'\"'") + "'";
export const delay = ms => new Promise(done => setTimeout(done, ms));
export const random = () => randomBytes(32).toString('hex');
export function requirePrivateEqual(actual, expected, category) {
  if (!isDeepStrictEqual(actual, expected)) throw Error(category);
}
export function frozenInput(input, snapshot, report, stage) {
  assert(input.runID === report.runID && input.directory === report.directory, 'fixture_custody_identity_changed');
  assert(snapshot.runID === report.runID && snapshot.directory === report.directory && sha256(JSON.stringify(snapshot)) === report.inputSha256, 'fixture_snapshot_changed');
  if (stage === 'cleanup') return snapshot;
  const requested = { ...input }; delete requested.authorizedStages;
  assert.equal(sha256(JSON.stringify(requested)), report.inputSha256, 'fixture_frozen_input_changed');
  return input;
}
export function stageAllowed(input, stage) {
  assert(STAGES.includes(stage) && input.authorizedStages?.includes(stage), 'fixture_stage_not_authorized');
  return true;
}
export function ownEnvironment(directory, generated = {}) {
  return { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: join(directory, 'home'), TMPDIR: join(directory, 'tmp'), LANG: 'C.UTF-8', ...generated };
}
export function fixtureOrigins(ports) {
  return { core: `http://127.0.0.1:${ports.core}`, console: `http://127.0.0.1:${ports.console}`, studio: `http://localhost:${ports.console}`, account: `http://localhost:${ports.account}` };
}
export function redact(value, secrets = []) {
  let result = String(value);
  for (const secret of secrets.filter(Boolean).sort((a, b) => b.length - a.length)) result = result.replaceAll(secret, '[REDACTED]');
  return result.replace(/postgres(?:ql)?:\/\/[^\s"'<>]+/gi, '[REDACTED_DSN]')
    .replace(/-----BEGIN [^-\r\n]*PRIVATE KEY-----[\s\S]*?-----END [^-\r\n]*PRIVATE KEY-----/g, '[REDACTED_KEY]')
    .replace(/(?:password|secret|token|authorization|cookie|set-cookie)\s*[:=]\s*[^\r\n]+/gi, '[REDACTED_FIELD]');
}
export async function readPrivate(path) {
  assert(isAbsolute(path), 'private_absolute_path_required');
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const s = await file.stat();
    assert(s.isFile() && s.nlink === 1 && s.uid === process.getuid() && (s.mode & 0o077) === 0 && s.size <= 16 * 1024 * 1024, 'private_file_required');
    return await file.readFile('utf8');
  } finally { await file.close(); }
}
export async function writePrivate(path, value, replace = false) {
  assert(isAbsolute(path), 'private_absolute_path_required');
  const bytes = typeof value === 'string' || Buffer.isBuffer(value) ? value : JSON.stringify(value, null, 2) + '\n';
  if (existsSync(path)) await readPrivate(path);
  const temporary = replace ? path + '.' + randomBytes(8).toString('hex') + '.tmp' : path;
  const file = await open(temporary, 'wx', 0o600);
  try { await file.writeFile(bytes); await file.sync(); } finally { await file.close(); }
  if (replace) await rename(temporary, path);
  const directory = await open(dirname(path), constants.O_RDONLY | constants.O_DIRECTORY);
  try { await directory.sync(); } finally { await directory.close(); }
}
export async function custodyIntent(ledger, save, kind, name, attributes = {}) {
  assert(!ledger.entries.some(e => e.kind === kind && e.name === name && e.status !== 'removed'), 'custody_duplicate_intent');
  const item = { kind, name, ...attributes, status: 'intent' };
  ledger.entries.push(item); await save(); return item;
}
// Only finalization before callback startup uses this: rename would strand a file bind mount.
export async function writePinnedPrivate(path, value) {
  await readPrivate(path);
  const file = await open(path, constants.O_RDWR | constants.O_NOFOLLOW);
  try { await file.truncate(0); await file.writeFile(JSON.stringify(value, null, 2) + '\n'); await file.sync(); }
  finally { await file.close(); }
}
export async function custodyConfirm(item, save, identity) {
  Object.assign(item, identity, { status: 'owned' }); await save();
}
export function requireContainer(item, actual, runID) {
  assert(item.id && actual.Id === item.id && actual.Name === '/' + item.name && actual.Config.Labels?.['webdock.native-mail'] === runID, 'container_custody_mismatch');
}
export function requireDockerMissing(result, kind, identity) {
  const text = (result.stderr ?? '').trim().replace(/^Error(?: response from daemon)?:\s*/i, '').toLowerCase();
  const names = kind === 'container' ? ['container', 'object'] : [kind];
  assert(result.status === 1 && !result.error && !result.signal && names.some(name => text === `no such ${name}: ${identity}`.toLowerCase()), 'docker_absence_unverified');
}
export function requireObject(item, actual) {
  assert(item.uid && actual.metadata?.uid === item.uid && actual.metadata.name === item.name && (!item.namespace || item.namespace === actual.metadata.namespace), 'object_custody_mismatch');
}
export function runtimeNamespace(platform, generation) {
  const identity = generation === 1 ? `${platform.length}:${platform}4:mail` : `v2:${platform.length}:${platform}4:mail:${generation}`;
  return 'wd-plugin-' + sha256(identity).slice(0, 24);
}
export async function expectNamespace(f, name) {
  assert.equal((await f.kube(['get', 'namespace', name, '--ignore-not-found', '-o', 'name'])).output, '', 'fixture_namespace_preexists');
  const item = await custodyIntent(f.custody, f.saveCustody, 'namespace-intent', name);
  await custodyConfirm(item, f.saveCustody, { absentBeforeRequest: true });
}
export function validateInput(input, stage) {
  assert(Object.keys(input).every(key => ['version', 'runID', 'directory', 'authorizedStages', 'allowStartRetainedK3s', 'review', 'images', 'ports', 'kubeconfig'].includes(key)), 'unknown_fixture_input_field');
  assert(input.version === 1 && /^[a-f0-9]{12}$/.test(input.runID), 'fixture_input_version_or_id');
  assert(input.directory === join(BASE, input.runID), 'fixture_directory_mismatch');
  assert(Array.isArray(input.authorizedStages) && input.authorizedStages.every(s => STAGES.includes(s)), 'fixture_stages_invalid');
  assert(typeof input.allowStartRetainedK3s === 'boolean', 'explicit_retained_start_policy_required');
  assert(input.kubeconfig === join(ROOT, '.dev-local/k3s/kubeconfig.yaml'), 'explicit_dev_kubeconfig_required');
  assert(input.review?.sourceFiles && Object.keys(input.review.sourceFiles).length > 0, 'source_freeze_required');
  for (const [path, hash] of Object.entries(input.review.sourceFiles)) {
    assert(!isAbsolute(path) && !path.split('/').includes('..') && !/(?:HANDOVER|\.env|demo-owner|kubeconfig|\.runtime-test)/i.test(path) && /^[a-f0-9]{64}$/.test(hash), 'source_freeze_invalid');
  }
  for (const name of stage === 'cleanup' ? BINARY_NAMES.slice(0, 9) : BINARY_NAMES) assert(isAbsolute(input.review.binaries?.[name]?.path ?? '') && /^[a-f0-9]{64}$/.test(input.review.binaries[name].sha256), 'reviewed_binary_required');
  for (const name of ['postgres', 'registry', 'stalwart']) assert(/^[a-zA-Z0-9./:_-]+@sha256:[a-f0-9]{64}$/.test(input.images?.[name] ?? '') && !input.images[name].endsWith('0'.repeat(64)), 'pinned_fixture_image_required');
  const ports = ['core', 'console', 'account', 'callback'].map(name => input.ports?.[name]);
  assert(ports.every(port => Number.isInteger(port) && port >= 1024 && port <= 65535) && new Set(ports).size === 4, 'distinct_fixture_ports_required');
  return input;
}

export class Fixture {
  constructor(input, report, custody) { this.input = input; this.report = report; this.custody = custody; this.directory = input.directory; this.step = 0; }
  path(name) { assert(!isAbsolute(name) && !name.split('/').includes('..')); return join(this.directory, name); }
  async json(path) { return JSON.parse(await readPrivate(path)); }
  saveReport = () => writePrivate(this.path('report.json'), this.report, true);
  saveCustody = () => writePrivate(this.path('custody.json'), this.custody, true);
  async write(name, value, replace = false) { await writePrivate(this.path(name), value, replace); return this.path(name); }
  async command(program, args, { env = {}, input, timeout = 120000, allowFailure = false } = {}) {
    const name = `${Date.now()}-${process.pid}-${++this.step}`;
    const result = spawnSync(program, args, { cwd: ROOT, env: ownEnvironment(this.directory, env), input, encoding: 'utf8', timeout, maxBuffer: 64 * 1024 * 1024, shell: false });
    await this.write('logs/' + name + '.stdout', result.stdout ?? '');
    await this.write('logs/' + name + '.stderr', result.stderr ?? '');
    assert(allowFailure || result.status === 0, 'fixture_command_failed');
    return { ...result, output: (result.stdout ?? '').trim() };
  }
  async docker(args, options) { return this.command('docker', ['--host', 'unix:///var/run/docker.sock', ...args], options); }
  async inspect(name) { return JSON.parse((await this.docker(['inspect', '--format', OWNED_INSPECT, name])).output); }
  async inspectRetained(name = K3S) { return JSON.parse((await this.docker(['inspect', '--format', RETAINED_INSPECT, name])).output); }
  async kube(args, options) { return this.command('kubectl', ['--kubeconfig', this.input.kubeconfig, '--context', CONTEXT, '--request-timeout=15s', ...args], options); }
  async kubeJSON(args) { return JSON.parse((await this.kube([...args, '-o', 'json'])).output); }
  binary(name) { return this.input.review.binaries[name].path; }
  async ownContainer(name, args, env = {}) {
    const item = await custodyIntent(this.custody, this.saveCustody, 'container', name);
    const output = await this.docker(['create', '--name', name, '--label', `webdock.native-mail=${this.input.runID}`, ...args], { env });
    const id = output.output; assert(/^[a-f0-9]{64}$/.test(id), 'container_id_unavailable');
    const object = await this.inspect(id);
    assert.equal(object.Config.Labels['webdock.native-mail'], this.input.runID);
    await custodyConfirm(item, this.saveCustody, { id, volumes: object.Mounts.filter(m => m.Type === 'volume').map(m => m.Name) });
    await this.docker(['start', id]); return this.inspect(id);
  }
  async startProcess(name, program, args, env) {
    const expected = await executableIdentity(this, program);
    const item = await custodyIntent(this.custody, this.saveCustody, 'process', name, { executable: expected.path, executableSha256: expected.sha256, argsSha256: sha256(JSON.stringify(args)) });
    const stdout = openSync(this.path('logs/' + name + '.stdout'), 'wx', 0o600), stderr = openSync(this.path('logs/' + name + '.stderr'), 'wx', 0o600);
    try {
      const child = spawn(expected.path, args, { cwd: ROOT, env: ownEnvironment(this.directory, env), stdio: ['ignore', stdout, stderr], detached: true, shell: false });
      await new Promise((done, reject) => { child.once('spawn', done); child.once('error', () => reject(Error('fixture_process_start_failed'))); });
      child.unref();
      item.pid = child.pid; item.startTime = processIdentity(child.pid).startTime; await this.saveCustody();
      const identity = await settleProcessIdentity(child.pid, { startTime: item.startTime, executable: expected.path, executableSha256: expected.sha256, argv: [expected.path, ...args], uid: process.getuid() });
      await custodyConfirm(item, this.saveCustody, { pid: child.pid, startTime: identity.startTime, commandSha256: identity.commandSha256 }); child.unref(); return item;
    } finally { closeSync(stdout); closeSync(stderr); }
  }
  async ownObject(kind, object) {
    const { name, namespace, uid } = object.metadata;
    let item = this.custody.entries.find(e => e.kind === kind && e.name === name && e.namespace === namespace);
    if (item) { requireObject(item, object); return item; }
    assert(this.custody.entries.some(e => e.kind === 'namespace-intent' && e.name === (namespace ?? name)), 'object_requires_prior_namespace_custody');
    item = await custodyIntent(this.custody, this.saveCustody, kind, name, { namespace });
    await custodyConfirm(item, this.saveCustody, { uid }); return item;
  }
  async wait(check, milliseconds = 120000) {
    const end = Date.now() + milliseconds;
    while (Date.now() < end) { const result = await check(); if (result) return result; await delay(500); }
    throw Error('fixture_deadline_exceeded');
  }
}
export function processIdentity(pid) {
  const value = readFileSync(`/proc/${pid}/stat`, 'utf8');
  return { startTime: value.slice(value.lastIndexOf(')') + 2).split(' ')[19], commandSha256: sha256(readFileSync(`/proc/${pid}/cmdline`)) };
}
async function observedProcess(pid) {
  const initial = processIdentity(pid), executable = await realpath(`/proc/${pid}/exe`), command = await readFile(`/proc/${pid}/cmdline`), uid = (await lstat(`/proc/${pid}`)).uid;
  const executableSha256 = sha256(await readFile(`/proc/${pid}/exe`));
  assert(processIdentity(pid).startTime === initial.startTime, 'process_identity_changed');
  return { ...initial, commandSha256: sha256(command), executable, executableSha256, uid, argv: command.toString().split('\0').filter(Boolean) };
}
export async function settleProcessIdentity(pid, expected, observe = observedProcess, pause = delay, attempts = 100) {
  for (let n = 0; n < attempts; n++) {
    const actual = await observe(pid);
    assert(actual.startTime === expected.startTime && actual.uid === expected.uid, 'process_identity_changed');
    if (actual.executable === expected.executable && actual.executableSha256 === expected.executableSha256 && isDeepStrictEqual(actual.argv, expected.argv)) return actual;
    await pause(20);
  }
  throw Error('process_exec_not_settled');
}
async function publicELF(path, expectedHash) {
  const file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const s = await file.stat(); assert(s.isFile() && s.nlink === 1 && !(s.mode & 0o022) && (s.mode & 0o111) && [0, process.getuid()].includes(s.uid), 'reviewed_public_elf_required');
    const bytes = await file.readFile(); assert(bytes.subarray(0, 4).equals(Buffer.from([127, 69, 76, 70])) && (!expectedHash || sha256(bytes) === expectedHash), 'reviewed_elf_hash_changed'); return bytes;
  } finally { await file.close(); }
}
async function executableIdentity(f, program) {
  const reviewed = Object.values(f.input.review.binaries).find(item => item.path === program);
  if (reviewed) return { path: await realpath(program), sha256: sha256(await publicELF(program, reviewed.sha256)) };
  assert(['docker', 'kubectl'].includes(program), 'unreviewed_process_executable');
  for (const directory of ['/usr/local/bin', '/usr/bin', '/bin']) {
    const candidate = join(directory, program);
    try { const path = await realpath(candidate); return { path, sha256: sha256(await publicELF(path)) }; } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  throw Error('system_process_tool_missing');
}
export async function stageBuildx(f) {
  const config = f.path('home/.docker/config.json');
  try { await lstat(config); throw Error('private_docker_config_present'); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  for (const name of ['home', 'home/.docker', 'home/.docker/cli-plugins']) {
    await mkdir(f.path(name), { mode: 0o700, recursive: true }); const s = await lstat(f.path(name));
    assert(s.isDirectory() && !s.isSymbolicLink() && s.uid === process.getuid() && !(s.mode & 0o077), 'owned_tool_directory_required');
  }
  const tool = f.input.review.binaries['docker-buildx'], bytes = await publicELF(tool.path, tool.sha256), target = f.path('home/.docker/cli-plugins/docker-buildx');
  try { const s = await lstat(target); assert(s.uid === process.getuid(), 'owned_tool_required'); await publicELF(target, tool.sha256); }
  catch (error) { if (error.code !== 'ENOENT') throw error; await f.write('home/.docker/cli-plugins/docker-buildx', bytes); await chmod(target, 0o500); }
  const result = await f.docker(['buildx', 'version']); assert(result.status === 0 && result.output.includes('github.com/docker/buildx'), 'reviewed_buildx_unavailable');
}
export async function openFixture(inputPath, stage) {
  let input = validateInput(JSON.parse(await readPrivate(inputPath)), stage); stageAllowed(input, stage);
  await mkdir(BASE, { recursive: true, mode: 0o700 });
  await mkdir(input.directory, { recursive: true, mode: 0o700 });
  const metadata = await lstat(input.directory);
  assert(metadata.isDirectory() && !metadata.isSymbolicLink() && metadata.uid === process.getuid() && (metadata.mode & 0o077) === 0, 'private_run_directory_required');
  for (const name of ['logs', 'home', 'tmp']) await mkdir(join(input.directory, name), { mode: 0o700, recursive: true });
  const lock = await open(join(input.directory, 'active.lock'), 'wx', 0o600);
  const release = async () => { await lock.close(); await unlink(join(input.directory, 'active.lock')); };
  try {
    const frozen = { ...input }; delete frozen.authorizedStages;
    const inputSha256 = sha256(JSON.stringify(frozen));
    const reportPath = join(input.directory, 'report.json');
    const snapshotPath = join(input.directory, 'input-snapshot.json');
    assert(stage !== 'cleanup' || existsSync(reportPath) && existsSync(snapshotPath), 'cleanup_requires_original_custody');
    const report = existsSync(reportPath) ? JSON.parse(await readPrivate(reportPath)) : { version: 1, fixture: true, kind: 'managed-mail', runID: input.runID, directory: input.directory, inputSha256, completedStages: [], paths: { input: inputPath, custody: join(input.directory, 'custody.json') } };
    if (!existsSync(snapshotPath)) await writePrivate(snapshotPath, frozen);
    input = frozenInput(input, JSON.parse(await readPrivate(snapshotPath)), report, stage);
    const custodyPath = join(input.directory, 'custody.json');
    const custody = existsSync(custodyPath) ? JSON.parse(await readPrivate(custodyPath)) : { version: 1, runID: input.runID, entries: [], retainedK3s: null };
    assert.equal(custody.runID, input.runID);
    const fixture = new Fixture(input, report, custody); fixture.release = release;
    await fixture.saveCustody(); await fixture.saveReport(); return fixture;
  } catch (error) { await release(); throw error; }
}
async function verifyFreeze(f) {
  for (const [path, hash] of Object.entries(f.input.review.sourceFiles)) assert.equal(sha256(await readFile(join(ROOT, path))), hash, 'reviewed_source_changed');
  for (const item of Object.values(f.input.review.binaries)) assert.equal(sha256(await readFile(item.path)), item.sha256, 'reviewed_binary_changed');
}
export function requireDevContext(context, endpoint) {
  assert.equal(context, CONTEXT, 'dev_context_required');
  assert.equal(endpoint, 'https://127.0.0.1:56443', 'dev_loopback_api_required');
}
export async function guardDev(f, start = false) {
  const context = (await f.command('kubectl', ['--kubeconfig', f.input.kubeconfig, 'config', 'current-context'])).output;
  // Public filtered fields only; never config view --raw or a whole kubeconfig read/copy.
  requireDevContext(context, (await f.kube(['config', 'view', '--minify', '-o', 'jsonpath={.clusters[0].cluster.server}'])).output);
  let node = await f.inspectRetained(); assert(node.NetworkSettings.Networks[NETWORK], 'retained_network_missing');
  if (!f.custody.retainedK3s) {
    f.custody.retainedK3s = { id: node.Id, initialState: node.State.Status, started: false, mounts: node.Mounts.map(m => ({ type: m.Type, name: m.Name, source: m.Source, destination: m.Destination })) };
    await f.saveCustody();
  }
  assert.equal(node.Id, f.custody.retainedK3s.id, 'retained_node_identity_changed');
  if (!node.State.Running && start) {
    assert(f.input.allowStartRetainedK3s && node.State.Status === 'exited', 'retained_start_not_authorized');
    f.custody.retainedK3s.startIntent = true; await f.saveCustody();
    await f.docker(['start', node.Id]); f.custody.retainedK3s.started = true; await f.saveCustody(); node = await f.inspectRetained(node.Id);
  }
  return node;
}
export async function verifyExecution(f) {
  await verifyFreeze(f);
  const node = await guardDev(f);
  assert(node.State.Running, 'owned_fixture_node_not_running');
}
async function preflight(f) {
  await verifyFreeze(f); await guardDev(f);
}
async function tls(f, name) {
  const path = f.path(name); await mkdir(path, { mode: 0o700 });
  await f.command('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', join(path, 'ca.key'), '-out', join(path, 'ca.crt'), '-days', '3', '-subj', '/CN=Owned Mail ' + name, '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign']);
  await f.command('openssl', ['req', '-new', '-newkey', 'rsa:2048', '-nodes', '-keyout', join(path, 'server.key'), '-out', join(path, 'server.csr'), '-subj', '/CN=localhost']);
  for (const file of await readdir(path)) await chmod(join(path, file), 0o600);
  return path;
}
async function startPG(f, name) {
  const path = await tls(f, name), password = random(), container = `wd-mail-${name}-${f.input.runID}`;
  // A fixed script handles image-owned staging; no private value is interpolated into shell.
  const script = 'mkdir -p /tmp/owned-tls; cp /fixture-tls/server.crt /fixture-tls/server.key /tmp/owned-tls/; chown -R postgres:postgres /tmp/owned-tls; chmod 0600 /tmp/owned-tls/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/owned-tls/server.crt -c ssl_key_file=/tmp/owned-tls/server.key';
  // Create before start so the final private Docker IP can enter the certificate SAN.
  const item = await custodyIntent(f.custody, f.saveCustody, 'container', container);
  const result = await f.docker(['create', '--name', container, '--label', `webdock.native-mail=${f.input.runID}`, '--network', NETWORK, '--env', 'POSTGRES_PASSWORD', '--env', 'POSTGRES_DB=webdock_mail_fixture', '--publish', '127.0.0.1::5432', '--mount', `type=bind,source=${path},target=/fixture-tls,readonly`, '--entrypoint', 'sh', f.input.images.postgres, '-ec', script], { env: { POSTGRES_PASSWORD: password } });
  const id = result.output; let actual = await f.inspect(id);
  await custodyConfirm(item, f.saveCustody, { id, volumes: actual.Mounts.filter(m => m.Type === 'volume').map(m => m.Name) });
  // Docker assigns network addresses at start; start with a localhost cert, then reissue once.
  await f.write(`${name}/server.ext`, 'subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n');
  const issue = () => f.command('openssl', ['x509', '-req', '-in', join(path, 'server.csr'), '-CA', join(path, 'ca.crt'), '-CAkey', join(path, 'ca.key'), '-CAcreateserial', '-out', join(path, 'server.crt'), '-days', '3', '-extfile', join(path, 'server.ext')]);
  await issue(); await chmod(join(path, 'server.crt'), 0o600);
  await f.docker(['start', id]); actual = await f.inspect(id); const ip = actual.NetworkSettings.Networks[NETWORK].IPAddress;
  assert(privateIP(ip), 'private_database_ip_required');
  await f.write(`${name}/server.ext`, `subjectAltName=DNS:localhost,IP:127.0.0.1,IP:${ip}\nbasicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n`, true);
  await issue(); await chmod(join(path, 'server.crt'), 0o600); await f.docker(['restart', id]); actual = await f.inspect(id);
  const port = Number(actual.NetworkSettings.Ports['5432/tcp'][0].HostPort), ca = await readPrivate(join(path, 'ca.crt'));
  const url = new URL(`postgresql://postgres@127.0.0.1:${port}/webdock_mail_fixture`); url.password = password;
  const { Client } = await import('pg');
  await f.wait(async () => { const db = new Client({ connectionString: url.href, ssl: { ca, rejectUnauthorized: true }, connectionTimeoutMillis: 1000 }); try { await db.connect(); await db.query('SELECT 1'); return true; } catch { return false; } finally { await db.end().catch(() => {}); } });
  const storage = { container, id, ip, port, tls: path, ca, url: url.href };
  await f.write('storage-' + name + '.json', storage); return storage;
}
export function privateIP(ip) {
  const values = ip.split('.').map(Number);
  return values.length === 4 && values.every(v => Number.isInteger(v) && v >= 0 && v < 256) && (values[0] === 10 || values[0] === 172 && values[1] >= 16 && values[1] < 32 || values[0] === 192 && values[1] === 168);
}
async function prepare(f) {
  assert(f.report.completedStages.includes('artifacts'), 'artifacts_stage_required');
  const node = await guardDev(f, true);
  await f.wait(async () => {
    const ready = await f.kube(['get', '--raw', '/readyz'], { allowFailure: true, timeout: 5000 });
    return ready.status === 0 && ready.output === 'ok';
  }, 120000);
  await f.kube(['wait', '--for=condition=Ready', 'nodes', '--all', '--timeout=120s']);
  for (const crd of ['certificates.cert-manager.io', 'ingressroutes.traefik.io', 'ingressroutetcps.traefik.io']) await f.kube(['get', 'crd', crd, '-o', 'name']);
  const classes = await f.kubeJSON(['get', 'storageclasses']); assert(classes.items.some(c => c.metadata.annotations?.['storageclass.kubernetes.io/is-default-class'] === 'true'), 'default_storageclass_required');
  await f.kube(['-n', 'kube-system', 'rollout', 'status', 'deployment/traefik', '--timeout=120s']);
  const initialMailPVCUIDs = (await f.kubeJSON(['get', 'pvc', '--all-namespaces', '-l', 'webdock.dev/plugin=mail'])).items.map(c => c.metadata.uid);
  const platform = await startPG(f, 'platform'), metadata = await startPG(f, 'metadata');
  const roles = ['webdock_auth_bootstrap', 'webdock_auth_runtime', 'webdock_worker_runtime', 'webdock_admin_runtime', 'webdock_registry_runtime', 'webdock_runtime', 'webdock_browser_website', 'webdock_cms_archive_runtime', 'webdock_offline_migrate'];
  const passwords = Object.fromEntries(roles.map(name => [name, random()])); await f.write('roles.json', passwords);
  const dbURL = (role, remote = false) => { const url = new URL(platform.url); url.username = role; url.password = role === 'postgres' ? new URL(platform.url).password : passwords[role]; url.hostname = remote ? platform.ip : '127.0.0.1'; url.port = String(remote ? 5432 : platform.port); url.searchParams.set('sslmode', 'verify-full'); url.searchParams.set('sslrootcert', join(platform.tls, 'ca.crt')); return url.href; };
  const ports = f.input.ports, platformID = 'mail-' + f.input.runID;
  const origins = fixtureOrigins(ports);
  const env = { NODE_ENV: 'development', PORT: String(ports.core), WEBDOCK_BIND_ADDRESS: '127.0.0.1', WEBDOCK_API_ORIGIN: origins.core, WEBDOCK_AUTH_ORIGIN: origins.account, WEBDOCK_STUDIO_ORIGIN: origins.studio, WEBDOCK_CONSOLE_ORIGIN: origins.console, WEBDOCK_AUTH_SECRET: random(), DATABASE_RLS_SECRET: random(), DATABASE_CA_CERT: platform.ca, WEBDOCK_FEATURES: JSON.stringify({ hosting: 'disabled', databases: 'disabled', mail: 'disabled' }), WEBDOCK_WORKLOADS_ENABLED: 'true' };
  const target = `127.0.0.1:${platform.port}/webdock_mail_fixture`;
  await f.command(f.binary('webdock-migrate'), ['--target', target, '--initialize-empty', '--provision-roles-from', f.path('roles.json')], { env: { ...env, WEBDOCK_MIGRATION_DATABASE_URL: dbURL('postgres') } });
  const offline = { ...env, DATABASE_URL_UNPOOLED: dbURL('webdock_offline_migrate') };
  const operator = await f.write('operator-input.json', { email: `mail-${f.input.runID}@example.test`, name: 'Owned Mail operator', password: 'Owned-' + random() + '!', authOrigin: origins.account, studioOrigin: origins.studio, consoleOrigin: origins.console });
  await f.command(f.binary('webdock-bootstrap-operator'), ['--target', target, '--config', operator, '--output', f.path('operator-output.json')], { env: offline });
  await f.write('enrollment-input.json', { platformID, agentID: 'mail-agent', name: 'Owned Mail acceptance', country: 'de', region: 'eu', locationEvidence: 'Explicitly owned local webdock-dev acceptance; no production location claim', transport: { kind: 'unix', uid: process.getuid() } });
  await f.command(f.binary('webdock-enroll-agent'), ['--target', target, '--config', f.path('enrollment-input.json'), '--output', f.path('enrollment.json')], { env: offline });
  const enrollment = await f.json(f.path('enrollment.json')), artifact = await f.json(f.path('artifacts.json'));
  const catalog = f.path('catalog.json'), callbackHost = `mail-core-${f.input.runID}.test`;
  await f.write('catalog.json', { version: 1, platformID, keys: [artifact.key], targets: [], releases: [] });
  await f.write('kernel-input.json', { platformID, installationCatalog: catalog, callbackOrigin: `https://${callbackHost}:${ports.callback}`, listenAddress: `0.0.0.0:${ports.callback}` });
  await f.command(f.binary('webdock-bootstrap-plugins'), ['--target', target, '--config', f.path('kernel-input.json'), '--output', f.path('kernel')], { env: offline });
  const kernel = f.path('kernel'), callbackName = 'wd-mail-callback-' + f.input.runID;
  const callbackEnv = { ...env, ...((await f.json(f.path('operator-output.json'))).environment), WEBDOCK_NATIVE_PLUGIN_CONFIG: join(kernel, 'plugin-config.json') };
  const ownEnv = { ...callbackEnv };
  for (const [key, role] of [['WEBDOCK_DATABASE_URL', 'webdock_auth_bootstrap'], ['WEBDOCK_SESSION_DATABASE_URL', 'webdock_auth_runtime'], ['WEBDOCK_WORKER_DATABASE_URL', 'webdock_worker_runtime'], ['WEBDOCK_REGISTRY_DATABASE_URL', 'webdock_registry_runtime']]) { ownEnv[key] = dbURL(role); callbackEnv[key] = dbURL(role, true); }
  const files = ['plugin-config.json', 'host-bindings.json', 'connections.json', 'invocation.key', 'core.crt', 'core.key', 'ca.crt'].map(name => join(kernel, name)); files.push(catalog, join(platform.tls, 'ca.crt'));
  const executable = f.binary('webdock-plugin-control');
  const callback = await f.ownContainer(callbackName, ['--network', NETWORK, '--network-alias', callbackHost, '--user', `${process.getuid()}:${process.getgid()}`, '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', ...files.flatMap(path => ['--mount', `type=bind,source=${path},target=${path},readonly`]), '--mount', `type=bind,source=${executable},target=/reviewed-callback,readonly`, '--entrypoint', 'sh', artifact.baseImage, '-ec', 'exec sleep infinity']);
  const callbackIP = callback.NetworkSettings.Networks[NETWORK].IPAddress, nodeIP = node.NetworkSettings.Networks[NETWORK].IPAddress; assert(privateIP(callbackIP) && privateIP(nodeIP));
  const pin = (await f.json(join(kernel, 'bootstrap.json'))).coreCertificateSha256; assert.equal(pin, sha256(new X509Certificate(await readPrivate(join(kernel, 'core.crt'))).raw));
  const routeDomains = [`mail-${f.input.runID}.localhost`, `mail-admin-${f.input.runID}.localhost`];
  const gateway = await prepareMailGateway(f, nodeIP, artifact.baseImage, routeDomains[1]);
  const managementEgress = gateway.egress;
  const recipe = { storage: true, sealingKey: true, routeDomains, privateConfiguration: { platformTransport: { kind: 'smtp', host: '127.0.0.1', port: 9 }, trustUnverifiedDomains: true, mailboxes: { image: f.input.images.stalwart, domain: routeDomains[0], imapPort: 993, submissionPort: 465, admin: { origin: gateway.origin, domain: routeDomains[1], entryPoints: ['web'] }, issuer: null } } };
  // The recipe is installed before services load the signed catalog; signed manifests stay exact.
  const releases = artifact.releases.map(release => ({ ...release, recipe }));
  const policy = { runtimeDomain: `mail-runtime-${f.input.runID}.test`, runtimePort: 8443, entryPoint: 'websecure', runtimeOriginPort: 443, runtimeIngressHost: nodeIP, core: { origin: `https://${callbackHost}:${ports.callback}`, domain: callbackHost, certificate: join(kernel, 'core.crt'), privateKey: join(kernel, 'core.key'), authority: join(kernel, 'ca.crt'), serverCertificateSha256: pin }, runtimeIdentity: { authority: join(kernel, 'ca.crt'), privateKey: join(kernel, 'ca.key') }, storageID: 'metadata', resources: { cpuMillicores: 100, memoryBytes: 268435456 }, egress: [{ cidr: callbackIP + '/32', port: ports.callback }, { cidr: metadata.ip + '/32', port: 5432 }, ...managementEgress], hostAliases: [{ ip: callbackIP, hostnames: [callbackHost] }] };
  await writePinnedPrivate(catalog, { version: 1, platformID, keys: [artifact.key], targets: [{ id: 'managed-eu', label: 'Owned local Mail acceptance', clusterID: enrollment.clusterID, policy }], releases });
  await f.write('worker.json', { storageTargets: [{ id: 'metadata', adminUrl: metadata.url, authority: join(metadata.tls, 'ca.crt'), runtimeHost: metadata.ip, runtimePort: 5432 }] });
  const socketDirectory = '/tmp/wdmi-' + f.input.runID;
  const socketItem = await custodyIntent(f.custody, f.saveCustody, 'directory', socketDirectory);
  await mkdir(socketDirectory, { mode: 0o700 }); const socketStat = await lstat(socketDirectory); await custodyConfirm(socketItem, f.saveCustody, { inode: socketStat.ino, device: socketStat.dev });
  const socket = join(socketDirectory, 'control.sock');
  await f.write('agent-control.json', { kind: 'unix', path: socket, installation: platformID, agent_id: 'mail-agent', uid: process.getuid() });
  await mkdir(f.path('agent-state'), { mode: 0o700 });
  const chart = join(ROOT, 'deploy/helm/webdock-plugin');
  await f.write('agent.json', { version: 1, installation: platformID, agentId: 'mail-agent', generation: enrollment.identity.generation, stateDirectory: f.path('agent-state'), control: { kind: 'unix', path: socket }, runtime: { kind: 'kubernetes', kubeconfig: f.input.kubeconfig, context: CONTEXT, helm: '/usr/local/bin/helm', chart, chartSha256: await chartHash(chart) }, pluginRuntimeEnabled: true, workloads: true });
  await f.write('core-environment.json', ownEnv); await f.write('callback-environment.json', callbackEnv);
  Object.assign(f.report, { platformID, clusterID: enrollment.clusterID, origins, paths: { ...f.report.paths, operator, catalog, kernel, worker: f.path('worker.json'), agentControl: f.path('agent-control.json'), agent: f.path('agent.json'), coreEnvironment: f.path('core-environment.json'), callbackEnvironment: f.path('callback-environment.json'), mailGateway: gateway.configuration }, topology: { callbackContainer: callbackName, callbackID: callback.Id, callbackIP, callbackMounts: [...files, executable], metadataIP: metadata.ip, nodeIP, managementIP: gateway.ip, gatewayID: gateway.id, managementEgress, routeDomains, initialMailPVCUIDs } });
  await f.saveReport();
}
export async function prepareMailGateway(f, nodeIP, baseImage, adminDomain) {
  assert(privateIP(nodeIP), 'private_node_ingress_required');
  const script = join(ROOT, 'tools/tests/native-mail-gateway.mjs'), source = await readFile(script);
  assert(f.input.review.sourceFiles['tools/tests/native-mail-gateway.mjs'] === sha256(source), 'reviewed_gateway_source_required');
  const bun = f.input.review.binaries.bun; await publicELF(bun.path, bun.sha256);
  const image = f.custody.entries.find(e => e.kind === 'docker-image' && e.name === baseImage && e.absentBeforeCreate);
  assert(image && JSON.parse((await f.docker(['image', 'inspect', '--format', '{{json .Id}}', baseImage])).output) === image.id, 'owned_gateway_base_changed');
  const configuration = await f.write('mail-gateway.json', { version: 1, runID: f.input.runID, adminDomain, upstream: `http://${nodeIP}:80`, maxBodyBytes: 262144, timeoutMs: 4000 });
  const container = await f.ownContainer('wd-mail-management-' + f.input.runID, ['--network', NETWORK, '--user', `${process.getuid()}:${process.getgid()}`, '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--tmpfs', '/tmp:rw,noexec,nosuid,size=16m', '--env', 'HOME=/tmp', '--env', 'TMPDIR=/tmp', '--env', 'BUN_RUNTIME_TRANSPILER_CACHE_PATH=0', ...[[bun.path, '/reviewed-bun'], [script, '/gateway.mjs'], [configuration, '/gateway.json']].flatMap(([source, target]) => ['--mount', `type=bind,source=${source},target=${target},readonly`]), '--entrypoint', '/reviewed-bun', image.id, '--no-env-file', '/gateway.mjs', '/gateway.json']);
  const ip = container.NetworkSettings.Networks[NETWORK].IPAddress; assert(privateIP(ip), 'private_gateway_ip_required');
  const health = 'try{const h=await fetch("http://127.0.0.1:8080/healthz",{signal:AbortSignal.timeout(1000)});if(h.status!==200)throw Error();const r=await fetch("http://127.0.0.1:8080/jmap/session",{headers:{Host:process.argv[1]},redirect:"manual",signal:AbortSignal.timeout(6000)});const s=Number(r.headers.get("x-webdock-fixture-upstream-status"));if(![200,302,401,404].includes(s))throw Error();process.stdout.write("gateway_health_and_upstream_http:"+s);}catch{process.stderr.write("fixture_gateway_health_failed");process.exitCode=1;}';
  await f.wait(async () => {
    assert((await f.inspect(container.Id)).State.Running, 'gateway_process_exited');
    const result = await f.docker(['exec', container.Id, '/reviewed-bun', '--no-env-file', '-e', health, 'c1.' + adminDomain], { allowFailure: true, timeout: 8000 });
    return result.status === 0 && /^gateway_health_and_upstream_http:(200|302|401|404)$/.test(result.output);
  }, 60000);
  return { id: container.Id, ip, origin: `http://${ip}:8080`, configuration, egress: [{ cidr: ip + '/32', port: 8080 }] };
}
async function chartHash(path) {
  const hash = createHash('sha256');
  const walk = async (directory, prefix = '') => { const files = []; for (const item of await readdir(directory, { withFileTypes: true })) { assert(!item.isSymbolicLink()); const name = prefix + item.name; if (item.isDirectory()) files.push(...await walk(join(directory, item.name), name + '/')); else if (item.isFile()) files.push(name); } return files; };
  const length = value => { const bytes = Buffer.alloc(8); bytes.writeBigUInt64BE(BigInt(value)); return bytes; };
  for (const name of (await walk(path)).sort()) { const bytes = await readFile(join(path, name)); hash.update(length(Buffer.byteLength(name))); hash.update(name); hash.update(length(bytes.length)); hash.update(bytes); } return hash.digest('hex');
}
export async function importArtifacts(f) {
  assert(f.report.completedStages.includes('prepare')); const node = await guardDev(f, true);
  const artifact = await f.json(f.path('artifacts.json'));
  for (const release of artifact.proof) {
    assert(['0.1.0', '0.2.0'].includes(release.version), 'unexpected_archive_version');
    const host = await open(release.archive, constants.O_RDONLY | constants.O_NOFOLLOW);
    let size, copyUID;
    try {
      const info = await host.stat(); assert(info.isFile() && info.nlink === 1 && info.uid === process.getuid() && (info.mode & 0o077) === 0, 'owned_archive_file_required');
      const bytes = await host.readFile(); size = bytes.length; copyUID = info.uid; assert.equal(sha256(bytes), release.archiveSha256, 'artifact_archive_changed');
    } finally { await host.close(); }
    const temporary = `/tmp/wd-mail-${f.input.runID}-${release.version}.tar`;
    const indexBytes = await readPrivate(f.path(`oci-${release.version}/index.json`)), index = JSON.parse(indexBytes);
    assert(index.manifests.length === 1 && index.manifests[0].digest === release.digest && index.manifests[0].annotations['org.opencontainers.image.ref.name'] === release.image, 'owned_archive_index_changed');
    const archiveIndexDigest = 'sha256:' + sha256(indexBytes);
    const aliases = (await f.docker(['exec', node.Id, 'ctr', '-n', 'k8s.io', 'images', 'list', '--quiet'])).output.split('\n');
    assert(!aliases.includes(release.image), 'image_alias_preexists');
    assert(await nodePathAbsent(f, node.Id, temporary), 'node_archive_path_preexists');
    const file = await custodyIntent(f.custody, f.saveCustody, 'node-file', temporary, { nodeID: node.Id, absentBeforeCopy: true, archiveSha256: release.archiveSha256, size, copyUID });
    const alias = await custodyIntent(f.custody, f.saveCustody, 'containerd-image', release.image, { nodeID: node.Id, digest: release.digest, configDigest: release.configDigest, archiveIndexDigest, preexistingReferences: aliases, additionalAliases: [] });
    await f.docker(['cp', release.archive, node.Id + ':' + temporary]);
    let identity = await nodeFileIdentity(f, node.Id, temporary);
    assert(identity.sha256 === file.archiveSha256 && identity.size === size && [0, copyUID].includes(identity.uid), 'node_archive_copy_changed');
    await custodyConfirm(file, f.saveCustody, { identity });
    if (identity.uid !== 0) {
      file.uidNormalization = { before: identity, targetUID: 0 }; await f.saveCustody();
      requirePrivateEqual(await nodeFileIdentity(f, node.Id, temporary), identity, 'node_archive_identity_changed');
      await f.docker(['exec', node.Id, 'chown', '-h', '0', '--', temporary]);
      const normalized = await nodeFileIdentity(f, node.Id, temporary);
      requirePrivateEqual(normalized, { ...identity, uid: 0 }, 'node_archive_identity_changed'); identity = normalized;
      await custodyConfirm(file, f.saveCustody, { identity });
    }
    requirePrivateEqual(await nodeFileIdentity(f, node.Id, temporary), identity, 'node_archive_identity_changed');
    await f.docker(['exec', node.Id, 'ctr', '-n', 'k8s.io', 'images', 'import', '--digests', temporary]);
    const afterRefs = (await f.docker(['exec', node.Id, 'ctr', '-n', 'k8s.io', 'images', 'list', '--quiet'])).output.split('\n').filter(Boolean);
    for (const name of afterRefs.filter(name => name !== release.image && !aliases.includes(name))) {
      if (!(name === release.configDigest || name.startsWith('import-') && name.endsWith('@' + archiveIndexDigest))) continue;
      const digest = await importedAliasDigest(f, node.Id, name);
      if (![release.digest, archiveIndexDigest].includes(digest)) continue;
      alias.additionalAliases.push({ name, digest, status: 'owned' }); await f.saveCustody();
    }
    const cri = JSON.parse((await f.docker(['exec', node.Id, 'crictl', 'inspecti', release.image])).output);
    assert(cri.status.repoDigests.includes(release.image) && cri.status.id === release.configDigest, 'imported_identity_mismatch');
    await custodyConfirm(alias, f.saveCustody, {}); await removeNodeArchive(f, file);
  }
}
async function importedAliasDigest(f, nodeID, name) {
  const rows = (await f.docker(['exec', nodeID, 'ctr', '-n', 'k8s.io', 'images', 'list', 'name==' + name])).output.split('\n').slice(1).map(line => line.trim().split(/\s+/)).filter(row => row[0] === name);
  assert(rows.length === 1 && /^sha256:[a-f0-9]{64}$/.test(rows[0][2]), 'imported_alias_identity_unavailable'); return rows[0][2];
}
export async function removeImportedAliases(f, item) {
  for (const extra of item.additionalAliases ?? []) {
    if (extra.status === 'removed') continue;
    assert(Array.isArray(item.preexistingReferences) && !item.preexistingReferences.includes(extra.name), 'shared_reference_cannot_be_removed');
    const names = (await f.docker(['exec', item.nodeID, 'ctr', '-n', 'k8s.io', 'images', 'list', '--quiet'])).output.split('\n');
    if (names.includes(extra.name)) {
      assert(await importedAliasDigest(f, item.nodeID, extra.name) === extra.digest, 'additional_alias_target_changed');
      await f.docker(['exec', item.nodeID, 'ctr', '-n', 'k8s.io', 'images', 'remove', extra.name]);
      assert(!(await f.docker(['exec', item.nodeID, 'ctr', '-n', 'k8s.io', 'images', 'list', '--quiet'])).output.split('\n').includes(extra.name), 'additional_alias_removal_unverified');
    }
    extra.status = 'removed'; await f.saveCustody();
  }
}
async function nodePathAbsent(f, nodeID, path) {
  const script = 'if [ -e "$1" ] || [ -L "$1" ]; then printf present; else printf absent; fi';
  const result = await f.docker(['exec', nodeID, 'sh', '-ec', script, 'sh', path]);
  assert(result.status === 0 && ['absent', 'present'].includes(result.output), 'node_archive_absence_unverified');
  return result.output === 'absent';
}
async function nodeFileIdentity(f, nodeID, path) {
  const script = 'test -f "$1"; test ! -L "$1"; stat -c "%d:%i:%s:%h:%u" -- "$1"; sha256sum -- "$1"';
  const result = await f.docker(['exec', nodeID, 'sh', '-ec', script, 'sh', path]);
  assert(result.status === 0, 'node_archive_regular_file_required');
  const lines = result.output.split('\n'), fields = /^(\d+):(\d+):(\d+):1:(\d+)$/.exec(lines[0]), hash = /^([a-f0-9]{64})\s+/.exec(lines[1] ?? '');
  assert(fields && hash && lines.length === 2, 'node_archive_identity_unavailable');
  return { device: fields[1], inode: fields[2], size: Number(fields[3]), uid: Number(fields[4]), sha256: hash[1] };
}
export async function removeNodeArchive(f, item) {
  assert(item.absentBeforeCopy && item.identity, 'node_archive_custody_incomplete');
  if (!await nodePathAbsent(f, item.nodeID, item.name)) {
    requirePrivateEqual(await nodeFileIdentity(f, item.nodeID, item.name), item.identity, 'node_archive_identity_changed');
    await f.docker(['exec', item.nodeID, 'rm', '--', item.name]);
    assert(await nodePathAbsent(f, item.nodeID, item.name), 'node_archive_removal_unverified');
  }
  item.status = 'removed'; await f.saveCustody();
}
async function start(f) {
  assert(f.report.completedStages.includes('import')); await guardDev(f, true);
  const env = await f.json(f.report.paths.coreEnvironment), kernel = f.report.paths.kernel;
  await f.command(f.binary('webdock-agent'), ['--config', f.report.paths.agent, 'validate']);
  const callback = await f.inspect(f.report.topology.callbackID), item = f.custody.entries.find(e => e.kind === 'container' && e.id === callback.Id); requireContainer(item, callback, f.input.runID);
  const allowed = new Set([...f.report.topology.callbackMounts.slice(0, -1), '/reviewed-callback']);
  assert(callback.Mounts.every(m => !m.RW && allowed.has(m.Destination)) && callback.Mounts.length === allowed.size, 'callback_mounts_changed');
  await f.docker(['exec', callback.Id, '/reviewed-callback', '--help']);
  const callbackEnv = await f.json(f.report.paths.callbackEnvironment);
  await f.startProcess('callback-exec', 'docker', ['--host', 'unix:///var/run/docker.sock', 'exec', ...Object.keys(callbackEnv).flatMap(key => ['--env', key]), callback.Id, '/reviewed-callback', '--config', join(kernel, 'plugin-config.json')], callbackEnv);
  await f.startProcess('core', f.binary('webdock-control-plane'), [], env);
  await f.startProcess('agent-control', f.binary('webdock-agent-control'), ['--config', f.report.paths.agentControl], env);
  await f.startProcess('controller', f.binary('webdock-plugin-controller'), ['--config', join(kernel, 'plugin-config.json'), '--installation-config', f.report.paths.worker], env);
  await f.startProcess('agent', f.binary('webdock-agent'), ['--config', f.report.paths.agent, 'run'], {});
  // Programmatic Vite with envDir:false prevents application .env discovery as well as Bun's loader.
  await f.startProcess('console', f.binary('bun'), ['--no-env-file', fileURLToPath(import.meta.url), 'serve-ui', '--input', f.report.paths.input], { WEBDOCK_DEV_API_ORIGIN: env.WEBDOCK_API_ORIGIN, WEBDOCK_FIXTURE_UI_PARENT: String(process.pid) });
  await f.wait(async () => { try { return (await fetch(f.report.origins.core + '/healthz', { signal: AbortSignal.timeout(2000), redirect: 'error' })).ok; } catch { return false; } });
  for (const origin of [f.report.origins.console, f.report.origins.account]) await f.wait(async () => { try { return (await fetch(origin, { signal: AbortSignal.timeout(2000), redirect: 'error' })).ok; } catch { return false; } });
  for (const item of f.custody.entries.filter(e => e.kind === 'process' && e.status === 'owned')) assert.deepEqual(processIdentity(item.pid), { startTime: item.startTime, commandSha256: item.commandSha256 }, 'fixture_process_exited_during_start');
}
async function serveUI(inputPath) {
  assert(Number(process.env.WEBDOCK_FIXTURE_UI_PARENT) === process.ppid, 'ui_requires_owned_stage_parent');
  const input = validateInput(JSON.parse(await readPrivate(inputPath))); stageAllowed(input, 'start');
  const report = JSON.parse(await readPrivate(join(input.directory, 'report.json')));
  assert(report.completedStages.includes('import') && report.runID === input.runID, 'ui_requires_prepared_fixture');
  const frozen = { ...input }; delete frozen.authorizedStages; assert.equal(report.inputSha256, sha256(JSON.stringify(frozen)));
  await verifyFreeze(new Fixture(input, report, {}));
  const { createServer } = await import('vite');
  const servers = [];
  for (const [name, host] of [['console', '127.0.0.1'], ['account', 'localhost']]) {
    const server = await createServer({ root: join(ROOT, 'apps/console'), configFile: join(ROOT, 'apps/console/vite.config.ts'), cacheDir: join(input.directory, 'vite-cache'), envDir: false, server: { host, port: input.ports[name], strictPort: true } });
    await server.listen(); servers.push(server);
  }
  const stop = async () => { for (const server of servers) await server.close(); process.exit(0); };
  process.on('SIGTERM', stop); process.on('SIGINT', stop);
}
export async function deleteOwnedObject(f, item) {
  const resource = await f.kube(['get', item.kind, item.name, '--ignore-not-found', '-o', 'json']);
  if (resource.output) {
    const object = JSON.parse(resource.output); requireObject(item, object);
    if (item.kind === 'namespace') {
      const claims = await f.kubeJSON(['-n', item.name, 'get', 'pvc']);
      claims.items.forEach(claim => { const owned = f.custody.entries.find(e => e.kind === 'persistentvolumeclaim' && e.namespace === item.name && e.name === claim.metadata.name); assert(owned, 'unowned_pvc_blocks_cleanup'); requireObject(owned, claim); });
    }
    if (item.kind === 'persistentvolume') assert.equal(object.spec.claimRef?.uid, item.claimUID, 'volume_claim_custody_changed');
    const endpoint = item.kind === 'namespace' ? '/api/v1/namespaces/' : '/api/v1/persistentvolumes/';
    await f.kube(['delete', '--raw', endpoint + item.name, '-f', '-'], { input: JSON.stringify({ apiVersion: 'v1', kind: 'DeleteOptions', preconditions: { uid: item.uid, resourceVersion: object.metadata.resourceVersion } }) });
    await f.wait(async () => !(await f.kube(['get', item.kind, item.name, '--ignore-not-found', '-o', 'name'])).output);
  }
  item.status = 'removed'; await f.saveCustody();
}
export async function cleanup(f) {
  // No discovery-based deletion. An uncertain creation without a returned identity blocks cleanup.
  assert(f.custody.entries.every(e => e.status !== 'intent'), 'uncertain_custody_requires_root_reconciliation');
  for (const item of [...f.custody.entries].reverse().filter(e => e.kind === 'process' && e.status !== 'removed')) {
    if (existsSync(`/proc/${item.pid}`)) {
      assert.deepEqual(processIdentity(item.pid), { startTime: item.startTime, commandSha256: item.commandSha256 }, 'process_custody_mismatch');
      if (item.executableSha256) {
        const actual = await observedProcess(item.pid);
        assert(actual.executable === item.executable && actual.executableSha256 === item.executableSha256 && actual.uid === process.getuid(), 'process_executable_changed');
      }
      process.kill(-item.pid, 'SIGTERM');
      await f.wait(() => !existsSync(`/proc/${item.pid}`), 30000);
    }
    item.status = 'removed'; await f.saveCustody();
  }
  const node = await guardDev(f, false);
  if (f.report.topology?.callbackID) {
    const item = f.custody.entries.find(e => e.kind === 'container' && e.id === f.report.topology.callbackID);
    assert(item);
    if (item.status !== 'removed') {
      const result = await f.docker(['inspect', '--format', OWNED_INSPECT, item.id], { allowFailure: true });
      if (result.status !== 0) {
        requireDockerMissing(result, 'container', item.id); await verifyVolumesAbsent(f, item);
        item.status = 'removed'; await f.saveCustody();
      } else {
        requireContainer(item, JSON.parse(result.output), f.input.runID);
        // Killing the Docker exec client alone does not terminate its process inside the container.
        await f.docker(['stop', item.id]);
      }
    }
  }
  for (const item of f.custody.entries.filter(e => e.kind === 'namespace-intent' && e.status !== 'removed')) {
    const result = await f.kube(['get', 'namespace', item.name, '--ignore-not-found', '-o', 'json']);
    if (result.output) {
      const actual = JSON.parse(result.output), owned = f.custody.entries.find(e => e.kind === 'namespace' && e.name === item.name);
      assert(owned, 'unobserved_namespace_requires_root_reconciliation'); requireObject(owned, actual);
    }
  }
  const objects = f.custody.entries.filter(e => ['namespace', 'persistentvolume'].includes(e.kind) && e.status !== 'removed').sort((a, b) => {
    const rank = item => item.kind === 'persistentvolume' ? 2 : item.name.startsWith('wd-plugin-') ? 0 : 1;
    return rank(a) - rank(b);
  });
  for (const item of objects) await deleteOwnedObject(f, item);
  for (const item of [...f.custody.entries].reverse().filter(e => e.status !== 'removed')) {
    if (item.kind === 'container') {
      const result = await f.docker(['inspect', '--format', OWNED_INSPECT, item.id], { allowFailure: true });
      if (result.status !== 0) requireDockerMissing(result, 'container', item.id);
      else { requireContainer(item, JSON.parse(result.output), f.input.runID); await f.docker(['rm', '--force', '--volumes', item.id]); }
      await verifyVolumesAbsent(f, item);
    } else if (item.kind === 'containerd-image') {
      assert.equal(item.nodeID, node.Id); await removeImportedAliases(f, item);
      const current = await f.docker(['exec', node.Id, 'crictl', 'inspecti', item.name], { allowFailure: true });
      if (current.status === 0) { const cri = JSON.parse(current.output); assert(cri.status.repoDigests.includes(item.name) && cri.status.id === item.configDigest); await f.docker(['exec', node.Id, 'ctr', '-n', 'k8s.io', 'images', 'remove', item.name]); }
      else {
        assert(/not found|no such image/i.test(current.stderr), 'image_absence_unverified');
        assert(!(await f.docker(['exec', node.Id, 'ctr', '-n', 'k8s.io', 'images', 'list', '--quiet'])).output.split('\n').includes(item.name), 'canonical_reference_still_present');
      }
    } else if (item.kind === 'docker-image') {
      assert(item.absentBeforeCreate, 'image_custody_incomplete');
      const result = await f.docker(['image', 'inspect', '--format', '{{json .Id}}', item.name], { allowFailure: true });
      if (result.status === 0) { assert.equal(JSON.parse(result.output), item.id); await f.docker(['image', 'rm', item.name]); }
      else requireDockerMissing(result, 'image', item.name);
    } else if (item.kind === 'directory') {
      const s = await lstat(item.name); assert(s.ino === item.inode && s.dev === item.device && s.uid === process.getuid());
      await rm(item.name, { recursive: true });
    } else if (item.kind === 'node-file') { assert.equal(item.nodeID, node.Id); await removeNodeArchive(f, item); }
    else if (!['persistentvolumeclaim', 'api-request', 'namespace-intent'].includes(item.kind)) throw Error('unknown_cleanup_custody');
    item.status = 'removed'; await f.saveCustody();
  }
  if (f.custody.retainedK3s.initialState === 'exited' && f.custody.retainedK3s.startIntent) {
    assert.equal(node.Id, f.custody.retainedK3s.id);
    if (node.State.Running) await f.docker(['stop', node.Id]);
    assert.equal((await f.inspectRetained(node.Id)).State.Status, 'exited');
  }
  await scrubOwnedFiles(f);
  f.report.cleanup = { verified: true, retainedK3sRestored: true, privateMaterialRetained: false };
}
async function verifyVolumesAbsent(f, item) {
  for (const volume of item.volumes) {
    const result = await f.docker(['volume', 'inspect', volume], { allowFailure: true });
    assert(result.status !== 0 && /no such volume|not found/i.test(result.stderr), 'owned_volume_removal_unverified');
  }
}
async function scrubOwnedFiles(f) {
  const allowed = new Set(['logs', 'home', 'tmp', 'vite-cache', 'agent-state', 'platform', 'metadata', 'kernel', 'release-context', 'oci-0.1.0', 'oci-0.2.0', 'mail-0.1.0.oci.tar', 'mail-0.2.0.oci.tar', 'artifacts.json', 'release-signing.key', 'storage-platform.json', 'storage-metadata.json', 'roles.json', 'operator-input.json', 'operator-output.json', 'enrollment-input.json', 'enrollment.json', 'kernel-input.json', 'catalog.json', 'worker.json', 'agent-control.json', 'agent.json', 'core-environment.json', 'callback-environment.json', 'mail-gateway.json', 'acceptance-state.json']);
  const keep = new Set([f.path('active.lock'), f.path('custody.json'), f.path('report.json'), f.path('input-snapshot.json'), f.report.paths.input]);
  for (const name of await readdir(f.directory)) {
    const path = f.path(name); if (keep.has(path)) continue;
    assert(allowed.has(name), 'unknown_private_file_requires_root_review');
    const before = await lstat(path); assert(before.uid === process.getuid() && !before.isSymbolicLink(), 'private_cleanup_identity_invalid');
    const item = await custodyIntent(f.custody, f.saveCustody, 'private-file', path, { inode: before.ino, device: before.dev });
    const current = await lstat(path); assert(current.ino === item.inode && current.dev === item.device, 'private_cleanup_identity_changed');
    await rm(path, { recursive: before.isDirectory() }); item.status = 'removed'; await f.saveCustody();
  }
}
export async function main(args, loadFixture = openFixture) {
  const [stage, flag, inputPath] = args; assert(flag === '--input' && isAbsolute(inputPath ?? '') && args.length === 3, 'fixture_cli_shape');
  if (stage === 'serve-ui') return serveUI(inputPath);
  const f = await loadFixture(inputPath, stage);
  try {
    assert(stage !== 'acceptance', 'use_mail_browser_entrypoint');
    assert(!f.report.completedStages.includes(stage), 'stage_already_completed');
    if (stage !== 'cleanup') await verifyFreeze(f);
    if (stage === 'preflight') await preflight(f);
    else {
      assert(stage === 'cleanup' || f.report.completedStages.includes('preflight'), 'preflight_stage_required');
      if (stage === 'artifacts') {
        await guardDev(f, false);
        await (await import('./native-plugin-artifacts.mjs')).buildArtifacts(f);
      }
      else if (stage === 'prepare') await prepare(f);
      else if (stage === 'import') await importArtifacts(f);
      else if (stage === 'start') await start(f);
      else if (stage === 'observe') await (await import('./native-mail-proof.mjs')).observe(f);
      else if (stage === 'cleanup') await cleanup(f);
    }
    f.report.completedStages.push(stage); await f.saveReport();
    process.stdout.write('native_mail_stage_complete:' + stage + '\n');
  } finally { await f.release(); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(() => { process.stderr.write('native_mail_fixture_failed; retain private custody for root review\n'); process.exitCode = 1; });
}
