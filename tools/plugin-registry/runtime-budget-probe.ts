import assert from 'node:assert/strict';
const fixtures = await Bun.file('/fixtures/fixtures.json').json();
const origin = 'http://127.0.0.1:3000';
const started = performance.now();
let maximumResponseMs = 0;
const forms = new Set<string>();
const validate = async (manifest: unknown, expected: boolean, kind?: string) => {
  const declared = (manifest as { kind?: string }).kind;
  const form = declared ?? kind ?? '(kindless)';
  if (!forms.has(form)) { forms.add(form); console.log('Checking schema form: ' + form); }
  const began = performance.now();
  const response = await fetch(origin + '/api/manifest/validate', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text: JSON.stringify(manifest), ...(kind ? { kind } : {}) }) });
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.equal(result.ok, expected, 'Unexpected manifest admission');
  maximumResponseMs = Math.max(maximumResponseMs, performance.now() - began);
};
for (let attempt = 0; ; attempt++) {
  try {
    const response = await fetch(origin + '/healthz');
    assert.equal(response.status, 200);
    assert.equal((await response.json()).ok, true);
    break;
  } catch {
    if (attempt >= 90) throw new Error('Fixture runtime did not become healthy');
    await Bun.sleep(500);
  }
}
const schemaResponse = await fetch(origin + '/manifest.schema.json?kind=extension');
assert.equal(schemaResponse.status, 200);
const effectiveSchema = await schemaResponse.json();
const schemaDefinitionCount = Object.keys(effectiveSchema.$defs ?? {}).length;
assert.ok(schemaDefinitionCount > 0, 'Effective schema must share generated local definitions');
let checked = 0;
for (let repetition = 0; repetition < 3; repetition++) {
  for (const manifest of fixtures.valid) { await validate(manifest, true); await validate(manifest, true, manifest.kind); checked += 2; }
  for (const manifest of fixtures.invalid) { await validate(manifest, false); await validate(manifest, false, manifest.kind); checked += 2; }
  await validate({ name: 'Invalid Legacy Display Name', version: '1.0.0', kind: 'unconfigured-fixture' }, false, 'unconfigured-fixture');
  checked++;
  await validate({ name: 'Invalid Legacy Display Name', version: '1.0.0' }, false);
  checked++;
}
await validate({ name: 'selector-fixture', version: '1.0.0', kind: 'provider' }, false, 'unconfigured-kind');
checked++;
assert.equal(forms.size, 4);
const status = await Bun.file('/proc/1/status').text();
assert.ok((await Bun.file('/proc/1/cmdline').text()).includes('\0--smol\0'));
assert.match(status, /^Uid:\s+65532\s+65532\s+65532\s+65532$/m);
assert.match(status, /^NoNewPrivs:\s+1$/m);
assert.match(status, /^Seccomp:\s+2$/m);
for (const field of ['CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb']) assert.match(status, new RegExp('^' + field + ':\\s+0+$', 'm'));
assert.equal(process.env.JSC_useJIT, 'false');
let readOnly = false;
try { await Bun.write('/app/.budget-write-probe', 'must not write'); } catch (error) { readOnly = (error as { code?: string }).code === 'EROFS'; }
assert.equal(readOnly, true);
await Bun.write('/tmp/private-budget-write-probe', 'temporary fixture');
await Bun.write('/data/budget-proof.json', JSON.stringify({ actualHttpApi: true, normalModeFixture: true, preparedExtensionAndProviderSchemas: true,
  validManifests: fixtures.valid.length, invalidManifests: fixtures.invalid.length, repetitions: 3, requestsChecked: checked,
  schemaDefinitionCount, effectiveSchemaBytes: JSON.stringify(effectiveSchema).length,
  uid: 65532, noNewPrivileges: true, defaultSeccompFilter: true, allCapabilitiesDropped: true, readOnlyRoot: true,
  privateDataAndTmpWritable: true, validatorCacheFormsExercised: forms.size, authoringSelectorMismatchRejected: true,
  elapsedMs: Math.round(performance.now() - started), maximumResponseMs: Math.round(maximumResponseMs),
  memoryMode: '--smol', jit: false, bun: Bun.version }, null, 2) + '\n');
console.log('Budget fixture assertions passed: ' + checked + ' HTTP admission requests');
