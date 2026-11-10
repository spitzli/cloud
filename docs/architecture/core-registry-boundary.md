# Core registry boundary

The internal operator registry is core platform functionality, independent of customer CMS installation. `packages/core/src/registry` owns registry DTOs, validation, filter compilation, persistence operations, compatibility reads and Snowflake allocation. `packages/platform-db/schema/registry` supplies the unchanged table mappings; it does not depend on core or initialize connections.

The API `registry/service.ts` validates environment URLs, role, TLS, pool limits and signed-context configuration, creates the scoped pool/ORM, and composes `createRegistryService`. Request identity storage in `context.ts`, HTTP handling, session verification and pool lifecycle ownership remain API responsibilities. No mutable service locator moved into core.

## Explicit dependencies

The service factory receives a typed ORM, raw query/connect pool with read-only pool telemetry, a signed session-scope runner, an installation capability callback and a close operation. Core imports neither the API, a product module nor a concrete provider adapter. The CMS write guard calls the injected `requireProduct("cms", intent)` before data access; a new registry CMS connection uses `provision`, existing connection management uses `access`. Core customers, projects, operator identity and inventory reads do not depend on CMS activation.

The extraction preserves the original transaction body: current operator authorization before and after work, the exact local `auth_subject` mapping with a shared row lock, immutable customer/project/CMS binding rules, parent/target locks, deletion fences and transactional audit records. Provider identities, URLs, schema/table names and decimal Snowflake strings remain unchanged. The compatibility store discards caller-provided `user` and `overrideAccess`; its verified identity is supplied by host composition.

Public `registry/types` contains only DTO types; `registry/contracts` contains validation without database imports. `@spitzli/api-client/registry` now exports the DTOs using explicit `export type`, so Console consumers cannot pull persistence clients through the compatibility path. Existing API import paths remain thin reexports or composition wrappers.

## Verification boundary

Core tests cover session scoping, mapped-row locking, post-read revocation, CMS gating before transactions, untrusted compatibility options, full-width Snowflake strings and source dependency boundaries. The existing restricted-role `registry.integration.test.ts` remains the authoritative verification of real SQL, RLS, audit behavior, concurrent writes, relationships and preserved identifiers. No production migration or provider operation is part of this source move.
