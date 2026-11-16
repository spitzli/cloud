// Process-local cache for the landing global; Payload runs in this process, so saves invalidate it directly.
const TTL_MS = 60_000;
let entry: { value: Promise<unknown>; expires: number } | undefined;

export function cachedLanding<T>(load: () => Promise<T>, now = Date.now()): Promise<T> {
  if (entry && entry.expires > now) return entry.value as Promise<T>;
  const value = load();
  const current = { value, expires: now + TTL_MS };
  entry = current;
  // Never keep a failed lookup; the next request retries.
  value.catch(() => { if (entry === current) entry = undefined; });
  return value;
}

export function invalidateLanding(): void {
  entry = undefined;
}
