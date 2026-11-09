# Native OAuth token introspection

> Execute inline with superpowers:executing-plans; preserve unrelated protected work.

**Goal:** Move complete owned token introspection and its caller-owned boundary onto direct PostgreSQL, preserving opaque/refresh/legacy-JWT, first-party preview and resource-grant semantics.

**Architecture:** Add explicit native introspection capability using the shared checked pool and client domain loader. Native domain records/queries preserve discovery and refresh-compatible lock order. Share pure signature/claim normalization with retained code; do not translate generic ORM statements. Existing grant issuance and retained caller-owned APIs remain explicit pending their ports.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Global constraints / review focus

- No schema, keys, credentials, dependency upgrades, production defaults or public build admission changes.
- Inactive discovery never revives; disappearing/replaced access discovery never uses JWT fallback. Compare immutable identity after canonical reload.
- Preserve client → user → session → token ownership order, deterministic resource/grant order and fences through caller completion.
- Preserve exact client-secret verification, issuer/audience, caller-resource linking, scope/role ceilings, current tenant/provider/billing permissions and narrow portal-preview exception.
- Native uncertainty fails closed; bounded operations/transaction cleanup, no automatic retained fallback or retry.
- Recheck current expiry after waits. Native reads do not generate/rotate signing keys or issue/refresh tokens.
- Share one Core native authentication pool; existing retained transaction-aware APIs remain compatible until replaced.

### Task 1: Native introspection

- [x] Write failing owned PG18 parity tests using actual internal OAuth issuance, disconnected retained reader, opaque/refresh/JWT state, provider/tenant/client/resource revocation and fences.
- [x] Implement direct typed native record loading, immutable reload checks, policy queries and shared pure crypto/claim helpers.
- [x] Wire explicit native public/portal/MCP/userinfo and consuming caller-owned APIs without bypassing existing authorities.
- [x] Qualify Core configuration/actual HTTP and clean max75 regression/Clippy/format/contracts/locked metadata gates.
- [ ] Review once, fix material findings via RED/GREEN, publish signed Blacksmith PR and integrate when green; continue issuance/security/product storage.
