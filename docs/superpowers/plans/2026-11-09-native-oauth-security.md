# Native OAuth revocation and logout

> Execute inline with one final fresh review; preserve protected unrelated work.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

**Goal:** Move OAuth token revocation and end-session writes onto checked native PostgreSQL, preserving client/issuer ownership, immutable discovery, signed logout hints, exact browser identity and confirmed completion.

**Architecture:** An explicit native security capability includes native OAuth reads, coordinated IDs and cookie session reads using existing shared pools. Reuse public signature verification and existing protocol helpers. Revocation discovers records then locks client → sorted canonical owners/sessions → access/refresh records before immutable reload/update; signed legacy access tokens receive revocation tombstones. Logout serializes the verified canonical owner before deleting the exact session, retaining cascade order. No generic ORM translation, request-time key generation or uncertain retry.

## Constraints / review focus

- Preserve constant-time client-secret authentication and exact client ownership. Unknown/invalid/foreign tokens remain idempotent RFC revocation outcomes; never mutate another client's records.
- Revocation is monotone and confirmed before success. Immutable owner/record changes during waits fail closed; deterministic owner/token order must coexist with refresh/account/session deletion and native readers.
- Legacy JWT tombstones require valid RS256/issuer/time, own azp/client_id and existing canonical subject, with session FK only when it belongs to that subject. Preserve scope/resource/time fields and unique conflict ownership.
- Logout accepts expired signed ID-token hints as before, requires exact current signed-cookie subject/session, enabled client end-session and exact post-logout redirect/state bounds. No foreign session deletion or cookie clearing before confirmed delete.
- No schema/grant/key/provider/credential changes or production activation/public builds. app_binding remains SELECT-only. No performance/CU improvement claim.
- Native errors/cancellation/uncertain commit fail closed, bounded operations and checked pool cleanup, no retained fallback. Existing issuance/refresh/consent and caller-owned APIs remain explicit pending ports.

### Task 1: Native security writes

- [x] RED owned PostgreSQL tests using actual OAuth issuance and disconnected retained writer.
- [x] Implement typed revocation/logout with shared pure/public verification and coordinated native IDs.
- [x] Add explicit Core configuration/shared-pool wiring and actual HTTP revocation proof.
- [x] Clean max75 targeted/regression/Clippy/format/contracts/locked metadata and one final review.
- [ ] Publish signed Blacksmith PR, integrate when green, continue remaining rewrite/storage.
