import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, chmod, symlink, link, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { generateKeyPairSync, verify } from 'node:crypto';
import { validateInput, stageAllowed, ownEnvironment, shellQuote, redact, readPrivate, writePrivate, writePinnedPrivate, custodyIntent, custodyConfirm, requireContainer, requireObject, sha256, Fixture, BASE, ROOT, BINARY_NAMES, requireDevContext, runtimeNamespace, frozenInput, requirePrivateEqual, deleteOwnedObject, guardDev, K3S, NETWORK } from './native-plugin-fixture.mjs';
import { signedRelease, imageIdentity, verifyRelease, ownImage } from './native-plugin-artifacts.mjs';
import { importArtifacts, cleanup, main, fixtureOrigins, stageBuildx, settleProcessIdentity, prepareMailGateway, removeImportedAliases } from './native-plugin-fixture.mjs';
import { observe, engineObjects } from './native-mail-proof.mjs';
import { acceptance, mailboxCreateForm } from '../../apps/console/tests/mail-installation-native.mjs';
import { waitMailReady, finishAccountNavigation, assertForeignDenied } from '../../apps/console/tests/support/native-plugin-lifecycle.mjs';
import { gatewayHandler, validateGatewayConfig } from './native-mail-gateway.mjs';

const gatewayConfig = () => ({ version: 1, runID: 'abcdef123456', adminDomain: 'mail-admin-abcdef123456.localhost', upstream: 'http://172.18.0.8:80', maxBodyBytes: 64, timeoutMs: 30 });
test('foreign denial accepts only secure403/404 error envelopes with no resource or credential payload', () => {
  const body = { error: { code: 'permission_denied', message: 'Access denied' } };
  assertForeignDenied(403, body); assertForeignDenied(404, body);
  for (const status of [200,201,204,302,400,401,409,500]) assert.throws(() => assertForeignDenied(status, body));
  for (const unsafe of [null, [], { ...body, data: { resource: 'owned' } }, Object.assign(Object.create(body), { resource: 'owned' }), { ...body, value: 'PRIVATE_SENTINEL' }, { error: { ...body.error, password: 'PRIVATE_SENTINEL' } }, { error: { code: 'denied', message: 'PRIVATE_SENTINEL' } }]) assert.throws(() => assertForeignDenied(403, unsafe, ['PRIVATE_SENTINEL']));
});
async function coldApiCase(readyEventually, run) {
  const directory = await mkdtemp(join(tmpdir(), 'mail-cold-api-'));
  try {
    const f = new Fixture({ directory, runID: 'abcdef123456', kubeconfig: '/not-read/dev.yaml', review: { sourceFiles: {}, binaries: {} } }, { completedStages: ['preflight','artifacts'] }, { entries: [], retainedK3s: { id: 'node' } });
    const events = []; let attempts = 0, ready = false;
    f.release = async () => {}; f.saveCustody = async () => {};
    f.command = async (program,args) => { if (program === 'openssl') { events.push('pg-setup'); throw Error('STOP_BEFORE_PG'); } return { output: args.includes('current-context') ? 'webdock-dev' : 'https://127.0.0.1:56443' }; };
    f.inspectRetained = async () => ({ Id: 'node', State: { Running: true }, NetworkSettings: { Networks: { [NETWORK]: {} } } });
    f.kube = async (args,options) => {
      if (args.includes('config')) return { output: 'https://127.0.0.1:56443' };
      if (args.includes('/readyz')) { assert.equal(options.allowFailure,true); assert(options.timeout<=5000); attempts++; ready=readyEventually&&attempts>=2; events.push(ready?'readyz-ok':'readyz-503'); return { status: ready?0:1, output: ready?'ok':'' }; }
      if (args[0] === 'wait') { events.push('node-wait'); if (!ready) throw Error('cold_api_503'); }
      return { status: 0, output: '' };
    };
    f.kubeJSON = async args => args.includes('storageclasses') ? { items: [{ metadata: { annotations: { 'storageclass.kubernetes.io/is-default-class': 'true' } } }] } : { items: [] };
    f.wait = async (check,deadline) => { assert(deadline<=120000); for(let n=0;n<3;n++)if(await check())return;throw Error('fixture_deadline_exceeded'); };
    await run(f,events);
  } finally { await rm(directory,{recursive:true,force:true}); }
}
test('cold API503 is polled readonly before node wait and any PostgreSQL/bootstrap work', async () => {
  await coldApiCase(true,async(f,events)=>{await assert.rejects(main(['prepare','--input',join(f.directory,'input.json')],async()=>f),/STOP_BEFORE_PG/);assert.deepEqual(events,['readyz-503','readyz-ok','node-wait','pg-setup']);});
});
test('cold API readiness deadline blocks all node wait and database/bootstrap mutation', async () => {
  await coldApiCase(false,async(f,events)=>{await assert.rejects(main(['prepare','--input',join(f.directory,'input.json')],async()=>f),/fixture_deadline_exceeded/);assert.deepEqual(events,['readyz-503','readyz-503','readyz-503']);});
});
test('gateway validates one private node upstream and the exact run admin domain', () => {
  validateGatewayConfig(gatewayConfig());
  for (const upstream of ['http://example.com', 'http://127.0.0.1', 'https://172.18.0.8', 'http://u:p@172.18.0.8', 'http://172.18.0.8:81', 'http://172.18.0.8/path', 'http://172.18.0.8?x=1']) assert.throws(() => validateGatewayConfig({ ...gatewayConfig(), upstream }));
  assert.throws(() => validateGatewayConfig({ ...gatewayConfig(), adminDomain: 'other.localhost' }));
});
test('gateway health never reaches upstream and malformed customer Hosts cannot forward', async () => {
  let calls = 0; const handle = gatewayHandler(gatewayConfig(), async () => { calls++; throw Error('must_not_forward'); });
  assert.equal((await handle(new Request('http://gateway/healthz'))).status, 200);
  for (const host of ['c0.mail-admin-abcdef123456.localhost', 'c01.mail-admin-abcdef123456.localhost', 'c1.mail-admin-abcdef123456.localhost:8080', 'c1.mail-admin-abcdef123456.localhost.', 'c1.mail-admin-abcdef123456.localhost,evil.test', 'c1.mail-admin-000000000000.localhost']) assert.equal((await handle(new Request('http://gateway/jmap/session', { headers: { host } }))).status, 403);
  assert.equal(calls, 0);
});
test('gateway forwards only fixed paths and customer Host to its fixed upstream without redirects', async () => {
  const seen = [];
  const handle = gatewayHandler(gatewayConfig(), async (url, options) => { seen.push({ url, options }); return new Response('{}', { status: 200, headers: { 'content-type': 'application/json', 'set-cookie': 'private-do-not-forward' } }); });
  const response = await handle(new Request('http://gateway/jmap', { method: 'POST', headers: { host: 'c123.mail-admin-abcdef123456.localhost', authorization: 'Basic PRIVATE_SENTINEL', 'content-type': 'application/json' }, body: '{}' }));
  assert.equal(response.status, 200); assert.equal(seen.length, 1); assert.equal(seen[0].url, 'http://172.18.0.8/jmap'); assert.equal(seen[0].options.redirect, 'manual');
  assert.equal(seen[0].options.headers.get('host'), 'c123.mail-admin-abcdef123456.localhost'); assert.equal(response.headers.get('set-cookie'), null);
  for (const [url, method, host] of [['http://gateway/jmap', 'GET', 'evil.test'], ['http://gateway/other', 'GET', 'c123.mail-admin-abcdef123456.localhost'], ['http://gateway/jmap?upstream=evil', 'GET', 'c123.mail-admin-abcdef123456.localhost'], ['http://gateway/jmap', 'DELETE', 'c123.mail-admin-abcdef123456.localhost']]) {
    assert((await handle(new Request(url, { method, headers: { host } }))).status >= 400);
  }
  assert.equal(seen.length, 1);
});
test('gateway bounds streamed bodies and response bodies and exposes no credentials on failures', async () => {
  let calls = 0;
  const handle = gatewayHandler(gatewayConfig(), async () => { calls++; return new Response('x'.repeat(65)); });
  const headers = { host: 'c123.mail-admin-abcdef123456.localhost', 'content-type': 'application/json', 'content-length': '1', authorization: 'PRIVATE_SENTINEL' };
  assert.equal((await handle(new Request('http://gateway/jmap', { method: 'POST', headers, body: 'x'.repeat(65) }))).status, 413); assert.equal(calls, 0);
  const response = await handle(new Request('http://gateway/jmap/session', { headers: { host: headers.host } })); assert.equal(response.status, 502); assert.equal((await response.text()).includes('PRIVATE_SENTINEL'), false);
  const throwing = gatewayHandler(gatewayConfig(), async () => { throw Error('PRIVATE_SENTINEL upstream credentials'); });
  assert.equal((await (await throwing(new Request('http://gateway/jmap', { method: 'POST', headers, body: '{}' }))).text()).includes('PRIVATE_SENTINEL'), false);
});
test('gateway rejects upstream redirects and has a bounded deadline without credential logging', async () => {
  let calls = 0;
  const handle = gatewayHandler(gatewayConfig(), async () => { calls++; return new Response(null, { status: 302, headers: { location: 'https://arbitrary.invalid/PRIVATE_SENTINEL' } }); });
  const req = () => new Request('http://gateway/jmap/session', { headers: { host: 'c123.mail-admin-abcdef123456.localhost' } });
  const response = await handle(req()); assert.equal(response.status, 502); assert.equal(response.headers.get('location'), null); assert.equal(calls, 1); assert.equal((await response.text()).includes('PRIVATE_SENTINEL'), false);
  const hanging = gatewayHandler({ ...gatewayConfig(), timeoutMs: 5 }, () => new Promise(() => {})); assert.equal((await hanging(req())).status, 504);
});
test('process custody waits through fork/exec until reviewed executable and exact argv settle', async () => {
  const expected = { startTime: '10', executable: '/reviewed/agent', executableSha256: 'a'.repeat(64), argv: ['/reviewed/agent', '--config', '/owned/agent.json'], uid: 1000 };
  const observations = [{ ...expected, executable: '/parent/bun', executableSha256: 'b'.repeat(64), argv: ['bun', 'parent'] }, { ...expected }];
  let reads = 0;
  const actual = await settleProcessIdentity(123, expected, async () => { reads++; return observations.shift(); }, async () => {});
  assert.equal(reads, 2); assert.equal(actual.executableSha256, expected.executableSha256);
  await assert.rejects(settleProcessIdentity(123, expected, async () => ({ ...expected, startTime: '11' }), async () => {}), /process_identity_changed/);
  await assert.rejects(settleProcessIdentity(123, expected, async () => ({ ...expected, argv: ['different'] }), async () => {}, 2), /process_exec_not_settled/);
});
test('Buildx stages only verified public ELF into private HOME before image intents', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'mail-buildx-')); const { mkdir, readFile } = await import('node:fs/promises');
  try {
    const source = join(dir, 'reviewed-buildx'), bytes = Buffer.from('\x7fELFpublic-test-tool'); await writePrivate(source, bytes); await chmod(source, 0o500); await mkdir(join(dir, 'home'), { mode: 0o700 });
    const f = new Fixture({ directory: dir, review: { binaries: { 'docker-buildx': { path: source, sha256: sha256(bytes) } } } }, {}, { entries: [] });
    f.docker = async args => { assert.deepEqual(args, ['buildx', 'version']); assert.equal(f.custody.entries.length, 0); return { status: 0, output: 'github.com/docker/buildx fixture' }; };
    await stageBuildx(f); assert.deepEqual(await readFile(join(dir, 'home/.docker/cli-plugins/docker-buildx')), bytes);
    const config = join(dir, 'home/.docker/config.json'); await writePrivate(config, 'PRIVATE_CONFIG_SENTINEL'); await assert.rejects(stageBuildx(f), /private_docker_config_present/);
    assert.equal(await readPrivate(config), 'PRIVATE_CONFIG_SENTINEL');
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('Buildx refuses wrong ELF hashes and dangling staged plugins without executing tools', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'mail-buildx-refuse-')); const { mkdir } = await import('node:fs/promises');
  try {
    const source = join(dir, 'buildx'), bytes = Buffer.from('\x7fELFowned'); await writePrivate(source, bytes); await chmod(source, 0o500);
    const f = new Fixture({ directory: dir, review: { binaries: { 'docker-buildx': { path: source, sha256: '0'.repeat(64) } } } }, {}, { entries: [] });
    f.docker = async () => { throw Error('must_not_execute'); };
    await assert.rejects(stageBuildx(f), /reviewed_elf_hash_changed/);
    f.input.review.binaries['docker-buildx'].sha256 = sha256(bytes);
    await mkdir(join(dir, 'home/.docker/cli-plugins'), { recursive: true, mode: 0o700 });
    const target = join(dir, 'home/.docker/cli-plugins/docker-buildx'); await symlink(join(dir, 'missing'), target);
    await assert.rejects(stageBuildx(f)); assert((await stat(source)).isFile());
    const { lstat } = await import('node:fs/promises'); assert((await lstat(target)).isSymbolicLink());
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('fixture gateway uses three readonly mounts and one private egress destination', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'mail-gateway-profile-')); const { readFile } = await import('node:fs/promises');
  try {
    const bytes = Buffer.from('\x7fELFreviewed-bun'), bun = join(dir, 'bun'); await writePrivate(bun, bytes); await chmod(bun, 0o500);
    const source = 'tools/tests/native-mail-gateway.mjs';
    const f = new Fixture({ directory: dir, runID: 'abcdef123456', review: { binaries: { bun: { path: bun, sha256: sha256(bytes) } }, sourceFiles: { [source]: sha256(await readFile(join(ROOT, source))) } } }, {}, { entries: [{ kind: 'docker-image', name: 'owned-base', id: 'sha256:' + 'a'.repeat(64), absentBeforeCreate: true }] });
    let creation;
    f.ownContainer = async (name, args) => { creation = { name, args }; return { Id: 'gateway', NetworkSettings: { Networks: { [NETWORK]: { IPAddress: '172.18.0.5' } } } }; };
    f.inspect = async () => ({ State: { Running: true } });
    f.docker = async args => args[0] === 'image' ? { status: 0, output: JSON.stringify('sha256:' + 'a'.repeat(64)) } : { status: 0, output: 'gateway_health_and_upstream_http:404' };
    f.wait = async check => assert(await check());
    const result = await prepareMailGateway(f, '172.18.0.8', 'owned-base', 'mail-admin-abcdef123456.localhost');
    assert.equal(result.origin, 'http://172.18.0.5:8080'); assert.deepEqual(result.egress, [{ cidr: '172.18.0.5/32', port: 8080 }]);
    assert.equal(creation.args.filter(v => v === '--mount').length, 3); assert.equal(creation.args.includes('--publish'), false); assert(creation.args.includes('--read-only'));
    assert.equal(creation.args.some(v => /ca\.key|kubeconfig|environment|storage-/.test(v)), false);
    assert.equal(JSON.parse(await readPrivate(result.configuration)).upstream, 'http://172.18.0.8:80');
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('account continuation accepts automatic Console redirect or one real consent click', async () => {
  const f = { wait: async check => { for (let n = 0; n < 3; n++) if (await check()) return; throw Error('deadline'); } };
  await finishAccountNavigation(f, { url: () => 'http://127.0.0.1:6161/extensions', getByRole: () => { throw Error('consent must not be required'); } }, 'http://127.0.0.1:6161');
  let ready = false, clicks = 0;
  await finishAccountNavigation(f, { url: () => ready ? 'http://127.0.0.1:6161/extensions' : 'http://localhost:6165/consent', getByRole: () => ({ isVisible: async () => true, click: async () => { clicks++; ready = true; } }) }, 'http://127.0.0.1:6161');
  assert.equal(clicks, 1);
});
test('mailbox native form uses its feature-label child and field names, not bare suffixed labels', async () => {
  const seen = [], field = name => ({ selectOption: async value => seen.push([name, value]), fill: async value => seen.push([name, value]) });
  const form = { locator: name => field(name) };
  const menu = { getByRole: () => ({ filter: ({ has }) => ({ click: async () => seen.push(['feature', has.text]) }) }) };
  const page = { getByText: text => ({ text, click: async () => seen.push(['text', text]) }), getByRole: role => role === 'form' ? form : menu };
  assert.equal(await mailboxCreateForm(page, 'owned-domain'), form);
  assert.deepEqual(seen, [['text', 'Add'], ['feature', 'Create mailboxes'], ['select', 'owned-domain'], ['input[name="local_part"]', 'anna'], ['input[name="name"]', 'Owned mailbox']]);
});

test('stages are explicit, Mail-only and cannot authorize an implicit next stage', () => {
  assert.equal(stageAllowed({ authorizedStages: ['preflight'] }, 'preflight'), true);
  for (const stage of ['start', 'build', '', 'cleanup']) assert.throws(() => stageAllowed({ authorizedStages: ['preflight'] }, stage));
});
test('bootstrap origins are distinct while Studio shares the existing Console port', () => {
  const origins = fixtureOrigins({ core: 6130, console: 6121, account: 6125, callback: 6140 });
  assert.equal(new Set([origins.account, origins.studio, origins.console]).size, 3);
  assert.equal(origins.console, 'http://127.0.0.1:6121');
  assert.equal(origins.studio, 'http://localhost:6121');
  assert.equal(origins.account, 'http://localhost:6125');
  assert.equal(new URL(origins.studio).port, new URL(origins.console).port);
  for (const origin of Object.values(origins)) assert.equal(new URL(origin).origin, origin);
});
test('review1 retained-node command output is filtered before it can be logged', async () => {
  const f = new Fixture({ kubeconfig: '/not-read/dev.yaml' }, {}, { entries: [], retainedK3s: null });
  const commands = [];
  f.saveCustody = async () => {};
  f.command = async (_program, args) => ({ output: args.includes('current-context') ? 'webdock-dev' : 'https://127.0.0.1:56443' });
  f.docker = async args => {
    commands.push(args);
    assert.equal(args[0], 'inspect'); assert.equal(args[1], '--format', 'full retained inspect must never reach the logger');
    const format = args[2];
    for (const forbidden of ['.Env', '.Config', '.Path', '.Args', '{{json .}}']) assert.equal(format.includes(forbidden), false);
    assert.equal(args[3], K3S);
    return { output: JSON.stringify({ Id: 'retained-id', State: { Status: 'exited', Running: false }, NetworkSettings: { Networks: { [NETWORK]: { IPAddress: '' } } }, Mounts: [] }) };
  };
  const node = await guardDev(f);
  assert.equal(node.Id, 'retained-id'); assert.equal(commands.length, 1);
  assert.equal(JSON.stringify(f.custody).includes('Env'), false);
});
test('bind mounts without Name use optional lookup in both filtered projections', async () => {
  const f = new Fixture({}, {}, {}), bind = { Type: 'bind', Source: '/owned/source', Destination: '/data', RW: true };
  f.docker = async args => {
    assert.equal(args[0], 'inspect'); assert.equal(args[1], '--format');
    assert.equal(args[2].includes('$m.Name'), false, 'bind mounts omit Name in Docker template maps');
    assert(args[2].includes('json (index $m "Name")'));
    return { output: JSON.stringify({ Mounts: [{ ...bind, Name: bind.Name ?? null }] }) };
  };
  for (const value of [await f.inspectRetained(), await f.inspect('owned')]) {
    assert.equal(value.Mounts[0].Type, 'bind'); assert.equal(value.Mounts[0].Name, null);
  }
});
async function artifactDispatchCase(change, run) {
  const directory = await mkdtemp(join(tmpdir(), 'mail-artifact-context-'));
  try {
    const f = new Fixture({ directory, runID: 'abcdef123456', kubeconfig: '/not-read/dev.yaml', allowStartRetainedK3s: false, review: { sourceFiles: {}, binaries: {} } }, { completedStages: [] }, { entries: [], retainedK3s: null });
    const tool = join(directory, 'buildx'); const bytes = Buffer.from('\x7fELFtest-public-buildx'); await writePrivate(tool, bytes); await chmod(tool, 0o500); f.input.review.binaries['docker-buildx'] = { path: tool, sha256: sha256(bytes) };
    const state = { context: 'webdock-dev', endpoint: 'https://127.0.0.1:56443', nodeID: 'retained-node', imageChecks: 0, mutations: [], released: false };
    f.saveCustody = async () => {}; f.saveReport = async () => {}; f.release = async () => { state.released = true; };
    f.command = async (_program, args) => ({ output: args.includes('current-context') ? state.context : state.endpoint });
    f.inspectRetained = async () => ({ Id: state.nodeID, State: { Running: false, Status: 'exited' }, NetworkSettings: { Networks: { [NETWORK]: {} } }, Mounts: [] });
    f.docker = async args => {
      if (args[0] === 'buildx') return { status: 0, output: 'github.com/docker/buildx fixture' };
      if (args[0] === 'image' && args[1] === 'inspect') { state.imageChecks++; return { status: 1, stderr: 'Error: No such image: ' + args.at(-1) }; }
      state.mutations.push(args[0]); throw Error('STOP_BEFORE_ARTIFACT_MUTATION');
    };
    await guardDev(f, false); f.report.completedStages.push('preflight');
    assert.equal(f.custody.retainedK3s.id, 'retained-node');
    if (change === 'context') state.context = 'foreign-context';
    if (change === 'endpoint') state.endpoint = 'https://127.0.0.1:6443';
    if (change === 'identity') state.nodeID = 'replacement-node';
    await run(f, state);
  } finally { await rm(directory, { recursive: true, force: true }); }
}
for (const [change, category] of [['context', /dev_context_required/], ['endpoint', /dev_loopback_api_required/], ['identity', /retained_node_identity_changed/]]) {
  test('fix2 artifact dispatch rejects changed ' + change + ' after successful preflight', async () => {
    await artifactDispatchCase(change, async (f, state) => {
      await assert.rejects(main(['artifacts', '--input', join(f.directory, 'input.json')], async () => f), category);
      assert.equal(state.imageChecks, 0); assert.deepEqual(state.mutations, []); assert(state.released);
    });
  });
}
test('fix2 artifact dispatch accepts the unchanged stopped retained node without starting it', async () => {
  await artifactDispatchCase('unchanged', async (f, state) => {
    await assert.rejects(main(['artifacts', '--input', join(f.directory, 'input.json')], async () => f), /STOP_BEFORE_ARTIFACT_MUTATION/);
    assert.equal(state.imageChecks, 1); assert.deepEqual(state.mutations, ['build']);
    assert.equal(f.custody.retainedK3s.started, false); assert(state.released);
  });
});
test('review2 image creation refuses preexisting tags and daemon uncertainty before build', async () => {
  for (const result of [{ status: 0, output: JSON.stringify([{ Id: 'foreign' }]) }, { status: 1, stderr: 'Cannot connect to the Docker daemon' }]) {
    let built = false;
    const f = { custody: { entries: [] }, saveCustody: async () => {}, docker: async () => result };
    await assert.rejects(ownImage(f, 'owned:tag', async () => { built = true; }));
    assert.equal(built, false); assert.deepEqual(f.custody.entries, []);
  }
});
test('review2 image absence is durably recorded before a new tag is built', async () => {
  let built = false;
  const f = { custody: { entries: [] }, saveCustody: async () => {}, docker: async () => built ? { status: 0, output: JSON.stringify('sha256:' + 'a'.repeat(64)) } : { status: 1, stderr: 'Error: No such image: owned:tag' } };
  await ownImage(f, 'owned:tag', async () => {
    assert.equal(f.custody.entries[0].absentBeforeCreate, true); built = true;
  });
  assert.equal(f.custody.entries[0].id, 'sha256:' + 'a'.repeat(64));
});
async function nodeImportCase(mode, run) {
  const directory = await mkdtemp(join(tmpdir(), 'mail-node-custody-'));
  try {
    const archive = join(directory, 'archive.tar'); await writePrivate(archive, 'owned archive');
    const release = { version: '0.1.0', archive, archiveSha256: sha256('owned archive'), image: 'fixture.invalid/mail@sha256:' + 'a'.repeat(64), digest: 'sha256:' + 'a'.repeat(64), configDigest: 'sha256:' + 'b'.repeat(64) };
    const f = new Fixture({ directory, runID: 'abcdef123456', kubeconfig: '/not-read/dev.yaml' }, { completedStages: ['prepare'] }, { entries: [], retainedK3s: { id: 'node' } });
    const { mkdir } = await import('node:fs/promises'); await mkdir(join(directory, 'oci-0.1.0'), { mode: 0o700 });
    const index = { manifests: [{ digest: release.digest, annotations: { 'org.opencontainers.image.ref.name': release.image } }] };
    const indexRaw = JSON.stringify(index); await writePrivate(join(directory, 'oci-0.1.0/index.json'), indexRaw); const indexDigest = 'sha256:' + sha256(indexRaw);
    f.saveCustody = async () => {}; f.inspectRetained = async () => ({ Id: 'node', State: { Running: true }, NetworkSettings: { Networks: { [NETWORK]: {} } } });
    f.command = async (_program, args) => ({ output: args.includes('current-context') ? 'webdock-dev' : 'https://127.0.0.1:56443' });
    f.json = async () => ({ proof: [release] });
    const calls = [], state = { present: mode === 'present' || mode === 'dangling', hashReads: 0, imported: false, removed: false, normalized: false, chowns: 0 };
    f.docker = async args => {
      calls.push(args);
      if (args.includes('list') && args.includes('--quiet')) return { status: 0, output: ['preexisting-shared', ...(state.imported ? [release.image, release.configDigest, 'import-owned@' + indexDigest] : [])].join('\n') };
      if (args.includes('list') && args.at(-1).startsWith('name==')) { const name = args.at(-1).slice(6); return { status: 0, output: 'REF TYPE DIGEST\n' + name + ' application/manifest ' + (name.startsWith('import-') ? indexDigest : release.digest) }; }
      if (args[0] === 'cp') {
        assert.equal(state.present, false, 'copy would overwrite an unowned node path');
        assert.equal(f.custody.entries.find(e => e.kind === 'node-file').absentBeforeCopy, true); state.present = true;
      } else if (args.includes('sha256sum')) throw Error('identity_script_expected');
      else if (args.includes('sh')) {
        const script = args[args.indexOf('-ec') + 1];
        if (script.includes('sha256sum')) {
          state.hashReads++;
          if (mode === 'replaced-symlink') return { status: 1, stderr: 'not regular' };
          const hash = mode === 'tampered-after-import' && state.imported ? 'c'.repeat(64) : release.archiveSha256;
          const uid = mode === 'foreign-uid' ? process.getuid() + 123 : ['runner-uid', 'replaced-after-chown'].includes(mode) && !state.normalized ? process.getuid() : 0;
          const inode = mode === 'replaced-after-chown' && state.normalized ? '3' : '2';
          return { status: 0, output: '1:' + inode + ':13:1:' + uid + '\n' + hash + '  ' + args.at(-1) };
        }
        return { status: 0, output: state.present ? 'present' : 'absent' };
      } else if (args.includes('chown')) { state.chowns++; state.normalized = true; }
      else if (args.includes('import')) {
        assert(state.hashReads > 0, 'import must verify a regular owned file first'); state.imported = true;
      } else if (args.includes('inspecti')) return { status: 0, output: JSON.stringify({ status: { repoDigests: [release.image], id: release.configDigest } }) };
      else if (args.includes('rm')) { assert(state.hashReads > 1, 'removal must reverify node identity'); state.removed = true; state.present = false; }
      return { status: 0, output: '' };
    };
    await run(f, state, calls);
  } finally { await rm(directory, { recursive: true, force: true }); }
}
test('review2 node archive copy preserves existing files and dangling symlinks', async () => {
  for (const mode of ['present', 'dangling']) await nodeImportCase(mode, async (f, state, calls) => {
    await assert.rejects(importArtifacts(f)); assert.equal(state.removed, false); assert.equal(calls.some(args => args[0] === 'cp'), false);
  });
});
test('review2 import and removal verify the node archive type and exact identity', async () => {
  await nodeImportCase('fresh', async (f, state) => { await importArtifacts(f); assert(state.imported && state.removed); });
  for (const mode of ['replaced-symlink', 'tampered-after-import']) await nodeImportCase(mode, async (f, state) => {
    await assert.rejects(importArtifacts(f)); assert.equal(state.removed, false);
    if (mode === 'replaced-symlink') assert.equal(state.imported, false);
  });
});
test('owned copied archive accepts only root or recorded runner UID and normalizes without identity change', async () => {
  await nodeImportCase('runner-uid', async (f, state) => { await importArtifacts(f); assert(state.imported && state.removed); assert.equal(state.chowns, process.getuid() === 0 ? 0 : 1); });
  await nodeImportCase('foreign-uid', async (f, state) => { await assert.rejects(importArtifacts(f)); assert.equal(state.chowns, 0); assert.equal(state.imported, false); });
  if (process.getuid() !== 0) await nodeImportCase('replaced-after-chown', async (f, state) => { await assert.rejects(importArtifacts(f)); assert.equal(state.imported, false); assert.equal(state.removed, false); });
});
test('import records only attributable additional references with durable preexisting-reference provenance', async () => {
  await nodeImportCase('fresh', async (f) => {
    await importArtifacts(f); const alias = f.custody.entries.find(e => e.kind === 'containerd-image');
    assert.deepEqual(alias.preexistingReferences, ['preexisting-shared']); assert.equal(alias.additionalAliases.length, 2);
    assert(alias.additionalAliases.every(e => e.status === 'owned' && e.name !== 'preexisting-shared'));
  });
});
test('additional alias cleanup refuses shared or retargeted references and removes only exact owned targets', async () => {
  for (const mode of ['shared', 'retargeted', 'owned']) {
    const digest = 'sha256:' + 'a'.repeat(64), name = 'import-owned@' + digest, refs = new Set([name, 'shared-ref']), removed = [];
    const item = { nodeID: 'node', preexistingReferences: mode === 'shared' ? [name, 'shared-ref'] : ['shared-ref'], additionalAliases: [{ name, digest, status: 'owned' }] };
    const f = { saveCustody: async () => {}, docker: async args => {
      if (args.includes('--quiet')) return { output: [...refs].join('\n') };
      if (args.includes('remove')) { removed.push(args.at(-1)); refs.delete(args.at(-1)); return { output: '' }; }
      return { output: 'REF TYPE DIGEST\n' + name + ' application/manifest ' + (mode === 'retargeted' ? 'sha256:' + 'b'.repeat(64) : digest) };
    } };
    if (mode === 'owned') { await removeImportedAliases(f, item); assert.deepEqual(removed, [name]); assert(refs.has('shared-ref')); }
    else { await assert.rejects(removeImportedAliases(f, item)); assert.deepEqual(removed, []); }
  }
});
async function callbackCleanupCase(mode, run) {
  const directory = await mkdtemp(join(tmpdir(), 'mail-cleanup-resume-'));
  try {
    const id = 'c'.repeat(64), imageID = 'sha256:' + 'd'.repeat(64), nonce = 'abcdef123456';
    const callback = { kind: 'container', name: 'owned-callback', id, volumes: [], status: mode === 'removed' ? 'removed' : 'owned' };
    const image = { kind: 'docker-image', name: 'owned:tag', id: imageID, absentBeforeCreate: true, status: 'owned' };
    const f = new Fixture({ directory, runID: nonce, kubeconfig: '/not-read/dev.yaml' }, { topology: { callbackID: id }, paths: { input: join(directory, 'input.json') } }, { entries: [image, callback], retainedK3s: { id: 'node', initialState: 'exited', startIntent: true } });
    const state = { absent: ['removed', 'absent'].includes(mode), stopped: false, imageFailure: mode === 'partial', inspections: 0, mutations: [] };
    const actual = { Id: mode === 'mismatch' ? 'foreign' : id, Name: '/owned-callback', Config: { Labels: { 'webdock.native-mail': nonce } }, Mounts: [] };
    f.saveCustody = async () => {};
    f.command = async (_program, args) => ({ output: args.includes('current-context') ? 'webdock-dev' : 'https://127.0.0.1:56443' });
    f.inspectRetained = async () => ({ Id: 'node', State: { Running: !state.stopped, Status: state.stopped ? 'exited' : 'running' }, NetworkSettings: { Networks: { [NETWORK]: {} } } });
    f.inspect = async () => { state.inspections++; assert(!state.absent, 'removed callback must not be inspected'); return actual; };
    f.docker = async args => {
      if (args[0] === 'inspect') {
        state.inspections++;
        if (mode === 'daemon') return { status: 1, stderr: 'Cannot connect to the Docker daemon' };
        return state.absent ? { status: 1, stderr: 'Error: No such object: ' + id } : { status: 0, output: args.includes('--format') ? JSON.stringify(actual) : JSON.stringify([actual]) };
      }
      if (args[0] === 'image' && args[1] === 'inspect') {
        if (state.imageFailure) { state.imageFailure = false; return { status: 1, stderr: 'Cannot connect to the Docker daemon' }; }
        return { status: 0, output: args.includes('--format') ? JSON.stringify(imageID) : JSON.stringify([{ Id: imageID }]) };
      }
      state.mutations.push(args);
      if (args[0] === 'rm') state.absent = true;
      if (args[0] === 'stop' && args[1] === 'node') state.stopped = true;
      return { status: 0, output: '' };
    };
    await run(f, state, callback);
  } finally { await rm(directory, { recursive: true, force: true }); }
}
test('review3 cleanup skips recorded removed callback and completes node restoration', async () => {
  await callbackCleanupCase('removed', async (f, state) => { await cleanup(f); assert(state.stopped); assert.equal(state.inspections, 0); });
});
test('review3 cleanup resumes after callback removal followed by a later daemon failure', async () => {
  await callbackCleanupCase('partial', async (f, state, callback) => {
    await assert.rejects(cleanup(f)); assert.equal(callback.status, 'removed');
    const before = state.inspections; await cleanup(f); assert.equal(state.inspections, before); assert(state.stopped); assert(f.report.cleanup.verified);
  });
});
test('review3 callback absence must be verified; daemon errors and replacement stay closed', async () => {
  await callbackCleanupCase('absent', async (f, state, callback) => { await cleanup(f); assert.equal(callback.status, 'removed'); assert(state.stopped); });
  for (const mode of ['daemon', 'mismatch']) await callbackCleanupCase(mode, async (f, state, callback) => {
    await assert.rejects(cleanup(f)); assert.equal(callback.status, 'owned'); assert.deepEqual(state.mutations, []);
  });
});
test('review3 resumed cleanup never queries removed namespaces on an already restored node', async () => {
  await callbackCleanupCase('removed', async (f, state) => {
    state.stopped = true;
    f.custody.entries.push({ kind: 'namespace-intent', name: 'wd-removed', status: 'removed' });
    f.kube = async args => { assert(args.includes('config'), 'removed namespace must not require a live API'); return { output: 'https://127.0.0.1:56443' }; };
    await cleanup(f); assert(f.report.cleanup.verified);
    assert.equal(state.mutations.some(args => args[0] === 'stop' && args[1] === 'node'), false);
  });
});
async function guardedExecutionCase(mode, run) {
  const directory = await mkdtemp(join(tmpdir(), 'mail-execution-guard-'));
  try {
    const { readFile } = await import('node:fs/promises');
    const source = 'tools/tests/native-plugin-fixture.mjs', hash = sha256(await readFile(join(ROOT, source)));
    const binary = join(directory, 'binary'); await writePrivate(binary, 'owned test binary');
    const input = { directory, kubeconfig: '/not-read/dev.yaml', review: { sourceFiles: { [source]: mode === 'source' ? '0'.repeat(64) : hash }, binaries: { fixture: { path: binary, sha256: mode === 'binary' ? '0'.repeat(64) : sha256('owned test binary') } } } };
    const reportPath = join(directory, 'report.json'); await writePrivate(reportPath, { paths: { input: join(directory, 'input.json') } });
    const f = new Fixture(input, { completedStages: ['start'] }, { entries: [], retainedK3s: { id: 'node' } }), events = [];
    f.release = async () => {};
    f.command = async (_program, args) => { const context = args.includes('current-context'); events.push(context ? 'context' : 'endpoint'); return { output: context ? mode === 'context' ? 'foreign-context' : 'webdock-dev' : 'https://127.0.0.1:56443' }; };
    f.inspectRetained = async () => { events.push('inspect'); return { Id: 'node', State: { Running: true }, NetworkSettings: { Networks: { [NETWORK]: {} } } }; };
    f.saveCustody = async () => { events.push('browser-intent'); throw Error('STOP_BEFORE_BROWSER_LAUNCH'); };
    f.json = async () => { events.push('proof-read'); throw Error('STOP_BEFORE_PROOF_IO'); };
    await run(f, reportPath, events);
  } finally { await rm(directory, { recursive: true, force: true }); }
}
test('review4 browser checks current source, binary and context before its first mutation', async () => {
  for (const mode of ['source', 'binary', 'context']) await guardedExecutionCase(mode, async (f, path, events) => {
    const category = mode === 'source' ? /reviewed_source_changed/ : mode === 'binary' ? /reviewed_binary_changed/ : /dev_context_required/;
    await assert.rejects(acceptance(path, async () => f), category);
    assert.equal(events.includes('browser-intent'), false);
    if (mode !== 'context') assert.deepEqual(events, []);
  });
  await guardedExecutionCase('good', async (f, path, events) => {
    await assert.rejects(acceptance(path, async () => f), /STOP_BEFORE_BROWSER_LAUNCH/);
    assert.deepEqual(events, ['context', 'endpoint', 'inspect', 'browser-intent']);
  });
});
test('review4 standalone observe applies the same guards before any proof IO', async () => {
  await guardedExecutionCase('source', async (f, _path, events) => { await assert.rejects(observe(f), /reviewed_source_changed/); assert.deepEqual(events, []); });
  await guardedExecutionCase('context', async (f, _path, events) => { await assert.rejects(observe(f), /dev_context_required/); assert.equal(events.includes('proof-read'), false); });
  await guardedExecutionCase('good', async (f, _path, events) => { await assert.rejects(observe(f), /STOP_BEFORE_PROOF_IO/); assert.deepEqual(events, ['context', 'endpoint', 'inspect', 'proof-read']); });
});
function policyProof(spec) {
  const objects = {
    namespace: { metadata: { name: 'wd-mail-c123', uid: 'ns', labels: { 'webdock.dev/plugin': 'mail' } } },
    deployment: { metadata: { uid: 'deployment', labels: { 'webdock.dev/customer-id': '123' } }, spec: { template: { spec: { containers: [{ image: 'owned-stalwart' }] } } } },
    pvc: { metadata: { name: 'engine-data', uid: 'pvc', labels: { 'webdock.dev/customer-id': '123' } }, status: { phase: 'Bound' }, spec: { volumeName: 'pv' } },
    pv: { metadata: { name: 'pv', uid: 'pv-uid' }, spec: { claimRef: { uid: 'pvc' } } },
    networkpolicy: { spec }
  };
  return { input: { images: { stalwart: 'owned-stalwart' } }, custody: { entries: [{ kind: 'persistentvolume', name: 'pv', uid: 'pv-uid', claimUID: 'pvc' }] }, ownObject: async () => {}, kubeJSON: async args => objects[args[args.indexOf('get') + 1]] };
}
test('review5 deny-egress proof accepts Kubernetes omitted empty egress', async () => {
  const base = { policyTypes: ['Egress'], podSelector: { matchLabels: { app: 'engine' } } };
  await engineObjects(policyProof(base), '123');
  await engineObjects(policyProof({ ...base, egress: [] }), '123');
});
test('review5 an empty egress list alone cannot prove denial for engine pods', async () => {
  for (const spec of [
    { policyTypes: ['Ingress'], podSelector: { matchLabels: { app: 'engine' } }, egress: [] },
    { policyTypes: ['Egress'], podSelector: { matchLabels: { app: 'other' } }, egress: [] },
    { policyTypes: ['Egress'], podSelector: { matchLabels: { app: 'engine' } }, egress: [{}] }
  ]) await assert.rejects(engineObjects(policyProof(spec), '123'));
});
test('review6 ready mailbox row waits for actual incoming readiness before private probes', async () => {
  const events = [], states = ['provisioning', 'ready']; let probes = 0;
  const f = { wait: async (check, deadline) => { assert.equal(deadline, 300000); for (let n = 0; n < 2; n++) { const result = await check(); if (result) return result; } throw Error('deadline'); } };
  await waitMailReady(f, async () => ({ state: 'ready', username: 'owned@example.test' }), async () => { const state = states.shift(); events.push(state); return { incoming: { state, desiredEnabled: true } }; });
  probes++; assert.deepEqual(events, ['provisioning', 'ready']); assert.equal(probes, 1);
});
test('review6 readiness exposes API/engine failures and remains bounded while provisioning', async () => {
  const f = { wait: async check => { for (let n = 0; n < 2; n++) { const result = await check(); if (result) return result; } throw Error('fixture_deadline_exceeded'); } };
  const mailbox = async () => ({ state: 'ready' });
  await assert.rejects(waitMailReady(f, mailbox, async () => { throw Error('actual_domain_api_failure'); }), /actual_domain_api_failure/);
  await assert.rejects(waitMailReady(f, mailbox, async () => ({ incoming: { state: 'failed', desiredEnabled: true } })), /incoming_reconciliation_failed/);
  await assert.rejects(waitMailReady(f, mailbox, async () => ({ incoming: { state: 'provisioning', desiredEnabled: true } })), /fixture_deadline_exceeded/);
});
test('cleanup uses original custody input even after reviewed source hashes or plan fields change', () => {
  const snapshot = { runID: 'abcdef123456', directory: '/owned/run', review: { sourceFiles: { 'plan.md': 'old' } } };
  const changed = { ...snapshot, review: { sourceFiles: { 'plan.md': 'new' } }, authorizedStages: ['cleanup'] };
  const report = { runID: snapshot.runID, directory: snapshot.directory, inputSha256: sha256(JSON.stringify(snapshot)) };
  assert.equal(frozenInput(changed, snapshot, report, 'cleanup'), snapshot);
  assert.throws(() => frozenInput(changed, snapshot, report, 'start'));
  assert.throws(() => frozenInput({ ...changed, runID: 'foreign' }, snapshot, report, 'cleanup'));
});
test('private assertion errors contain only the fixed category, never actual or expected values', () => {
  let error;
  try { requirePrivateEqual({ password: 'PRIVATE_SENTINEL_A' }, { password: 'PRIVATE_SENTINEL_B' }, 'retention_failed'); } catch (value) { error = value; }
  assert.equal(error.message, 'retention_failed');
  assert.equal(JSON.stringify(error).includes('PRIVATE_SENTINEL'), false);
  assert.equal(error.stack.includes('PRIVATE_SENTINEL'), false);
});
test('subprocess environment does not inherit database, provider or loader variables', () => {
  process.env.WEBDOCK_FIXTURE_SECRET_TEST = 'ambient';
  const env = ownEnvironment('/private/run', { NODE_ENV: 'development' });
  assert.equal(env.WEBDOCK_FIXTURE_SECRET_TEST, undefined);
  assert.equal(env.HOME, '/private/run/home');
  assert.equal(env.NODE_OPTIONS, undefined);
  delete process.env.WEBDOCK_FIXTURE_SECRET_TEST;
});
test('command arguments survive shell metacharacters and children receive only owned env', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mail-command-'));
  const { mkdir } = await import('node:fs/promises'); await mkdir(join(directory, 'logs'));
  try {
    const f = new Fixture({ directory }, {}, { entries: [] });
    const literal = "'$(touch no-such-fixture-file)`id`\n";
    const r = await f.command(process.execPath, ['--no-env-file', '-e', 'process.stdout.write(process.argv[1])', literal]);
    assert.equal(r.stdout, literal);
    const q = await f.command('sh', ['-ec', 'printf %s ' + shellQuote(literal)]); assert.equal(q.stdout, literal);
    const child = await f.command(process.execPath, ['--no-env-file', '-e', 'process.stdout.write(String(process.env.DATABASE_URL)+":"+String(process.env.HTTP_PROXY))']);
    assert.equal(child.output, 'undefined:undefined');
  } finally { await rm(directory, { recursive: true, force: true }); }
});
test('shell quoting keeps substitutions, newlines and apostrophes literal', () => {
  assert.equal(shellQuote("a'$(touch nope)\n`id`"), "'a'\"'\"'$(touch nope)\n`id`'");
  assert.equal(shellQuote(''), "''");
});
test('diagnostics redact DSNs, key blocks, cookies and known secret values', () => {
  const value = redact('postgresql://a:p@host/db token=known password=abc Cookie: sid=xyz\n-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----', ['known']);
  for (const secret of ['a:p@host', 'known', 'abc', 'sid=xyz', 'BEGIN PRIVATE']) assert.equal(value.includes(secret), false);
});
test('the real CLI withholds a malformed private input from stderr', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mail-cli-'));
  const { mkdir, writeFile } = await import('node:fs/promises'); await mkdir(join(directory, 'logs'));
  try {
    const path = join(directory, 'input.json'); await writeFile(path, '{"value":"PRIVATE_CLI_SENTINEL",BROKEN}', { mode: 0o600 });
    const f = new Fixture({ directory }, {}, { entries: [] });
    const r = await f.command(process.execPath, ['--no-env-file', join(ROOT, 'tools/tests/native-plugin-fixture.mjs'), 'preflight', '--input', path], { allowFailure: true });
    assert.equal(r.status, 1); assert.equal(r.stdout, ''); assert.equal(r.stderr, 'native_mail_fixture_failed; retain private custody for root review\n');
  } finally { await rm(directory, { recursive: true, force: true }); }
});
test('custody records intent before mutation and requires exact container/object identities', async () => {
  const ledger = { entries: [] }, events = [];
  const save = async () => events.push(JSON.parse(JSON.stringify(ledger)));
  const item = await custodyIntent(ledger, save, 'container', 'owned', { label: 'abc' });
  assert.equal(events[0].entries[0].status, 'intent');
  await custodyConfirm(item, save, { id: 'a'.repeat(64) });
  assert.equal(events[1].entries[0].id, 'a'.repeat(64));
  const container = { Id: 'a'.repeat(64), Name: '/owned', Config: { Labels: { 'webdock.native-mail': 'abc' } } };
  requireContainer(item, container, 'abc');
  assert.throws(() => requireContainer(item, { ...container, Id: 'b'.repeat(64) }, 'abc'));
  const object = { metadata: { name: 'owned', uid: 'uid', resourceVersion: '3' } };
  requireObject({ name: 'owned', uid: 'uid' }, object);
  assert.throws(() => requireObject({ name: 'owned', uid: 'foreign' }, object));
});
test('Kubernetes cleanup sends UID and current resourceVersion preconditions, refuses replacement', async () => {
  const deleted = [];
  const object = { metadata: { name: 'wd-owned', uid: 'uid-owned', resourceVersion: '8' } };
  let gone = false;
  const f = { custody: { entries: [] }, saveCustody: async () => {}, kubeJSON: async () => ({ items: [] }), wait: async check => assert(await check()), kube: async (args, options) => {
    if (args[0] === 'delete') { deleted.push({ args, options }); gone = true; return { output: '' }; }
    return { output: gone ? '' : JSON.stringify(object) };
  } };
  await assert.rejects(deleteOwnedObject(f, { kind: 'namespace', name: 'wd-owned', uid: 'foreign' }));
  assert.equal(deleted.length, 0);
  const item = { kind: 'namespace', name: 'wd-owned', uid: 'uid-owned' }; await deleteOwnedObject(f, item);
  assert.equal(deleted.length, 1); assert.deepEqual(JSON.parse(deleted[0].options.input).preconditions, { uid: 'uid-owned', resourceVersion: '8' }); assert.equal(item.status, 'removed');
});
test('private files refuse symlinks, unsafe modes, hard links and replacement', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'mail-source-'));
  try {
    const path = join(dir, 'private.json');
    await writePrivate(path, { value: 1 });
    assert.deepEqual(JSON.parse(await readPrivate(path)), { value: 1 });
    await assert.rejects(writePrivate(path, { value: 2 }));
    await symlink(path, join(dir, 'link'));
    await assert.rejects(readPrivate(join(dir, 'link')));
    await link(path, join(dir, 'hardlink'));
    await assert.rejects(readPrivate(path));
    await rm(join(dir, 'hardlink'));
    await chmod(path, 0o644);
    await assert.rejects(readPrivate(path));
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('finalizing an idle callback catalog preserves its readonly bind-mount inode', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'mail-pinned-'));
  try {
    const path = join(dir, 'catalog.json'); await writePrivate(path, { releases: [] });
    const before = await stat(path); await writePinnedPrivate(path, { releases: ['mail'] });
    assert.equal((await stat(path)).ino, before.ino);
    assert.deepEqual(JSON.parse(await readPrivate(path)), { releases: ['mail'] });
  } finally { await rm(dir, { recursive: true, force: true }); }
});
test('signed Mail releases bind exact asset/manifest/image and reject altered identity', () => {
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const template = { name: 'mail', version: '0.1.0', image: '', schemaVersion: 1, readableSchema: { min: 1, max: 1 } };
  const release = signedRelease(template, '0.2.0', 'fixture.invalid/mail@sha256:' + 'a'.repeat(64), privateKey, 'https://fixture.invalid', 'owned', {});
  verifyRelease(release, publicKey);
  const [header, payload, signature] = release.jws.split('.');
  assert(verify(null, Buffer.from(header + '.' + payload), publicKey, Buffer.from(signature, 'base64url')));
  assert.equal(JSON.parse(Buffer.from(payload, 'base64url')).assets[0].name, 'mail-universal.json');
  assert.throws(() => verifyRelease({ ...release, descriptor: release.descriptor + '\n' }, publicKey));
  assert.throws(() => verifyRelease({ ...release, id: 'postgres' }, publicKey));
  assert.throws(() => imageIdentity('fixture.invalid/mail:latest'));
  assert.throws(() => imageIdentity('fixture.invalid/mail@sha256:' + '0'.repeat(64)));
  assert.equal(imageIdentity(template.image || 'fixture.invalid/mail@sha256:' + 'b'.repeat(64)).digest, 'sha256:' + 'b'.repeat(64));
  assert.equal(sha256(release.manifestRaw), JSON.parse(Buffer.from(payload, 'base64url')).manifest_sha256);
});
test('input rejects hidden authorization and non-dev context before touching infrastructure', () => {
  assert.throws(() => validateInput({ version: 1, authorizedStages: ['start'], directory: '/tmp' }));
  const input = { version: 1, runID: 'abcdef123456', directory: join(BASE, 'abcdef123456'), authorizedStages: ['preflight'], allowStartRetainedK3s: false, kubeconfig: join(ROOT, '.dev-local/k3s/kubeconfig.yaml'), review: { sourceFiles: { 'plugins/mail/Dockerfile': 'a'.repeat(64) }, binaries: Object.fromEntries(BINARY_NAMES.map(n => [n, { path: '/reviewed/' + n, sha256: 'a'.repeat(64) }])) }, images: Object.fromEntries(['postgres', 'registry', 'stalwart'].map(n => [n, 'fixture.invalid/' + n + '@sha256:' + 'b'.repeat(64)])), ports: { core: 6130, console: 6121, account: 6125, callback: 6140 } };
  assert.equal(validateInput(input), input);
  const legacy = { ...input, review: { ...input.review, binaries: { ...input.review.binaries } } }; delete legacy.review.binaries.bun; delete legacy.review.binaries['docker-buildx'];
  assert.equal(validateInput(legacy, 'cleanup'), legacy); assert.throws(() => validateInput(legacy));
  assert.throws(() => validateInput({ ...input, kubeconfig: '/private/production.yaml' }));
  assert.throws(() => validateInput({ ...input, directory: input.directory + '-other' }));
  assert.throws(() => validateInput({ ...input, images: { ...input.images, registry: 'registry:latest' } }));
  assert.throws(() => validateInput({ ...input, review: { ...input.review, sourceFiles: { 'CLAUDE_HANDOVER.md': 'a'.repeat(64) } } }));
  requireDevContext('webdock-dev', 'https://127.0.0.1:56443');
  for (const [context, endpoint] of [['other', 'https://127.0.0.1:56443'], ['webdock-dev', 'https://127.0.0.1:6443'], ['webdock-dev', 'https://host:56443']]) assert.throws(() => requireDevContext(context, endpoint));
});
test('runtime namespace derivation keeps gen1 identity and isolates lifecycle generations', () => {
  const platform = 'mail-abcdef123456';
  assert.equal(runtimeNamespace(platform, 1), 'wd-plugin-' + sha256(`${platform.length}:${platform}4:mail`).slice(0, 24));
  assert.notEqual(runtimeNamespace(platform, 1), runtimeNamespace(platform, 2));
  assert.notEqual(runtimeNamespace(platform, 2), runtimeNamespace(platform, 3));
});
test('all acceptance modules import without reading private files or starting processes', async () => {
  const proof = await import('./native-mail-proof.mjs'); assert.equal(typeof proof.observe, 'function');
  const browser = await import('../../apps/console/tests/mail-installation-native.mjs'); assert.equal(typeof browser.acceptance, 'function');
});
