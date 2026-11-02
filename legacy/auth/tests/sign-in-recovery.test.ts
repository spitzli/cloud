import test from 'node:test';
import assert from 'node:assert/strict';
import { signInError, SIGN_IN_EXPIRED } from '../src/lib/sign-in-recovery';

test('provider signature errors offer a fresh flow without accusing the password', () => {
  assert.equal(signInError({error:'invalid_signature',status:400}, 'Password failed'), SIGN_IN_EXPIRED);
  assert.equal(signInError({code:'invalid_signature'}, 'Code failed'), SIGN_IN_EXPIRED);
  assert.equal(signInError({message:'Invalid email or password'}, 'Fallback'), 'Invalid email or password');
  assert.equal(signInError(null, 'Fallback'), 'Fallback');
});
