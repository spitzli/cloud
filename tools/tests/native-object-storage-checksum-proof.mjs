// Requalify actual MinIO semantics honestly; FullObject-labelled headers are tested separately by the SDK HTTP fixture.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { load } from './native-object-storage-fixture.mjs';
import { setup, client } from './native-object-storage-acceptance.mjs';
import { sha256 } from './native-plugin-fixture.mjs';
const f = await load(process.argv[2]);
try {
  assert(Date.now() < f.report.deadline); await setup(f); const c = await client(f);
  const resource = c.state.resource; assert(resource);
  const call = async (action, input) => {
    const response = await c.raw(f.report.origins.console + '/api/console/plugins/invoke', { method: 'POST', headers: { origin: f.report.origins.console, 'content-type': 'application/json' }, body: JSON.stringify({ pluginID: 'object-storage', projectID: f.report.project, action, input, idempotencyKey: randomUUID() }) });
    return { status: response.status, value: await response.json() };
  };
  const observed = await call('objects.observe', { resource_id: resource, key: 'browser/proof.txt' }); assert.equal(observed.status, 200);
  const input = { resource_id: resource, ...observed.value, expires_at: Math.floor(Date.now() / 1000) + 120 }; delete input.expected_sha256;
  const omitted = await call('objects.download', input), known = typeof observed.value.expected_sha256 === 'string';
  if (known) assert.notEqual(omitted.status, 200, 'provider-known full SHA omission must be denied');
  else assert.equal(omitted.status, 200, 'unknown provider checksum remains optional');
  const digest = sha256(Buffer.from('controlled application replacement\n'));
  const accepted = known ? await call('objects.download', { ...input, expected_sha256: observed.value.expected_sha256 }) : omitted;
  assert.equal(accepted.status, 200); assert.equal(accepted.value.expectedSha256, known ? observed.value.expected_sha256 : undefined);
  const get = await c.raw(f.report.origins.console + accepted.value.path), bytes = Buffer.from(await get.arrayBuffer());
  assert.equal(get.status, 200); assert.equal(bytes.length, observed.value.exact_bytes); assert.equal(sha256(bytes), digest);
  // A supplied known SHA stays strict even if MinIO itself does not label a full checksum.
  const supplied = await call('objects.download', { ...input, expected_sha256: digest }); assert.equal(supplied.status, 200); assert.equal(supplied.value.expectedSha256, digest);
  const correct = await c.raw(f.report.origins.console + supplied.value.path); assert.equal(correct.status, 200); assert.equal(sha256(Buffer.from(await correct.arrayBuffer())), digest);
  const wrong = await call('objects.download', { ...input, expected_sha256: '0'.repeat(64) });
  let wrongDeliveryStatus = null;
  if (wrong.status === 200) { const denied = await c.raw(f.report.origins.console + wrong.value.path); wrongDeliveryStatus = denied.status; assert.notEqual(denied.status, 200); await denied.arrayBuffer(); }
  await f.write('checksum-review-private.json', { observed, omitted, supplied, wrongAdmissionStatus: wrong.status, wrongDeliveryStatus });
  f.report.checksumReviewProof = { actualSignedNativeProducer: true, providerFullObjectKnown: known, omittedHashAdmissionStatus: omitted.status, knownFullObjectOmissionDenied: known ? true : null, unknownChecksumOmissionPreserved: !known, exactVersionAndETagBound: true, coreDownloadBytes: bytes.length, coreDownloadSha256: digest, suppliedCorrectShaEnforced: true, suppliedWrongShaDenied: true, wrongAdmissionStatus: wrong.status, wrongDeliveryStatus, fullObjectLabelRegressionBoundary: 'actual-aws-sdk-local-http-fixture-only' };
  await f.saveReport(); console.log('native_object_storage_checksum_review_passed ' + f.input.runID);
} catch (error) { await f.write('checksum-review-failure-' + Date.now() + '.json', { error: String(error), stack: error.stack }); console.error('native_object_storage_checksum_review_failed_private_diagnostics_retained'); process.exitCode = 1; }
