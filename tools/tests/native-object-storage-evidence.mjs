// Publish only an explicit nonsecret allowlist. Runtime JSON, cookies, DSNs and signed URLs stay private.
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { X509Certificate } from 'node:crypto';
import { load } from './native-object-storage-fixture.mjs';
import { ROOT, sha256, readPrivate } from './native-plugin-fixture.mjs';

export function publicEvidence(report, custody, receipt, snapshots) {
  return {
    date: '2026-10-25', scope: 'Owned local MinIO qualification only', runID: report.runID,
    privateEvidenceDirectory: report.directory, platformID: report.platformID ?? null, clusterID: report.clusterID ?? null,
    deadline: new Date(report.deadline).toISOString(), completedStages: report.completedStages ?? [],
    operatorSetup: report.realOperatorSetup ?? null,
    schemaSha256: report.schema ?? null, producerManifestSha256: report.imageManifestSha256 ?? null,
    releases: (report.artifacts ?? []).map(r => ({ version: r.version, imageManifestDigest: r.digest, imageConfigDigest: r.configDigest, signedManifestSha256: r.manifestSha256, archiveSha256: r.archiveSha256 })),
    acceptedOperation: receipt ? { id: receipt.id, pluginID: receipt.pluginID, generation: receipt.generation, accepted: true } : null,
    installationVerified: report.signedNativeInstallation === true, coreReadyStatus: report.coreReadyStatus ?? null,
    installationRecovery: report.installRetryProof ?? null, ownedNodeDiskFloor: report.ownedNodeAbsoluteDiskFloor ?? null,
    binarySha256: Object.fromEntries(Object.entries(report.binaries ?? {}).map(([name, value]) => [name, value.sha256])),
    containers: custody.entries.filter(e => e.kind === 'container').map(e => ({ name: e.name, id: e.id, status: e.status, anonymousVolumes: e.volumes })),
    runtimes: snapshots.map(({ stage, values }) => ({ stage, generations: values.map(r => ({ generation: r.generation, version: r.version, namespace: r.namespace, objects: r.objects, replicas: r.replicas, pods: r.pods.map(p => ({ name: p.name, uid: p.uid, ready: p.ready, containers: p.containers.map(c => ({ id: c.id, imageID: c.imageID, restarts: c.restarts })) })) })) })),
    transportProof: report.proxyProof ?? null, dataProof: report.dataProof ?? null, lifecycleProof: report.lifecycleProof ?? null, browserProof: report.browserProof ?? null, binaryBrowserProof: report.binaryBrowserProof ?? null, projectReaderBrowserProof: report.projectReaderBrowserProof ?? null, cleanupRegressionProof: report.cleanupRegressionProof ?? null, checksumReviewProof: report.checksumReviewProof ?? null,
    nativeResourceCleanup: report.nativeResourceCleanup ?? null, finalResourceCleanup: report.finalResourceCleanup ?? null, cleanup: report.cleanup ?? null,
    limits: { coreTransferBytes: 536870912, concurrentTransfers: 4, aggregateStagingBytes: 2147483648, planSeconds: 300, fixtureProxyBytes: 16777216 },
    productionActions: false, wasabiQualified: false, buildCacheAdded: false,
  };
}
async function main(runID) {
  const f = await load(runID), snapshots = [];
  for (const stage of ['retry-before', 'installed', 'data', 'before-restart', 'restart', 'resume', 'update', 'rollback']) {
    const path = f.path('runtime-' + stage + '.json'); if (existsSync(path)) snapshots.push({ stage, values: await f.json(path) });
  }
  const receipt = existsSync(f.path('install-retry-receipt.json')) ? await f.json(f.path('install-retry-receipt.json')) : existsSync(f.path('install-receipt.json')) ? await f.json(f.path('install-receipt.json')) : null;
  const value = publicEvidence(f.report, f.custody, receipt, snapshots);
  const artifacts = await f.json(f.path('artifacts.json'));
  value.signedReleaseIdentity = { signerPublicKeySha256: sha256(JSON.stringify(artifacts.key.key)), releases: artifacts.releases.map(release => ({ pluginID: release.id, version: release.version, signedEnvelopeSha256: sha256(release.jws) })) };
  if (f.report.kernel) {
    const ca = new X509Certificate(await readPrivate(join(f.report.kernel, 'ca.crt'))), core = new X509Certificate(await readPrivate(join(f.report.kernel, 'core.crt')));
    const bootstrap = await f.json(join(f.report.kernel, 'bootstrap.json'));
    assert.equal(sha256(core.raw), bootstrap.coreCertificateSha256);
    value.kernelIdentity = { authorityCertificateSha256: sha256(ca.raw), coreCertificateSha256: sha256(core.raw), oneKernelAuthorityAndCoreLeaf: true };
  }
  value.privateReportSha256 = sha256(await readPrivate(f.path('report.json')));
  const serialized = JSON.stringify(value, null, 2) + '\n';
  assert(!serialized.includes('X-Amz-') && !serialized.includes('postgresql:') && !serialized.includes('BEGIN PRIVATE KEY') && !serialized.includes('secretKeyHex'));
  const path = join(ROOT, `docs/verification/2026-10-25-object-storage-native-${runID}.json`);
  await writeFile(path, serialized, { flag: 'wx' }); process.stdout.write('native_object_storage_evidence_written ' + runID + '\n');
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main(process.argv[2]).catch(() => { process.stderr.write('native_object_storage_evidence_failed\n'); process.exitCode = 1; });
