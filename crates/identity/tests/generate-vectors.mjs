// Run from the repository root with Node; only synthetic values are used.
import { scryptSync, createHmac, createHash } from 'node:crypto';
import { createOTP } from '@better-auth/utils/otp';
import { xchacha20poly1305 } from '@noble/ciphers/chacha.js';
const secret = 'fixture-secret-with-at-least-32-bytes';
const password = 'CaféÅ-password-123';
const salt = '000102030405060708090a0b0c0d0e0f';
const nonce = Buffer.alloc(24, 7);
const plain = '["abcde-fghij","12345-67890"]';
console.log(JSON.stringify({
  password,
  totp: await createOTP('0123456789abcdefghijklmnopqrstuv').hotp(56666666),
  hash: `${salt}:${scryptSync(password.normalize('NFKC'), salt, 64, { N: 16384, r: 16, p: 1, maxmem: 67108864 }).toString('hex')}`,
  cookie: encodeURIComponent(`test-session-token.${createHmac('sha256', secret).update('test-session-token').digest('base64')}`),
  encrypted: Buffer.concat([nonce, Buffer.from(xchacha20poly1305(createHash('sha256').update(secret).digest(), nonce).encrypt(Buffer.from(plain)))]).toString('hex'),
}));
