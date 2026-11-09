# Native OAuth client and public-key store

> Execute inline with superpowers:executing-plans. Preserve protected work.

**Goal:** Port first-party OAuth client validation and public JWKS reads to direct PostgreSQL, including a caller-owned native client-validation boundary for subsequent transaction ports.

**Architecture:** Decode native client rows into the existing domain model and reuse credential verification/portal rules. Share public-JWK normalization between retained/native readers. An optional native store serves owned read transactions; a consuming native `_in` method retains the canonical client lock and checks binding with retained SELECT-only privileges. Token issuance, introspection and existing caller-owned retained transactions remain explicit until their later ports.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Constraints and review focus

- Preserve constant-time client-secret verification, auth-method matching, exact redirect allowlists, disabled/DPoP/subject-type refusal, enabled portal binding and required OIDC scopes.
- Client and binding changes during the client lock wait must be observed; retain the canonical client fence until caller completion. Binding checks keep existing SELECT-only rights; validation alone is not a lasting resource-access grant.
- Public JWKS never returns private material, ignores unsupported key types and retains key IDs/RS256/use metadata.
- No key generation/rotation, schema/provider/credential changes, dependency upgrades or production default activation.
- Native uncertainty fails closed with bounded operations, no automatic retained fallback or SQLx/ORM statement translation.
- Existing caller-owned legacy validation/introspection remains compatible; no native connection crosses unrelated OAuth/identity operations.

### Task 1: Native client/key reads

- [x] Write failing owned restricted PostgreSQL tests using disconnected legacy DB for actual native validation/JWKS, credential/redirect/binding/scope/disabled refusal and lock fences.
- [x] Implement explicit domain-native loaders, shared verification/normalization and consuming native transaction API.
- [x] Add explicit Core flag/wiring using the shared max4/min0 bootstrap pool; qualify actual HTTP public JWKS/native read where appropriate.
- [x] Run clean max75 owned/regression/Clippy/format and locked metadata gates; fresh review found no material findings.
- [ ] Complete static contracts and signed publication.
- [ ] Publish signed Blacksmith PR, integrate when green and continue native token/introspection/identity/product storage.
