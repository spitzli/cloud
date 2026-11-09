# Native scoped-automation session storage

> Execute inline with superpowers:executing-plans; preserve protected Native-Git work.

**Goal:** Complete the trusted scoped-automation issuance/revocation part of the native session store used by workspace API tokens and signed logout continuations.

**Architecture:** Reuse the existing native management capability/pool and coordinated ID allocator. Direct typed SQL creates an opaque canonical session, returns only its confirmed ID, and revokes only a matching user/session. Existing AccessService/workspace/bearer authorization stays authoritative; these backend methods do not authenticate bare IDs. Retained mode remains compatible.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Constraints and review focus

- No new flag, pool, dependency, schema, credentials, production activation or public build admission.
- Preserve 1..366-day TTL and nonempty/120-byte label bounds; never return/log raw canonical tokens.
- Preserve the database's scoped automation exemption from interactive Managed Auth, without granting an unbound session or an interactive operator login.
- Require the existing explicit durable allocator in native mode; no compatibility allocator stub or native-to-ORM translation.
- Bound operations30s and return success only after confirmed commit; uncertain/canceled operations cannot release a successful ID/result or retry automatically.
- Native trusted revocation must constrain both user and session; public APIs still need authenticated request provenance and workspace rights.
- The separately running immutablea403 Core security release must not include this working tree.

### Task 1: Native automation session lifecycle

- [x] Write failing owned restricted PostgreSQL tests proving actual native issuance/revocation (legacy DB deliberately disconnected), input bounds, finite expiry, token opacity, foreign-user refusal and scoped managed-provider enforcement.
- [x] Implement native paths behind the existing management capability; retain default callers.
- [x] Run the owned integration plus existing management/MFA/HTTP regressions; verify cancellation/unknown-commit behavior through the shared guard and no implicit retries.
- [x] Qualify clean max75 export and affected regression/Clippy/format gates; fresh final review found no material findings.
- [ ] Finish contracts/locked metadata and signed publication.
- [ ] Publish a signed Blacksmith PR and integrate when green; continue broader identity/product persistence ports.
