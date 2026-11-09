# Native OAuth token exchange and refresh

> Execute inline, one final fresh review; preserve protected unrelated work.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

**Goal:** Port code exchange, access/refresh persistence, refresh rotation/replay and private signing reads onto direct PostgreSQL, completing the owned OAuth HTTP lifecycle while retaining offline setup and caller-owned domain APIs until ported.

**Architecture:** Highest explicit token capability composes native grants/security/reads/IDs through existing pools. Reuse the protocol engine, pure ID-token claim/signing helpers and typed domain records. Discover then fence canonical identity before code/refresh rows; fresh immutable reload/expiry after waits. Refresh serializes owner before replay/rotation and commits family revocation before returning invalid_grant. Load only offline-provisioned encrypted private keys, fence/recheck key eligibility and retain through confirmed commit.

## Constraints / review focus

- Preserve client-secret/method, PKCE, exact redirect, code one-use, scope ceilings, resource TTL minima, nonce/auth_time/at_hash and same signing/encryption formats. No new OAuth implementation or key generation on requests.
- No credential response before confirmed token writes/code consumption/rotation commit; errors/cancellation roll back the complete operation, uncertain completion never retries or falls back.
- Code and refresh ownership/payload/session/resource/time identity immutable after discovery; inactive cannot revive. Canonical/provider/MFA/tenant/billing authority and deadlines are fresh after waits/pending writes; preview cannot mint tokens.
- Follow client→canonical user/session→code/refresh→resource/key ordering. Refresh owner-exclusive serialization and durable replay-family revocation coexist with native security and retained consumers.
- Private keys never leave trusted signer; selection is deterministic, encrypted material/current expiry rechecked after key waits and before commit. Retain key fence; no schema/grants/credentials/provider/default/public-build changes.
- Reuse shared max4/min0 native pool plus existing coordinated ID allocator. Offline provisioning and retained caller-owned/MCP/domain boundaries remain explicit. No full-platform or performance/CU gain claim.

### Task 1: Native token lifecycle

- [x] RED owned PostgreSQL tests with disconnected retained token writer and actual PKCE/JWT verification.
- [x] Implement direct native code/token/refresh state and shared pure signing/claim helpers.
- [x] Core explicit capability and actual HTTP full native OAuth flow/refresh proof.
- [x] Clean max75 targeted/regression/Clippy/format/contracts/locked metadata, one final review and material RED/GREEN fixes.
- [ ] Publish signed Blacksmith PR, integrate when green; continue remaining native domain stores and measured storage work.
