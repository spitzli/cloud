import test from 'node:test';
import assert from 'node:assert/strict';
import { cachedLanding, invalidateLanding } from './landing-cache';

test('landing cache reuses, expires, settles after saves and drops failures', async () => {
  invalidateLanding(-10_000);
  let calls = 0;
  const load = async () => ++calls;
  assert.equal(await cachedLanding(load, 0), 1);
  assert.equal(await cachedLanding(load, 59_999), 1);
  assert.equal(await cachedLanding(load, 60_000), 2);
  // A save clears the entry and keeps reads uncached until the write has committed.
  invalidateLanding(60_000);
  assert.equal(await cachedLanding(load, 60_001), 3);
  assert.equal(await cachedLanding(load, 64_999), 4);
  assert.equal(await cachedLanding(load, 65_000), 5);
  assert.equal(await cachedLanding(load, 65_001), 5);
  invalidateLanding(-10_000);
  await assert.rejects(cachedLanding(() => Promise.reject(new Error('unavailable')), 0));
  assert.equal(await cachedLanding(load, 1), 6);
});
