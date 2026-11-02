export const SIGN_IN_EXPIRED = 'This sign-in request has expired or is no longer valid. Start a new sign-in from Studio.';

// Only the provider decides whether a request is still valid; never trust the device clock.
export function signInError(error: unknown, fallback: string): string {
  if (!error || typeof error !== 'object') return fallback;
  const value = error as { error?: unknown; code?: unknown; message?: unknown };
  if (value.error === 'invalid_signature' || value.code === 'invalid_signature') return SIGN_IN_EXPIRED;
  return typeof value.message === 'string' && value.message ? value.message : fallback;
}
