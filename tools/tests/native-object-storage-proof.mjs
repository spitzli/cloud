// Read-only observations; raw metadata and runtime material remain in the private run directory.
import assert from 'node:assert/strict';
import https from 'node:https';
import { sha256 } from './native-plugin-fixture.mjs';

export async function database(f, name, application, action) {
  const config = await f.json(f.path(name + '.json')), url = new URL(config.url);
  if (application) url.pathname = '/wd_pm_d_' + sha256(JSON.stringify([f.report.platformID, 'object-storage'])).slice(0, 32);
  const { Client } = await import('pg');
  const db = new Client({ connectionString: url.href, ssl: { ca: config.ca, rejectUnauthorized: true }, connectionTimeoutMillis: 10000, query_timeout: 10000 });
  await db.connect();
  try { await db.query('BEGIN READ ONLY'); const value = await action(db); await db.query('ROLLBACK'); return value; }
  finally { await db.end(); }
}
export async function metadataSnapshot(f) {
  const name = 'wd_pm_d_' + sha256(JSON.stringify([f.report.platformID, 'object-storage'])).slice(0, 32);
  const identity = await database(f, 'metadata', false, async db => {
    const row = (await db.query('SELECT d.oid::text AS database_oid,d.datname,r.oid::text AS role_oid,r.rolname,r.rolpassword,shobj_description(d.oid,\'pg_database\') AS database_marker,shobj_description(r.oid,\'pg_authid\') AS role_marker FROM pg_database d JOIN pg_authid r ON r.oid=d.datdba WHERE d.datname=$1', [name])).rows[0];
    assert(row?.rolpassword && row.database_marker && row.role_marker);
    const systemIdentifier = (await db.query('SELECT system_identifier::text FROM pg_catalog.pg_control_system()')).rows[0].system_identifier;
    return { systemIdentifier, databaseOID: row.database_oid, database: row.datname, roleOID: row.role_oid, role: row.rolname, passwordVerifier: sha256(row.rolpassword), databaseMarker: sha256(row.database_marker), roleMarker: sha256(row.role_marker) };
  });
  const platformSystemIdentifier = await database(f, 'platform', false, async db => (await db.query('SELECT system_identifier::text FROM pg_catalog.pg_control_system()')).rows[0].system_identifier);
  assert.notEqual(identity.systemIdentifier, platformSystemIdentifier, 'metadata_must_be_separate_physical_postgresql');
  const resources = await database(f, 'metadata', true, async db => (await db.query('SELECT id::text,body FROM resources ORDER BY id')).rows.map(row => { const r = JSON.parse(row.body); return { id: row.id, binding: r.binding, revision: r.revision, state: r.state, credentials: r.credentials.map(c => ({ accessKeySha256: sha256(c.access_key), sealedSha256: sha256(JSON.stringify(c.sealed)) })) }; }));
  return { identity, resources };
}
export async function runtimeSnapshot(f, stage) {
  const namespaces = (await f.kubeJSON(['get', 'namespaces', '-l', 'webdock.dev/plugin=object-storage'])).items;
  const result = [];
  for (const namespace of namespaces) {
    const name = namespace.metadata.name, objects = [{ kind: 'Namespace', name, uid: namespace.metadata.uid }];
    let config, deployment;
    for (const resource of ['secrets', 'deployments', 'services', 'networkpolicies', 'ingressroutetcps.traefik.io']) {
      const rows = (await f.kubeJSON(['-n', name, 'get', resource])).items;
      assert.equal(rows.length, 1, 'native_six_object_custody_required');
      const object = rows[0]; objects.push({ kind: object.kind, name: object.metadata.name, uid: object.metadata.uid });
      if (resource === 'secrets') config = JSON.parse(Buffer.from(object.data['runtime.json'], 'base64').toString());
      if (resource === 'deployments') deployment = object;
    }
    assert(config?.secretKeyHex && config?.storageUrl);
    const pods = (await f.kubeJSON(['-n', name, 'get', 'pods'])).items;
    result.push({ namespace: name, objects, generation: config.generation, version: JSON.parse(Buffer.from((await f.kubeJSON(['-n', name, 'get', 'secret', objects.find(v => v.kind === 'Secret').name])).data['manifest.json'], 'base64').toString()).version,
      sealingKeySha256: sha256(config.secretKeyHex), storageUrlSha256: sha256(config.storageUrl), replicas: deployment.spec.replicas,
      pods: pods.map(p => ({ name: p.metadata.name, uid: p.metadata.uid, ready: p.status.containerStatuses?.every(c => c.ready) ?? false, containers: p.status.containerStatuses?.map(c => ({ id: c.containerID, imageID: c.imageID, restarts: c.restartCount })) ?? [] })) });
  }
  assert(result.length > 0); await f.write('runtime-' + stage + '.json', result);
  return result;
}
export async function proxyProof(f) {
  const request = ca => new Promise((done, reject) => {
    const req = https.request({ hostname: f.report.proxy.ip, port: 19443, servername: f.report.proxy.domain, path: '/minio/health/ready', method: 'GET', ca, rejectUnauthorized: true, headers: { host: f.report.proxy.domain + ':19443' }, timeout: 5000 }, response => { response.resume(); response.once('end', () => done(response.statusCode)); });
    req.once('error', reject); req.once('timeout', () => req.destroy(Error('deadline'))); req.end();
  });
  assert.equal(await request(f.report.proxy.ca), 200);
  await assert.rejects(request(undefined));
  const current = await f.inspect('webdock-dev-minio-1'); assert(current.State.Running && current.Id === f.report.retained.minioID);
  return { privateCATLSVerified: true, untrustedCARejected: true, existingMinioIdentityRetained: true };
}
