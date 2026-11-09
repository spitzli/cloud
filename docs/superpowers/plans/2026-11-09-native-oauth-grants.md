# Native OAuth authorization and consent

> Execute inline, one final fresh review, preserve protected unrelated work.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

**Goal:** Port authorization/consent request creation, detail reads and one-use decisions to native PostgreSQL. Retain the existing OAuth engine and interoperable request format; code exchange/refresh/private signing remain explicit pending their ports.

**Architecture:** An explicit native grants capability composes native security/reads/coordinated IDs through Core's existing pools. Client/canonical ownership precedes request/resource locks. Discover immutable request identity before fencing, reload current expiry/payload/owner after waits, retain policy fences, and confirm writes before returning codes/consent. Bounded expired-request cleanup skips locked rows rather than waiting behind other users.

## Constraints / review focus

- Preserve existing engine PKCE, exact redirects, scopes, prompt/login/max-age, nonce/state, issuer parameter and consent scope/resource containment semantics; do not write a new OAuth engine.
- Derive canonical identity from verified signed cookies; revalidate current provider/MFA/tenant/billing authority within the writer transaction and after waits. Tenant preview cannot issue grants.
- Consent request must match exact subject/session/client/immutable payload, be live after waits, and be consumed once. Approval persists only approved scope/resource consent and a new code; denial returns original state/error/issuer and creates no code.
- Follow client → canonical user/session → request/resource order; serialize consent decisions per owner to avoid duplicate consent writes. No second connection replacing caller-owned authority; existing allocator remains coordinated and native.
- Expired cleanup is bounded and skips locked rows. Errors/cancellation/uncertainty roll back pending state, no successful response before confirmed commit, no retained fallback/retry.
- No schema/grant/key/credential/provider changes, default activation or public builds. app_binding stays SELECT-only. No complete-driver or performance/CU gain claim.

### Task 1: Native authorization/consent

- [x] Owned PostgreSQL RED tests with real PKCE and disconnected retained grant writer.
- [x] Implement typed native request/consent lifecycle using existing engine and authority helpers.
- [x] Wire explicit Core capability and actual HTTP native grant creation/legacy exchange interoperability.
- [x] Clean max75 targeted/regression/Clippy/format/contracts/locked metadata; one final review.
- [ ] Publish signed Blacksmith PR, integrate when green; continue code exchange/refresh and product stores.
