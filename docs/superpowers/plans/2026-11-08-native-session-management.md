# Native session management implementation

> **For agentic workers:** Execute inline with superpowers:executing-plans. Preserve unrelated Native-Git work.

**Goal:** Port actual self-service session listing, renewal, revocation and logout to native PostgreSQL and provide the locked native canonical-session boundary needed by subsequent service ports.

**Architecture:** Extend the existing optional native session backend with an explicit management capability; share one pool for reads/management. Direct typed SQL keeps canonical user/session row locks and checks local/provider authority in a fresh statement after those locks. The Core opts in using `WEBDOCK_SESSION_MANAGEMENT_BACKEND=tokio-postgres`, which also selects native cookie reads. Keep the read-only flag and legacy default compatible.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Global constraints

- No migrations, credentials, production/default changes, provider calls or public-build activation.
- Preserve exact cookie/Principal, setup/MFA, expiry, refresh threshold (15 minutes/eight hours), token stability, ownership and provider revocation behavior.
- Preserve canonical lock order and fresh authority checks after waiting for locks. Never trust cached Principal or pre-lock provider predicates for writes.
- Native management shares max4/min0 session pool; complete operations bounded30s, verified TLS/roles, uncertain transactions discarded. No fallback after opt-in, no idle SQL polling.
- Keep original protected Native-Git/root atomic-agent files unchanged. Clean export maximum migration75.
- Blacksmith only for own GitHub CI, existing server-signed publication, no new 1Password prompts.

## Review focus

- Setup accounts may manage their own sessions without gaining tenant/operator authority.
- Provider expiry/revision/credential changes while a request waits on a canonical row lock must be observed before mutation.
- Foreign session IDs/tokens must never authorize reads or deletion.
- Refresh must release its checked connection before rereading the canonical Principal; max1 qualification must not deadlock.
- Cancellation or uncertain commit must never report a successful renewal/revocation or return an unsafe connection.

### Task 1: Native management and locked authority

- [x] Write failing owned PostgreSQL tests for management parity/ownership, fresh-after-lock provider denial, locked caller revalidation and bounded cancellation.
- [x] Implement direct native list/refresh/revoke-other/revoke-owned/revoke-token/logout paths; retain legacy dispatch when disabled.
- [x] Implement consuming native caller-owned session revalidation with FOR SHARE and fresh authority checks, returning the checked transaction only on success.
- [x] Run owned tests including refresh throttle, setup behavior, provider/canonical revocation, challenge cleanup, row fences and connection release.

### Task 2: Core opt-in and qualification

- [x] Add failing default/opt-in/unknown config and actual HTTP session-management tests; management implies native reads on the same pool.
- [x] Wire the explicit management mode; default remains retained, read-only mode still supported.
- [x] Qualify clean max75 source, affected regressions, Clippy/format/contracts and all independent locked metadata.
- [x] Review once and fix the expiry-during-lock-wait finding with native/retained RED/GREEN regressions.
- [ ] Publish signed PR and await Blacksmith before normal merge.
- [ ] Continue remaining identity issuance/security and product-service persistence ports; performance/storage decisions remain measured.
