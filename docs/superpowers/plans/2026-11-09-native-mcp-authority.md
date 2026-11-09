# Native MCP authority references

> Execute inline, one final fresh review; preserve unrelated protected work.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

**Goal:** Native owned bearer-to-MCP reference authentication and consuming caller-owned revalidation, preserving exact access-record identity, scope/role ceilings and live resource/canonical proof through domain completion. Existing retained request-owned guards remain explicit until their domain ports.

**Architecture:** Reuse native typed access discovery/fencing/inspection and share pure access fingerprint/MCP scope rules with retained code. Native caller-owned methods consume/return checked transactions, avoiding self-referential connection/transaction ownership. Explicit additive MCP capability composes existing OAuth capabilities with one Core bootstrap pool. No new transaction manager, ORM adapter or unsafe lifetime mechanism.

## Constraints / review focus

- Exact access row required; bare legacy JWT, refresh token, raw user IDs or cached claims cannot create durable MCP authority.
- Client credentials and current caller/issuer eligibility/linking, canonical/provider/MFA/tenant authority, audience/namespace/scope/role/≤300second lifetime remain current and capped.
- Immutable fingerprint byte-compatible with retained records; revalidation checks access ID/hash/record, original caller/issuer/subject/session/resource/role/write/iat/exp and current state after waits.
- Preserve client→canonical→access→resource/grant order and returned fences through caller domain completion. No second pool connection replacing caller-owned authority, synthetic sessions or preview grant.
- Native uncertainty/cancellation/deadlines fail closed with checked cleanup, no retained retry/fallback. No schema/grant/credential/key/default/public-build or performance/CU claim changes.
- Core shares max4/min0 pool; additive capability preserves existing enabled native lifecycle. Retained guards/domain consumers remain explicit, not falsely claimed migrated.

### Task 1: Native MCP authority

- [x] RED real restricted PostgreSQL tests using native-issued MCP tokens/disconnected retained reader and retained/native proof interoperability.
- [x] Implement shared pure fingerprint/scope policy plus typed native owned/consuming boundaries.
- [x] Explicit Core capability and relevant actual HTTP evidence.
- [x] Clean max75 targeted/regression/Clippy/format/contracts/locked metadata; one final review.
- [ ] Publish signed Blacksmith PR, integrate when green; port actual remaining domain/identity stores and measure storage.
