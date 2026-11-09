# Native browser flow storage

> Execute inline with superpowers:executing-plans. Preserve unrelated protected work.

**Goal:** Move durable first-party login/logout nonce and session-sidecar storage to direct PostgreSQL while retaining existing OAuth, PKCE, nonce, client, origin and scope authorities.

**Architecture:** Add an explicitly optional checked native store to BrowserSessions with domain-specific typed operations. Crypto verification/flow types stay shared; state/origin validation runs while the nonce row is locked before confirmed consumption. Legacy caller-owned OAuth/introspection/revalidation remains explicit until its own port. No ORM/SQLx statement translation or provider network dependency is added.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Constraints and review focus

- No schema, credential, dependency upgrades, production default activation or public build opening.
- Preserve PKCE, constant-time state checks, nonce, issuer/client/origin, role/scope/expiry ceilings, one-time callback and explicit logout confirmation.
- Wrong state/origin/client cannot consume a valid flow; expiry after a row-lock wait cannot revive or consume authority.
- Encrypt sidecar payloads with existing keys, hash opaque references, never expose/log bearer/refresh tokens or kube/registry credentials.
- Native operations use bounded checked transactions, confirmed commit and fail-closed uncertainty; no automatic fallback after opt-in.
- Retained read-side compatibility and caller-owned transaction fences stay compatible; storage-only native mode must not claim complete OAuth migration.
- Include the confirmed parent security rollout proof, preserving its a403 artifact identity rather than relabeling it as this source.

### Task 1: Native durable browser storage

- [x] Write failing owned PostgreSQL tests for encrypted flow persistence, wrong-state/origin/client refusal, concurrent one-time consumption, post-lock expiry, bounded pruning and logout proof replay.
- [x] Implement domain-specific native store operations and wire them into existing BrowserSessions storage points without changing authentication authorities.
- [x] Add explicit Core configuration/wiring and actual HTTP login-begin/nonce persistence proof where compatible with the owned OAuth fixture.
- [x] Qualify clean max75 source, browser/OAuth/identity/Core regressions, Clippy/format/contracts/locked metadata.
- [x] Review once and fix logout-proof expiry after lock waits with native/retained RED/GREEN; clarify uncertain-commit cancellation documentation.
- [ ] Publish signed Blacksmith PR and integrate when green; continue remaining OAuth/security/product persistence ports.
