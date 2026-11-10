import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { openSecret, sealSecret } from '../src/server/secrets.ts';

// Same public synthetic retained envelope the native Rust Mail reader must open.
const fixture = JSON.parse(readFileSync(new URL('../../../crates/mail/tests/node-compatibility.json', import.meta.url), 'utf8'));
test('source namespace changes preserve retained Mail secrets across Node and Rust', () => {
  assert.equal(openSecret(fixture.sealed, fixture.purpose, fixture.key), fixture.value);
  assert.throws(() => openSecret(fixture.sealed, fixture.purpose + ':other', fixture.key));
  const value = { text: 'Änderung', version: 1 };
  assert.deepEqual(openSecret(sealSecret(value, fixture.purpose, fixture.key), fixture.purpose, fixture.key), value);
});
