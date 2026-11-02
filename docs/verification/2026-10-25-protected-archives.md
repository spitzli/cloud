# Protected exports and diagnostic archives: local producer evidence

This local milestone is superseded by `2026-10-25-protected-archives-r2.md` for current implemented product paths.

Date: 2026-10-25. Scope: handover sections 5/6, implementation sub-agent in the shared
checkout. This record describes the concrete new producer, not a production rollout
or completion of every project/report source mentioned in the handover.

## Delivered behavior

`plugins/archives` is a standalone native executable using the existing
ActionHandler/JobHandler/HostClient interfaces and frozen StoragePlan ABI. It provides
REST/MCP manifest parity for create, generate, status, download and explicit delete.
Core alone owns admission, resource reservations, immediate jobs, subjects, sessions,
features and authenticated opaque TransferDescriptors. No queue or auth model was added.

Concrete producers:

- Consistent PostgreSQL custom-format, gzip-compressed logical dumps of explicitly
  configured loopback per-tenant databases, with a read-only source role. 32 MiB output,
  two concurrent producers, 25-second subprocess deadline, five-second connection/lock
  wait bounds and 20-second statement timeout. Private anonymous staging and credential
  environment; no source URL/password in argv or error output. Actual dump restored
  into a fresh database and both fixture rows verified after restore.
- Finished, submitted structured diagnostics compressed as `diagnostics.json.gz`.
  Decimal build IDs, bounded numeric counters and small enums only. At most 100 build
  summaries/32 checks. Unknown/free-text fields, credential URLs, tokens, mail bodies
  and unfinished state are rejected. Existing arbitrary build log strings are excluded.

A private plugin PostgreSQL migration stores product receipts. No native migration
was added or edited, including 0046–0049 or 0050. Source identity is fixed at request
creation without hashing passwords into identity. Rebinding is rejected, while password
rotation preserves identity. Successful output removes structured input from metadata.

Verified S3 uploads/readback use `crates/object-storage`. Locations stay private.
Download plans are exact-size/hash-bound and private; Core creates the public descriptor.
Both original Core authority and producer ready/tenant/resource/epoch/expiry authority
must still hold. Expiry is at most 24 hours; download grants at most four minutes.
Explicit delete first revokes the producer epoch, then deletes only the receipt's key,
observes absence, and records deletion. Deleting/failed/incomplete/expired archives
cannot be downloaded. Receipts survive runtime restart; a retried completed job returns
the original receipt. Producing records cannot be seized by a different job ID.

The production constructor requires HTTPS, no bucket policy, a private owner-only ACL,
no versioning and a namespace-scoped enabled lifecycle: expiration 1–7 days plus one-day
multipart abort. It checks these before accepting uploads/deletion. There is no public
bulk-delete/GC API. Provider lifecycle handles inaccessible upload orphans after a
crash before receipt commit; it is not an immediate deletion acknowledgment.

## Tests and commands

TDD evidence: the initial diagnostic compression test failed on the unavailable
producer; the ready-receipt authority test failed on deny-all; the source-binding test
failed before fingerprint support; the ACL test failed on deny-all. Each was followed
by a green implementation. Real acceptance also exposed and corrected libpq environment
connection selection, schema-depth constraints and fixture timing assumptions.

All final commands completed successfully:

```bash
CARGO_TARGET_DIR=target cargo build --manifest-path plugins/archives/Cargo.toml --locked -j 2
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --features local-test -j 2
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-test -j 2 -- -D warnings
python3 tools/tests/archives-storage.py
cargo test --locked -p webdock-object-storage --features local-test -j 2
cargo test --locked -p webdock-plugin-host --test storage_stream -p webdock-plugin-protocol --test storage -p webdock-plugin-runtime --test storage -j 2
rustfmt --check --edition 2024 --config skip_children=true plugins/archives/src/*.rs plugins/archives/tests/*.rs
```

Results: plugin workspace **6 passed**, with 2 owned tests ignored in the ordinary
command and explicitly run by the acceptance wrapper; owned acceptance **2 passed**
(final producer run 13.03s, native Core/mTLS run 19.41s). Shared object-storage suite
**6 passed**, one separately owned MinIO test ignored; the new acceptance itself uses
the real shared S3 byte transport. Existing gateway/protocol/runtime regressions
**10 passed** (7 + 2 + 1). Build, strict Clippy, Rustfmt and Python AST syntax checks
passed. No full root workspace suite was run; other product work remained concurrent.

The plugin lockfile was independently resolved for its declared Rust 1.94 floor,
instead of inheriting the fixture's newer AWS transitive releases requiring 1.94.1.
Linux dependency metadata confirms no selected package advertises an MSRV above
1.94.0. Execution used the installed Rust 1.98.1 toolchain; an actual 1.94 compiler run was not
performed. No root lockfile was modified by these commands.

The Core gateway's existing native configuration remains
`storage.stagingDirectory` (owned 0700) and
`targetPolicy.storage[projectID]["archives"]`; the runtime's producer staging is
separate. The native test uses the equivalent existing file binding and pinned
provider target. No public input selects a URL, gateway binding or filesystem path.

## Real owned acceptance

`python3 tools/tests/archives-storage.py` reuses the existing running MinIO, creating
only fresh bounded owned buckets, one restricted user/policy and private temporary
configuration. It starts an explicitly labeled disposable PostgreSQL 18 instance for
source/metadata/restore and reuses the existing disposable Core jobs fixture for Core
state. It never starts another S3 service or changes k3s, Kind, Contabo or existing stores.

The owned Rust acceptance exercises:

- Real read-only pg_dump, verified S3 upload, binary gateway delivery and actual restore.
- Durable receipt restart/retry and conflicting tenant/project rejection.
- Same-length S3 corruption: a new ETag is not mistaken for a SHA-256; delivery is denied.
- A blocked real pg_dump is cancelled; anonymous staging is removed and its server
  backend ends within the configured five-second lock-wait bound.
- Real missing-database failure produces no usable download receipt.
- Source database rebinding fails instead of exporting the replacement database.
- Structured gzip decompression/content equality, actual expiry denial and exact deletion.

The native Core acceptance exercises real runtime RPC and **pinned mTLS Core callbacks**,
with AWS SDK and tonic both linked. Explicit ring selection happens before creating TLS
clients. Create is recorded through Core reservations; generate uses the real Core queue,
claim, signed job attempt and completion. Core retains an opaque descriptor after the
JSON invocation ends. Wrong original subject, revoked feature and removed membership
are denied. Revoking the feature after gateway staging denies the next delivered chunk.
Deleting the producer also denies its authenticated authorize_storage callback.

Successful cleanup observes an empty owned bucket and no incomplete multipart uploads,
then removes only the helper's exact generated buckets/user/policy. The labeled owned
PostgreSQL container and private files are removed. Core fixture containers use Docker
`--rm` and are dropped by their existing fixture. No retained fixture inventory, root
credential, signed URL, source credential or payload is written to this record.

## Capability limits and remaining work

The actual local MinIO drops `AbortIncompleteMultipartUpload` when importing lifecycle
configuration and rejects `s3:GetBucketAcl` in IAM policy creation. The explicitly built
`local-test` constructor permits these two gaps while still requiring absent bucket
policy, no versioning and bounded current-object expiry. The ordinary executable
cannot select that constructor, even if the feature is compiled. Its retention/access
checks remain strict. The final fixture's observed lifecycle has one enabled
`webdock-archives-retention` rule, prefix `archives/`, current expiration two days and
no multipart-abort field. No existing MinIO server/global setting was changed. A local
green result does **not** establish production multipart cleanup or Wasabi compatibility.

Production/Wasabi startup remains gated on a target proving the required private ACL,
HTTPS and retention capabilities. Lifecycle expiration is asynchronous. A real process
crash during S3 upload and provider deletion failures/lost deletion responses were not
fault-injected here; the shared transport's interrupted/oversized-upload tests run
separately. Retention policy is the recovery boundary for unreceipted upload orphans.

Supported scope deliberately excludes project source archives and arbitrary report
sources: inventory found no existing active Rust producer/scoped snapshot contract for
these. The existing host exposes jobs/data and the frozen storage ABI, not a generic
project/report export source. A separate approved product source contract is required.
No generic unused archive kernel or arbitrary filesystem/SQL/log upload was introduced.

Managed-database credential/source discovery is not integrated with the concurrently
owned database module. Sources must be explicitly configured; remote pg_dump and
shared-table tenant exports are rejected. Database backup/PITR is another workstream.
Submitted diagnostics do not claim to be observed build/monitoring truth.

No signed image/catalog entry, custom Console UI, production activation, old-store
migration or irreversible locks were created. The repository image digest is a packaging
placeholder. Parent/native installer coordination is still required to package and
register this new plugin. No production database or mailbox was exported.

## Exact changed files

Only new files belonging to this subtask:

- `plugins/archives/.gitignore`
- `plugins/archives/Cargo.toml`
- `plugins/archives/Cargo.lock`
- `plugins/archives/README.md`
- `plugins/archives/manifest.json`
- `plugins/archives/migrations/0001_metadata.sql`
- `plugins/archives/src/backend.rs`
- `plugins/archives/src/diagnostics.rs`
- `plugins/archives/src/engine.rs`
- `plugins/archives/src/lib.rs`
- `plugins/archives/src/main.rs`
- `plugins/archives/src/model.rs`
- `plugins/archives/src/producer.rs`
- `plugins/archives/src/store.rs`
- `plugins/archives/tests/core.rs`
- `plugins/archives/tests/diagnostics.rs`
- `plugins/archives/tests/lifecycle.rs`
- `plugins/archives/tests/manifest.rs`
- `plugins/archives/tests/owned.rs`
- `plugins/archives/tests/source.rs`
- `tools/tests/archives-storage.py`
- `docs/superpowers/plans/2026-10-25-protected-archives.md`
- `docs/verification/2026-10-25-protected-archives.md`

Root TODO, quality runner, shared manifests, database/build/CMS modules, the gateway,
producer ABI and object-storage manifest were not edited by this subtask. Protected
handover/owner files were neither read nor modified. No commit or delegation occurred.
