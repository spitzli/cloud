import { expect, test } from 'bun:test';
import { cachedLanding, invalidateLanding } from './landing-cache';

test('reuses, expires, invalidates and drops failures', async () => {
  invalidateLanding();
  let calls = 0;
  const load = async () => ++calls;
  expect(await cachedLanding(load, 0)).toBe(1);
  expect(await cachedLanding(load, 59_999)).toBe(1);
  expect(await cachedLanding(load, 60_000)).toBe(2);
  invalidateLanding();
  expect(await cachedLanding(load, 60_001)).toBe(3);
  invalidateLanding();
  await expect(cachedLanding(() => Promise.reject(new Error('x')), 0)).rejects.toThrow();
  expect(await cachedLanding(load, 1)).toBe(4);
});
