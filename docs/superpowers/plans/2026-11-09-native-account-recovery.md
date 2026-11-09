# Native account recovery

> Execute inline with one fresh final review. Preserve unrelated work.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

**Goal:** Move the four owned recovery entry points to explicit tokio-postgres
transactions: request reset, queue activation, dispatch delivery, complete reset.

**Architecture:** Add an independent opt-in recovery pool capability on
IdentityService and `WEBDOCK_ACCOUNT_RECOVERY_BACKEND=tokio-postgres` in Core,
defaulting to the retained implementation. Reuse the shared max4/min0 bootstrap
pool, native allocator, cryptography, cookie policy and Delivery interface.
No ORM adapter, new dependency, schema/grant changes or production activation.

## Constraints and review focus

- Same public acknowledgement for unknown/disabled accounts; normalize email.
  Managed mappings use the existing Neon reset API with no local fallback.
- Local reset binds the exact current account email and credential digest.
  Reject banned, externally managed, mapped, ambiguous, expired or replayed proof.
  Recheck expiry after locks and expensive hashing; retain MFA/passkeys.
- One request per minute per locked canonical user; recovery token is hashed,
  delivery payload encrypted. Token and job admission commit atomically.
- Delivery claims use SKIP LOCKED, bounded attempts, lease and generation fencing.
  Release DB connection before external delivery; preserve stable delivery IDs
  and at-least-once behavior. Never report success for an unconfirmed DB receipt.
- Explicit parameter types, checked cleanup and bounded DB work; cancellation or
  uncertainty never retries through the retained database.

## Task 1: Recovery persistence and Core wiring

- [x] RED: owned restricted PostgreSQL18 fixture, disconnected retained connection;
  activation → delivery → reset → one-use denial with session/MFA-challenge
  retirement, unchanged MFA/passkeys and preserved foreign tenant data.
- [x] Port all four public boundaries and Core's explicit capability.
- [x] Exercise throttle/concurrency, managed mapping refusal/no local fallback,
  wrong email/digest/expiry, failed commit, canceled locks and delivery retry.
- [x] Verify actual Core reset HTTP plus config, existing native identity/browser/
  OAuth regressions, strict Clippy/fmt/contracts against clean max75 source.
- [x] One fresh final review; fix material findings RED→GREEN, document scope.
- [ ] Publish signed Blacksmith PR; integrate only its exact passing head.

The larger identity/domain rewrite, measured S3 work and production customer E2E
remain separate pending work. This increment does not qualify the full platform.
