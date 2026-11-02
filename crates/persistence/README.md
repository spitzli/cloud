# Coordinated platform IDs

The private `webdock-ids` library calculates IDs. This adapter coordinates its fixed platform identity with PostgreSQL so concurrent API processes and restarts cannot reset the sequence. No separate ID service is required.

Apply `migrations/0001_ids.sql` through offline migration tooling. It reserves slot 2 for new platform allocation; legacy slots 0 and 1 remain available only to the retained generators. Existing entity IDs are never rewritten. The platform epoch, positive signed-63-bit range and JSON decimal strings remain unchanged.

Create the allocator with `IdAllocator::connect` using an authorized restricted role. It intentionally opens a dedicated small pool: callers may already hold business transactions, and sharing an exhausted business pool would deadlock. Each allocation locks the checkpoint row, samples database time after the lock, calls the Rust layout implementation, updates the checkpoint and commits **before** returning the ID. A failed business transaction or uncertain response may leave a gap, never permission to reuse an ID.

The database trigger prevents checkpoint rollback or identity/layout mutation; runtime roles can update only the checkpoint column. Runtime code cannot create/delete/reassign generator slots. Additional agent identities need an explicit coordinated registration lifecycle. This adapter does not claim disconnected/offline generation or exactly-once network delivery.

`allocate_at` accepts an explicit clock value for deterministic tests/integrations. Ordinary platform code uses `allocate` and the database clock. Clock rollback, sequence exhaustion, missing metadata or incompatible state fail rather than emit a guessed value.

Verification: `WEBDOCK_ID_TEST_DATABASE_URL=... cargo test -p webdock-persistence --test ids -- --ignored` requires an owned literal-loopback database ending in `_test`. It applies the additive schema there and preserves committed checkpoints between runs.

## Shared database connection policy

`connection::ConnectionUrl` validates and canonicalizes runtime/offline PostgreSQL URLs. It requires explicit URL authority, rejects query overrides and duplicate TLS-mode aliases, and requires verified TLS outside loopback or when production requires it. Unknown driver options are rejected before their values could be logged. The validated URL contains credentials and deliberately implements neither Debug nor Serialize.

`connection::verify_runtime_role` is the common constructor/readiness check for platform pools. It verifies the fixed expected role against current/session authorization and the backend's original authenticated username, then rejects privileged flags and owner-role membership. Qualified PostgreSQL catalogs prevent shadowing. A privileged connection cannot pass merely by changing its role or session authorization. The helper returns a redacted policy error; domain adapters preserve their own error envelopes. It does not replace per-request RLS context or live user authority.

## Fresh installation and bootstrap provenance

`roles::provision_roles` operates only on an explicitly named empty database through an administrative connection. Its private password map still accepts the eight fixed runtime login names. An optional ninth entry, `webdock_offline_migrate`, creates the dedicated offline login with no superuser, bypass-RLS, database-creation, role-creation, replication or inherited privileges. It receives only explicit `SET ROLE` membership in the three schema-owner roles, without membership administration or automatic inheritance. It is deliberately excluded from `LOGIN_ROLES` and the runtime connection verifier.

Existing role passwords and grants are never changed. Unexpected privilege flags or memberships fail provisioning. Adding the optional password to `webdock-migrate --initialize-empty --provision-roles-from /absolute/private/passwords.json` requires no new CLI flag. Initial schema creation remains an explicit administrative operation; operator/account bootstrap must connect as the dedicated offline login and explicitly assume the platform owner inside its transaction.

Fresh `migrations::bootstrap` records the SHA-256 of the exact bundled `schema/baseline.sql` bytes in `webdock_api_migrations.baseline`. That schema and its baseline, retained journal and native journal tables belong to `webdock_platform_owner`. Ordinary runtime roles receive no marker-writing authority.

`migrations::verify_bootstrap_baseline(&impl ConnectionTrait)` is a read-only prerequisite for guarded initial operator setup. Under the caller's migration advisory lock, it requires trusted ownership, one recognized marker, and the complete exact retained journal. It accepts the native snapshot checksum or the retained `tools/migrations/baseline.sha256` checksum; native provenance, or an existing native journal, additionally requires exact native journal checksums and successful entries. `native_schema_ready` also accepts a transaction, so the caller can check both prerequisites while holding the same fence and owner role.

Neither verification nor ordinary upgrade adds missing provenance or repairs an existing marker/owner. A database created before marker support needs a separately reviewed adoption procedure; do not stamp it merely to make operator setup pass.

Role-creation verification runs only in a newly owned PostgreSQL Docker container:

```sh
WEBDOCK_BOOTSTRAP_DOCKER_TEST=1 cargo test -p webdock-persistence --test roles -- --ignored
```

The migration integration fixture uses a fresh uniquely named database with existing roles, then removes that database. It checks marker tampering, missing/changed journal entries, ownership and repeated initialization without modifying shared role credentials.

`migrations/frozen.json` records accepted native source checksums. `bun run quality:contracts` checks the actual bytes, following the repository's domain migration symlinks, before any database is involved. Keep existing entries unchanged and add a new migration for schema corrections. Append a newly accepted migration only after its restricted-role, upgrade/repeat and review checks; draft migrations need not yet appear in this file. This record describes source acceptance, not deployment to a production installation.
