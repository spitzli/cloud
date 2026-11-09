# Native browser session authority

> Execute inline, one final fresh review; preserve unrelated protected work.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

**Goal:** Move browser references/revalidation and server-owned stored authority onto checked native caller-owned transactions, retaining exact client, origin, canonical identity, token, role/scope/read-only/customer ceilings and revocation.

**Architecture:** Explicit browser-authority capability implies native browser sidecar and OAuth inspection/client reads using Core's shared bootstrap pool. Use discovery then client/canonical/token/browser fencing, immutable sidecar comparison and fresh expiry after waits. Consume/return caller-owned native transactions; no second pool connection or generic ORM translation. Share pure browser claim/principal/reference policy with retained code. OAuth issuance/refresh/logout writes and retained caller-owned APIs remain explicit until their ports.

## Constraints / review focus

- Existing signatures, cookie MACs/compatibility windows, origin allowlists and exact subject/session/client proof remain authoritative.
- A sidecar is not a user grant: decrypt protected tokens, revalidate actual OAuth/canonical/provider/tenant authority, retain fences through caller completion.
- Discovery that disappears, changes ownership/payload or expires while waiting cannot authorize. Recheck expiry after all waits; initial inactive cannot revive.
- Preserve role/scope/read-only/customer ceilings and narrow first-party preview. Stored references are server-owned proof, not accepted unauthenticated IDs.
- Preserve client → user → session → token → browser sidecar order. First-party stored-authority native revalidation must not acquire canonical identity before client authority.
- No schema, grants, key/provider/credential changes, public build opening, production activation or measured performance claim. app_binding stays SELECT-only.
- Shared max4/min0 native Core authentication pool; bounded operations, cancellation/uncertainty fail closed, no retained fallback.

### Task 1: Native browser proof

- [x] Owned PostgreSQL RED tests prove actual signed browser/OAuth cookies with disconnected retained reader and consuming stored-authority fences.
- [x] Implement direct native reference/principal/revalidation and shared pure claim/ceiling policy; keep retained transaction APIs explicit.
- [x] Core opt-in/default/implication tests and actual HTTP proof with restricted roles.
- [x] Clean max75 targeted/regression/strict Clippy/format/contracts/locked metadata; one final review and material RED/GREEN fixes if any.
- [ ] Publish signed Blacksmith PR, integrate when green; continue remaining services/storage.
