import test from 'node:test';
import assert from 'node:assert/strict';
import { nativeMailGrants } from '@webdock/mail-core/schema';

test('managed Mail grants work without provisioning an unused Docker worker role', () => {
  const sql = nativeMailGrants('webdock_auth_runtime');
  assert.match(sql, /webdock_mail.instance/);
  assert.doesNotMatch(sql, /worker|undefined/);
  assert.throws(() => nativeMailGrants('bad"role'));
  assert.throws(() => nativeMailGrants('auth', 'auth'));
  assert.match(nativeMailGrants('auth', 'local_worker'), /TO "local_worker"/);
});
