// Process-local cache for the landing global; Payload runs in this process, so saves invalidate it directly.
const TTL_MS = 60_000;
// afterChange runs before the save commits; do not cache reads that may still see the previous row.
const SETTLE_MS = 5_000;
type State = { entry?: { value: Promise<unknown>; expires: number }; settleUntil: number };
// Global state survives duplicated module instances (CMS config and page bundles may load this file separately).
const key = Symbol.for('spitzli.website.landing-cache');
const scope = globalThis as unknown as Record<symbol, State | undefined>;
const state: State = (scope[key] ??= { settleUntil: 0 });

export function cachedLanding<T>(load: () => Promise<T>, now = Date.now()): Promise<T> {
  const cached = state.entry;
  if (cached && cached.expires > now) return cached.value as Promise<T>;
  const value = load();
  if (now < state.settleUntil) return value;
  const current = { value, expires: now + TTL_MS };
  state.entry = current;
  // Never keep a failed lookup; the next request retries.
  value.catch(() => { if (state.entry === current) state.entry = undefined; });
  return value;
}

export function invalidateLanding(now = Date.now()): void {
  state.entry = undefined;
  state.settleUntil = now + SETTLE_MS;
}
