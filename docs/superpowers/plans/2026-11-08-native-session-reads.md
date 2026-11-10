# Native session reads and signed tenant scope

> **For agentic workers:** Use superpowers:executing-plans inline. Preserve unrelated dirty Native-Git work.

**Goal:** Move actual canonical cookie/session authentication and the signed tenant-context boundary to direct PostgreSQL operations.

**Architecture:** Add an explicit native session reader to the existing IdentityService, using native typed rows/parameters and the checked pool from PR85. Keep the canonical Principal and cookie/MFA/passkey contracts. Share signing payload generation between retained and native transactions. Core opts in with `SPITZLI_SESSION_READ_BACKEND=tokio-postgres`; default remains `sea-orm` while other identity writes are ported.

**Tech Stack:** Axum, tokio-postgres/Deadpool, existing PostgreSQL policy functions and HMAC signer.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Global constraints

- No migrations, provider calls, credential changes, production activation or public build admission changes.
- Preserve authorization, setup/MFA restrictions, expiry/banned state, managed-provider revision/session/credential revocation, tenant preview and acting scope.
- Native uncertainty fails closed; never fall back to the legacy reader after opting in. Established authoritative SQL functions must exist.
- Bounded complete operation (30s), scoped transactions, verified TLS and discard on unconfirmed completion. Native read pool max4/min0, no idle SQL polling.
- Preserve original protected Native-Git files and reviewed original-root atomic-agent hashes. Clean release export excludes untracked migration76.
- Own GitHub CI uses Blacksmith. No new 1Password prompts; signed publication through existing authorized GitHub identity only.

## Review focus

- Missing policy functions, wrong role or changed permissions must fail closed rather than grant a session.
- Changes to provider session, binding revision, credential and MFA state must remain authoritative on every read.
- A signed Principal can become stale: SQL RLS must recheck current session and tenant scope independently.
- No token, signature, connection URL or credential may enter errors/logs/Debug; no native-to-ORM query translator.
- Cancellation/withheld replies and dirty scopes must not return pooled authority to another request.

### Task 1: Native reader and signer

- [x] Write failing owned PostgreSQL tests for cookie parity (valid/invalid/expired/banned/setup/operator MFA), managed-provider revocation, and native signed RLS tenant isolation/revocation/cleanup.
- [x] Implement a native reader with one parameterized canonical session query using the established local/provider authority functions and direct typed row decoding. Keep the complete client operation bounded.
- [x] Implement native RlsSigner application using the same signed payload and transaction-local settings; leave existing callers compatible.
- [x] Run owned restricted PostgreSQL tests and identity regressions. Expected: same Principal/denial behavior and no authority leakage.

### Task 2: Core wiring and qualification

- [x] Add failing default/opt-in/unknown-config tests and actual Core process HTTP session test.
- [x] Wire only the opted-in native session pool; keep other identity writes explicit legacy paths.
- [x] Qualify clean max75 source, actual authenticated HTTP, policy/identity/Core regressions, Clippy, formatting and all independent locked metadata. Measure the actual session read path without claiming whole-API/CU-hour improvements.
- [x] Review this increment once (no material findings).
- [ ] Publish signed PR, await Blacksmith and merge when green.
- [ ] Continue remaining native identity/session write and product-service ports, then measured streaming storage improvements.
