# Native PostgreSQL allocator implementation plan

> Execute inline with superpowers:executing-plans; owner authorised the full rewrite.

**Goal:** Introduce the bounded direct tokio-postgres runtime and port central ID allocation through it.

**Architecture:** Use maintained Deadpool for pooling and tokio-postgres transactions, not an ORM/SQL translation facade. A small ownership guard discards connections on cancelled or unconfirmed transactions; every transaction verifies role and clean session scope on its actual backend. Keep the existing allocator explicitly as the temporary legacy/offline path while the new Core path is opted in via `WEBDOCK_ID_BACKEND=tokio-postgres`.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Constraints and rulings

- No schema migration, credential rotation, new database or production activation.
- URL authority and verified TLS must not drift. TLS file references are bounded/owned; credentials never enter errors or Debug output.
- No background health queries. Idle cleanup only drops local pooled objects; preserve the allocator's maximum two connections and warm/minimum one unless explicitly tested otherwise.
- Native ID transactions preserve row lock, database-clock-after-lock ordering, immutable generator layout and confirmed high-water commit. Do not combine the clock query before the lock without proving its concurrency semantics.
- No automatic retry of an uncertain commit. Business rollback never rewinds IDs.
- SeaORM/SQLx remain explicit legacy/offline dependencies pending later service/migration ports; this increment must not claim the full driver rewrite or a throughput gain.

### Task 1: Native pool and lifecycle

- [x] Write failing native connection-policy and owned PostgreSQL tests for strict URI/TLS/role handling, bounded size, cancellation, explicit rollback reuse, dirty-session refusal and idle cleanup.
- [x] Implement `persistence::postgres` with Deadpool/tokio-postgres and a transaction guard, using native query/row types directly.
- [x] Verify no unsafe connection returns to the pool after query/commit cancellation or failure.

### Task 2: Actual allocator port

- [x] Add failing tests for native concurrent allocation, restart, backwards clock, durable high-water and independent business rollback.
- [x] Add explicit native allocator construction; retain the legacy constructor temporarily for unmigrated tests/offline paths.
- [x] Wire the new allocator into Core behind the specific ID backend flag and prove an actual HTTP write uses it against owned PostgreSQL.

### Task 3: Qualification and continuation

- [x] Run clean max75 owned PostgreSQL, current runtime regressions, Clippy and formatting; preserve unrelated Native-Git files.
- [x] Review the complete security/lifecycle change once and fix material findings.
- [ ] Publish the signed Blacksmith PR and wait for checks before integration.
- [ ] Continue native signed tenant transaction/session/service ports after this increment; benchmark before changing defaults or selecting S3 replacements.
