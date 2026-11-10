# Database module and native Tabularis boundary

Source checkpoint: 2026-10-15. No schema, tenant identity, database credentials or live resources changed.

`modules/databases/src/server/service.ts` now owns the existing registered and managed database launch/grant/session service as `createDatabaseService`. The API composition supplies local hosting authorization, audit, encryption and gateway configuration through typed ports. The module reads using the stable `DatabaseReader`; it does not import applications or initialize provider credentials. Existing SQL, row locks, one-use launch codes, grant/credential/revision fences, expiry and audit behavior are retained. Existing API export paths remain composition wrappers, so current route URLs, IDs, SSO binding identities and transaction boundaries stay unchanged.

`adapters/databases/postgres` owns the actual PostgreSQL/Drizzle pool construction previously in `apps/control-plane/src/db/client.ts`. The compatibility path exports the same `databaseURL`, `createDatabases`, `createWorkerDatabases` and `DatabaseConfig`. Production role checks, verified TLS, signed RLS pools, connection budgets and the worker's restricted single pool are preserved. These are platform metadata connections; they are not browser-accessible customer database bindings.

`integrations/tabularis-web` owns the existing native gateway RPC allowlist, connection remapping, profile-aware capability restrictions and public response redaction. `apps/database-gateway/src/policy.ts` remains a compatibility export. SQL authorization continues to depend on the bound PostgreSQL role, not a home-grown SQL parser. The actual vendored `@tabularis/web-ui` remains the Console UI; no iframe or substitute editor is introduced.

The separate gateway process still holds confidential runtime credentials and starts isolated Tabularis runtimes using the existing bounded process manager. Its privileged process lifecycle, loopback listener, verified PostgreSQL CA, per-session scope and shutdown remain unchanged. Provider extraction does not move customer database credentials into the browser or API UI responses.

## Launch and capability contract

`@spitzli/integration-tabularis-web/launch` exposes browser-safe validation for existing launch data and session endpoints. The target is explicitly a registered customer binding, managed customer database, or platform metadata database. Customer/project IDs and binding/database IDs are checked together. Expired launches, extra top-level runtime credentials and session endpoints on another origin/scope are rejected. Capabilities default to read-only; platform inspection refuses write/schema hints. No platform metadata launch endpoint exists in this slice: requesting one fails explicitly, rather than reusing a customer's launch or giving an operator an owner connection.

The existing managed customer database workflow remains its explicitly authorized schema-role operation. Changing it to a different profile requires a separately provisioned database role and is not inferred from UI flags. UI capability hints never replace gateway authorization or database grants.

## Verification

New offline tests cover database URL parameter injection, production role/TLS/unsigned refusals, worker pool isolation, tenant-admin launch authorization, row locking, unhealthy/pending credential fences, public response redaction, target separation and exact session endpoint validation. Existing gateway policy tests exercise the extracted implementation through its compatibility path. Full retained RLS/session integration tests continue to use the owned disposable PostgreSQL fixture; this extraction does not run against production databases.

## Managed PostgreSQL ownership

Managed database ordering, backend selection, reservation capacity, billing records, credentials, application bindings, agent claims/checkpoints/completion, observations, maintenance, migration-source retention and database schema definitions now live in `modules/databases/src/server/database-*.ts` and `databases.ts`. The old API `modules/hosting/database*.ts` locations are composition/compatibility paths only. No implementation remains under the Hosting product module for those controllers.

`@spitzli/database-contracts/runtime-ports` declares the public `DatabaseServices` interface, with concrete offer/list/plan/view/order dispatch overloads and typed binding, credential and agent-operation contracts. The raw transport dispatcher still accepts unvalidated input, which is parsed by its owning database service. Hosting's agent transport, command router, setup orchestration and service bindings receive these services from API composition; they no longer import Database server internals even as `ReturnType` types.

`@spitzli/hosting-contracts/runtime-ports` contains the neutral query, actor authorization, agent identity, transaction, audit, lock, replay and result-recording signatures used by both domains. Allocation arithmetic is shared through the pure `capacity-totals` helper. Runtime pools, signed scopes, encryption and native-resource capacity remain explicit host-supplied ports. The signed `database-order` admission carries the already-authorized actor's local subject/session references for Clerk mapping rechecks; it grants no broad agent access to mappings. Existing row locks, SQL predicates, lease proofs, account IDs and cleanup/reconciliation semantics remain intact.

## Database UI ownership

Database order/plan forms, credential display, tenant database navigation, registration/access forms, managed rows, workspace shell, actual Tabularis workbench and schema preferences now live in `modules/databases/src/ui`. Console files keep existing imports/routes and inject real app Server Actions via `DatabaseActionsProvider`; Hosting's action provider no longer owns database actions. The module contains no application alias or Hosting server/UI imports.

The browser still uses the pinned vendored Tabularis package, redirects are refused during session launch, exact gateway scope checks remain, and cleanup logs out the prior runtime. Resource keys include local user/binding identity and managed-versus-registered mode, so switching resources remounts the editor/session state instead of showing the previous workspace while a new launch completes. Schema preference keys and PostgreSQL permission enforcement are unchanged.
