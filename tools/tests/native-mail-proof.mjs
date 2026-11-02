// Private Mail retention observations. No fixture SQL writes and no delivery calls.
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { sha256, processIdentity, custodyIntent, custodyConfirm, requirePrivateEqual, verifyExecution } from './native-plugin-fixture.mjs';

function metadataNames(platformID) {
  const identity = sha256(JSON.stringify([platformID, 'mail'])).slice(0, 32);
  return { database: 'wd_pm_d_' + identity, role: 'wd_pm_r_' + identity };
}
async function database(f, name, application, action) {
  const config = await f.json(f.path('storage-' + name + '.json'));
  const url = new URL(config.url); if (application) url.pathname = '/' + metadataNames(f.report.platformID).database;
  const { Client } = await import('pg');
  const db = new Client({ connectionString: url.href, ssl: { ca: config.ca, rejectUnauthorized: true }, connectionTimeoutMillis: 10000, query_timeout: 10000 });
  await db.connect();
  try { await db.query('BEGIN READ ONLY'); const result = await action(db); await db.query('ROLLBACK'); return result; }
  finally { await db.end(); }
}
export async function noEngines(f) {
  await database(f, 'metadata', true, async db => {
    const row = (await db.query('SELECT (SELECT count(*)::int FROM customer_engines) AS customers,(SELECT count(*)::int FROM engines) AS legacy,(SELECT count(*)::int FROM mailboxes) AS mailboxes')).rows[0];
    assert.deepEqual(row, { customers: 0, legacy: 0, mailboxes: 0 }, 'unexpected_incoming_reservation');
  });
  await database(f, 'platform', false, async db => {
    assert.equal((await db.query("SELECT count(*)::int AS n FROM webdock_auth.plugin_workload WHERE plugin_id='mail'")).rows[0].n, 0, 'unexpected_mail_workload');
  });
  const claims = await f.kubeJSON(['get', 'pvc', '--all-namespaces', '-l', 'webdock.dev/plugin=mail']);
  const initial = new Set(f.report.topology.initialMailPVCUIDs);
  assert(claims.items.every(c => initial.has(c.metadata.uid)), 'unexpected_mailbox_volume');
}
export async function metadataSnapshot(f, customerID) {
  const names = metadataNames(f.report.platformID);
  const identity = await database(f, 'metadata', false, async db => {
    const row = (await db.query('SELECT d.oid::text AS database_oid,d.datname,r.oid::text AS role_oid,r.rolname,r.rolpassword,shobj_description(d.oid,\'pg_database\') AS database_marker,shobj_description(r.oid,\'pg_authid\') AS role_marker FROM pg_database d JOIN pg_authid r ON r.oid=d.datdba WHERE d.datname=$1 AND r.rolname=$2', [names.database, names.role])).rows[0];
    assert(row?.rolpassword && row.database_marker && row.role_marker, 'metadata_ownership_missing');
    return { database: row.datname, databaseOID: row.database_oid, role: row.rolname, roleOID: row.role_oid, passwordVerifier: sha256(row.rolpassword), databaseMarker: sha256(row.database_marker), roleMarker: sha256(row.role_marker) };
  });
  const data = await database(f, 'metadata', true, async db => {
    const engines = (await db.query('SELECT customer_id,id::text,encode(admin_sealed,\'hex\') AS sealed FROM customer_engines ORDER BY customer_id')).rows;
    assert.equal(engines.length, 1, 'one_customer_engine_required'); assert.equal(engines[0].customer_id, customerID);
    const mailboxes = (await db.query('SELECT id::text,project_id,customer_id,account_id,encode(password_sealed,\'hex\') AS sealed FROM mailboxes ORDER BY id')).rows;
    const relays = (await db.query('SELECT id::text,project_id,encode(key_sealed,\'hex\') AS key_sealed,encode(secret_sealed,\'hex\') AS secret_sealed FROM relays ORDER BY id')).rows;
    const domains = (await db.query('SELECT id::text,project_id,domain,token FROM domains ORDER BY id')).rows;
    const migrations = (await db.query('SELECT version FROM schema_migrations ORDER BY version')).rows.map(r => r.version); assert.deepEqual(migrations, [1, 2, 3]);
    return { engines, mailboxes, relays, domains, migrations };
  });
  const workload = await database(f, 'platform', false, async db => {
    const rows = (await db.query("SELECT plugin_id,unit,customer_id,project_id,cluster_id,namespace FROM webdock_auth.plugin_workload WHERE plugin_id='mail' ORDER BY unit")).rows;
    assert.equal(rows.length, 1); assert.equal(rows[0].unit, 'engine-c' + customerID); assert.equal(rows[0].customer_id, customerID); assert.equal(rows[0].project_id, null); return rows[0];
  });
  return { identity, data, workload };
}
export async function engineObjects(f, customerID) {
  const namespace = 'wd-mail-c' + customerID;
  const ns = await f.kubeJSON(['get', 'namespace', namespace]);
  assert.equal(ns.metadata.labels?.['webdock.dev/plugin'], 'mail', 'engine_namespace_owner_mismatch');
  await f.ownObject('namespace', ns);
  const deployment = await f.kubeJSON(['-n', namespace, 'get', 'deployment', 'engine']);
  assert.equal(deployment.metadata.labels?.['webdock.dev/customer-id'], customerID);
  assert.equal(deployment.spec.template.spec.containers[0].image, f.input.images.stalwart);
  const pvc = await f.kubeJSON(['-n', namespace, 'get', 'pvc', 'engine-data']);
  assert.equal(pvc.metadata.labels?.['webdock.dev/customer-id'], customerID);
  await f.ownObject('persistentvolumeclaim', pvc);
  assert.equal(pvc.status.phase, 'Bound');
  const pv = await f.kubeJSON(['get', 'pv', pvc.spec.volumeName]); assert.equal(pv.spec.claimRef.uid, pvc.metadata.uid);
  const known = f.custody.entries.find(e => e.kind === 'persistentvolume' && e.name === pv.metadata.name);
  if (!known) {
    const item = await custodyIntent(f.custody, f.saveCustody, 'persistentvolume', pv.metadata.name, { claimUID: pvc.metadata.uid });
    await custodyConfirm(item, f.saveCustody, { uid: pv.metadata.uid });
  } else { assert.equal(known.uid, pv.metadata.uid); assert.equal(known.claimUID, pvc.metadata.uid); }
  const policy = await f.kubeJSON(['-n', namespace, 'get', 'networkpolicy', 'engine-no-egress']);
  assert(policy.spec.policyTypes?.includes('Egress'), 'engine_egress_policy_type_required');
  assert.deepEqual(policy.spec.podSelector, { matchLabels: { app: 'engine' } }, 'engine_egress_selector_changed');
  assert.deepEqual(policy.spec.egress ?? [], [], 'engine_egress_not_denied');
  return { namespace, namespaceUID: ns.metadata.uid, deploymentUID: deployment.metadata.uid, pvcUID: pvc.metadata.uid, pvName: pv.metadata.name, pvUID: pv.metadata.uid };
}
export async function runtimeGenerations(f, generation, version) {
  const catalog = await f.json(f.report.paths.catalog), image = JSON.parse(catalog.releases.find(r => r.version === version).manifestRaw).image;
  const list = await f.kubeJSON(['get', 'namespaces', '-l', `webdock.dev/platform=${f.report.platformID},webdock.dev/plugin=mail`]);
  let found = false;
  const generations = [];
  for (const ns of list.items) {
    await f.ownObject('namespace', ns);
    const current = ns.metadata.labels['webdock.dev/runtime-generation'];
    const deployment = await f.kubeJSON(['-n', ns.metadata.name, 'get', 'deployment', 'runtime']);
    const pods = await f.kubeJSON(['-n', ns.metadata.name, 'get', 'pods']);
    const secretUID = (await f.kube(['-n', ns.metadata.name, 'get', 'secret', 'runtime-config', '-o', 'jsonpath={.metadata.uid}'])).output;
    assert(secretUID, 'generation_identity_secret_missing');
    if (current === String(generation)) {
      assert.equal(deployment.spec.replicas, 1); assert.equal(deployment.status.availableReplicas, 1); assert.equal(deployment.spec.template.spec.containers[0].image, image); found = true;
    } else { assert.equal(deployment.spec.replicas, 0); assert.equal(pods.items.length, 0, 'predecessor_pods_remain'); }
    generations.push({ generation: current, namespace: ns.metadata.name, namespaceUID: ns.metadata.uid, deploymentUID: deployment.metadata.uid, secretUID });
  }
  assert(found, 'promoted_runtime_namespace_missing');
  assert.deepEqual(generations.map(g => g.generation).sort(), Array.from({ length: generation }, (_, i) => String(i + 1)), 'retained_runtime_generation_missing');
  assert.equal(new Set(generations.map(g => g.secretUID)).size, generation, 'generation_identity_secret_reused');
  return generations;
}
async function jmap(origin, username, password, method, args) {
  const result = await fetch(origin + '/jmap', { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(10000), headers: { authorization: 'Basic ' + Buffer.from(username + ':' + password).toString('base64'), 'content-type': 'application/json' }, body: JSON.stringify({ using: ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:mail'], methodCalls: [[method, args, 'fixture']] }) });
  assert(result.ok, 'local_mailbox_authentication_failed'); const body = await result.json();
  assert.equal(body.methodResponses?.[0]?.[0], method, 'local_jmap_method_failed');
  const value = body.methodResponses[0][1]; assert(['notCreated', 'notUpdated', 'notDestroyed'].every(key => Object.keys(value[key] ?? {}).length === 0), 'local_jmap_write_failed'); return value;
}
export async function withMailbox(f, customerID, action) {
  const namespace = 'wd-mail-c' + customerID;
  const item = await f.startProcess('mailbox-forward-' + Date.now(), 'kubectl', ['--kubeconfig', f.input.kubeconfig, '--context', 'webdock-dev', '-n', namespace, 'port-forward', '--address', '127.0.0.1', 'service/engine', ':8080'], {});
  const log = f.path('logs/' + item.name + '.stdout');
  try {
    const port = await f.wait(async () => { const { readFile } = await import('node:fs/promises'); return Number(/Forwarding from 127\.0\.0\.1:(\d+) -> 8080/.exec(await readFile(log, 'utf8'))?.[1]) || false; }, 30000);
    return await action('http://127.0.0.1:' + port);
  } finally {
    if (existsSync(`/proc/${item.pid}`)) { assert.deepEqual(processIdentity(item.pid), { startTime: item.startTime, commandSha256: item.commandSha256 }); process.kill(-item.pid, 'SIGTERM'); await f.wait(() => !existsSync(`/proc/${item.pid}`), 30000); }
    item.status = 'removed'; await f.saveCustody();
  }
}
export async function mailboxSentinel(f, customerID, username, password, accountID, existing) {
  return withMailbox(f, customerID, async origin => {
    if (existing) {
      const response = await jmap(origin, username, password, 'Email/get', { accountId: accountID, ids: [existing.id], fetchTextBodyValues: true });
      assert.equal(response.list?.[0]?.subject, existing.subject);
      assert(Object.values(response.list[0].bodyValues ?? {}).some(body => body.value?.trimEnd() === existing.body), 'mailbox_sentinel_missing');
      return existing;
    }
    const folders = await jmap(origin, username, password, 'Mailbox/get', { accountId: accountID, ids: null });
    const draft = folders.list.find(folder => folder.role === 'drafts'); assert(draft, 'drafts_mailbox_missing');
    const subject = 'Owned retained draft ' + f.input.runID, body = 'Local stored Mail lifecycle sentinel ' + f.input.runID;
    const created = await jmap(origin, username, password, 'Email/set', { accountId: accountID, create: { sentinel: { mailboxIds: { [draft.id]: true }, keywords: { $draft: true }, from: [{ email: username }], subject, textBody: [{ partId: 'body', type: 'text/plain' }], bodyValues: { body: { value: body } } } } });
    assert(created.created?.sentinel?.id, 'draft_creation_failed'); return { id: created.created.sentinel.id, subject, body };
  });
}
export async function observe(f) {
  await verifyExecution(f);
  const state = await f.json(f.path('acceptance-state.json')); assert(state.complete, 'browser_acceptance_not_complete');
  await runtimeGenerations(f, 3, '0.1.0');
  requirePrivateEqual(await metadataSnapshot(f, state.customerID), state.baseline.metadata, 'retained_metadata_changed');
  assert.deepEqual(await engineObjects(f, state.customerID), state.baseline.engine, 'retained_engine_storage_changed');
  f.report.proof = { generations: [1, 2, 3], metadataAndCiphertextsRetained: true, originalMailboxCredentialUnseals: true, sentinelRetained: true, suspendResume: true, sameCustomerReuse: true, foreignDenied: true, outgoingDeliveryClaimed: false };
}
