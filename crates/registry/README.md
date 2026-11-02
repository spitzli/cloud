# Native registry

`RegistryService` owns retained customer/project CRUD, archive/restore, audit reads,
and the operator compatibility queries. Public JSON field names and decimal string
IDs remain compatible with `packages/core/src/registry`. SeaORM entities map the
existing `webdock_admin` tables; this crate never migrates them at runtime.

Every operation starts a signed RLS transaction, checks current operator authority,
locks the unique `users.auth_subject` mapping, and rechecks authority before commit.
Audit insertion shares the mutation transaction. Project ownership cannot change.
Project archival always checks retained non-retired CMS inventory, even if that
feature is disabled. Additional trusted `ArchiveGuard` implementations supplement
this check. Existing database deletion fences remain effective.

The host must inject its coordinated `webdock_persistence::IdAllocator` with
`with_allocator(Arc::new(allocator))`. Its dedicated pool commits ID high-water state
independently of business rollback. `DatabaseAllocator` is retained only for migration
compatibility; `RegistryService::new` alone selects that transitional allocator.

Validation:

```sh
cargo test -p webdock-registry
cargo clippy -p webdock-registry --all-targets -- -D warnings
```

The ignored PostgreSQL integration test requires an already provisioned local
`webdock_rls_test` with native identity/RLS and ID migrations. It reads
`WEBDOCK_TEST_MIGRATION_DATABASE_URL` (fixture owner),
`WEBDOCK_REGISTRY_DATABASE_URL` (restricted registry role), `AUTH_DATABASE_URL`
(identity fixture reads), and `DATABASE_RLS_SECRET`. Run it with:

```sh
cargo test -p webdock-registry -- --include-ignored
```

The fixture refuses non-loopback URLs and other database names. It inserts unique
records and removes only those records and the tenant organizations created by the
existing customer trigger; it does not reset schemas or shared data.

HTTP routing, shared cache infrastructure, feature extension registration and
production cutover are host responsibilities, outside this crate.
