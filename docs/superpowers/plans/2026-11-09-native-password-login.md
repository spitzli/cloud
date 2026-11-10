# Native password and MFA login

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

Port the complete owned password → optional MFA → canonical session flow to
typed tokio-postgres operations. Keep the pinned Neon HTTP transport; port its
mapped login/enrollment and pending step-up provenance persistence as part of the
same boundary. No ORM adapter, new dependencies, schema/grants, credential changes
or production activation. Work inline and verify the connected change before PR.

## Implementation

- [x] Add disconnected-retained-DB PostgreSQL tests for real password/MFA/session
  issuance, invalid credentials, setup/operator gates, replay and commit failure.
- [x] Implement native login, provider provenance and MFA modules. Reuse existing
  crypto, TOTP, cookie, allocator, provider policy and checked transaction helpers.
  Expose `with_native_password_login(Pool)` and explicit Core
  `SPITZLI_PASSWORD_LOGIN_BACKEND=tokio-postgres` (default `sea-orm`). Native IDs,
  reads and session management are dependencies; other capabilities are retained.
- [x] Test current credential, bans, migration/provider policy and expiry after
  lock waits; rate limits, recovery codes, TOTP replay, cancellation and rollback.
  Native provider-result tests must exercise real database facts and encrypted
  custody without introducing a production TLS exception or test-only endpoint.
- [x] Exercise actual Core HTTP password → MFA → session and recovery composition;
  run relevant native/default regression, Clippy, formatting and static gates on
  a clean max75 archive excluding the unrelated local draft migration76.
- [ ] Review the connected diff, document exact coverage/limits and publish a
  server-signed Blacksmith PR. Merge only the exact passing head.

## Security and operation constraints

Keep missing-account password verification equally expensive. Rate admission is
durable and bounded; no connection is held during local password hashing. Lock
canonical identity before credential/challenge/factor mutation. Refresh current
authority after waits, preserve credential digests, provider binding revision and
five-minute one-use MFA proof, durable failure counters and TOTP/recovery replay.
Do not grant an operator business access before MFA setup. Confirm session commit
before returning its signed cookie and release the connection before rereading,
including max1 pools. Provider calls retain their existing fixed HTTPS origin,
deadline, no redirect/proxy behavior and no local fallback/adoption of an existing
provider user. Uncertainty cannot retry through the retained database.

Passkey ceremonies/security management/offline provisioning and other identity or
domain stores remain separate ports. Live Neon HTTP, real customer production
login, full-platform E2E and measured performance gains require separate evidence.
