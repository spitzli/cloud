# Binary gateway implementation fix round 2

Local implementation and fresh owned fixtures only, 2026-10-25. No commit, production,
Wasabi, retained-service change, native installation completion, or first-party
installation claim is made. Parent MinIO was left running. Private configuration,
credentials and signed URLs are omitted.

## Fixes and stable ABI

- The bounded staging kernel now owns delivery sends. Queue backpressure is guarded
  by the plan's exact deadline, live authority polling and receiver cancellation.
  Revocation/expiry/disconnect tests retain four unpolled bodies, observe four actual
  anonymous file descriptors, and verify all files close and all four permits can
  be reacquired without polling/dropping the stalled bodies.
- The HTTP lane accepts only object GET/PUT, including a nonempty bucket/object key
  for origin-wide bindings. Canonical, unique SigV4 query names, SHA-256 checksum
  parameters, GET `versionId` and matching `x-id` are permitted. Administrative
  subresources, encoded/alternate-case query names, duplicate parameters and all
  multipart queries are denied before provider I/O. Multipart is not supported by
  this ticket/receipt ABI.
- Signed PUT `if-none-match: *` and GET `if-match: "<etag>"` are accepted and forwarded.
  Conditional names must be signed; supplied SHA-256/content-type values must match
  the plan. The staged/received bytes still undergo exact-size and SHA-256 checks.
- Protocol schema fields and `required` entries accept lower-leading camelCase in
  Rust and the SDK. Action/capability/slug rules are unchanged. Prototype-hazard
  paths remain denied. The generated plugin-manifest contract is regenerated.
- `StoragePlan` / `StorageAuthorization` remain v1, with required `resourceEpoch`.
  The callback remains deny-by-default. Public descriptor names/serialization are
  unchanged: `version`, `id`, `path`, `method`, `expiresAt`, `exactBytes`,
  `expectedSha256`, `attachment` (`filename`, `contentType`).
- Managed installation now copies explicit private **targetPolicy.storage**
  `[projectID][alias]` bindings into enrollment. **recipe.storage remains a boolean**
  for PostgreSQL provisioning. There is no project-default fallback. Core's config
  property is **storage.stagingDirectory**. Bindings use `origin`, pinned
  `addresses`, optional PEM `ca`, and `pathPrefix`; managed policy requires HTTPS.
  The private map allows at most 256 projects and 32 aliases per project.
  A precreated real project can be bound before catalog/Core loading. `pathPrefix:
  "/"` is supported for an explicitly bound project; it still cannot target bucket
  operations. Binding content remains included in admission/current metadata fences.

## Limits and migration

The data limits stay 512 MiB/file, four concurrent transfers, 2 GiB staging, 64 KiB
read chunks, 300-second plans. Private plan JSON stays 12 KiB, URL length 8192 bytes.
Storage tickets retain only the TLS leaf, capped at 4096 DER bytes. Before sealing,
the complete private plaintext is capped at 24,520 bytes (existing cipher capacity),
with a 32 KiB sealed cap. PostgreSQL measures the complete final jsonb context
before insertion; size rejection is a sanitized invalid-input result.

**New migration 0049_plugin_storage_transfers.sql** allows 64 KiB outside the existing
configuration allowance for structurally restricted storage-transfer rows. Original
authority outside `storageTransfer` stays at 16 KiB; ordinary invocations retain
16 KiB. Transfer rows explicitly cannot authenticate as signed invocation tokens.
No older migration was edited. Migration 0048 remains the parent's backup migration.

The boundary regression seals a full 12 KiB plan with a 2 KiB TLS leaf and a large
outer MCP-shaped context, confirms the unnecessary chain is discarded, and checks
URL redaction. Fresh PostgreSQL verifies the storage-only allowance and continued
ordinary-limit rejection.

## Verification

All commands completed successfully:

- `cargo test -p webdock-plugin-protocol -p webdock-plugin-http -p webdock-plugin-runtime -p webdock-plugin-host -p webdock-control-plane`: **181 passed**, 108 optional fixtures ignored.
- `cargo test -p webdock-plugin-host --test storage_authority -- --ignored --nocapture`: **7 passed** on fresh PostgreSQL containers, including genuine source/browser/MCP authority, private producer deactivation, the 61-second Core/runtime delivery test, atomic concurrent upload claims, cancellation after claim, actual file/permit release, and ticket reuse denial.
- `cargo test -p webdock-plugin-host --test storage_minio -- --ignored --nocapture`: **1 passed**, exact UUID bucket cleaned.
- `python3 crates/plugin-host/tests/support/storage_producer.py`: actual Storage Engine + AWS SDK producer and binary gateway passed **3 MiB conditional PUT/GET**, SHA-256, create-only overwrite rejection, private state/epoch revocation and exact cleanup. This companion crate has its own lockfile; root locks were not edited.
- `bun test packages/plugin-sdk/tests`: **27 passed**.
- `bun run --cwd packages/plugin-sdk check`: passed.
- `bun tools/plugins/generate-schema.ts --check`: passed.
- `cargo clippy -p webdock-plugin-protocol -p webdock-plugin-http -p webdock-plugin-runtime -p webdock-plugin-host -p webdock-control-plane --all-targets -- -D warnings`: passed.

The delivery, query, checksum, camelCase schema, native policy and ledger regressions
were observed failing before their fixes. Earlier development failures were resolved;
no failure remains in the commands above. This does not cover every ignored fixture
or claim a full-workspace/production test run.

## Protocol fingerprint and producer handoff

No proto edit was needed in this round. Current `proto/webdock/plugin/v1/plugin.proto`
SHA-256 (protocolSchema source fingerprint):

`86af4593960509a3b13b84b5495d5473560fabaa0b102703116d00fff6451124`

Keep the current AWS conditional presigns, explicit checksum, `resourceEpoch` and
`authorize_storage` implementation. Update the producer manifest output to describe
the **complete public descriptor** above; no lowercase DTO conversion is needed.
Rebuild producer artifacts with the current protocol/runtime/SDK contracts. The
actual producer integration required no producer-source edits. Core's count/replay
short-circuit and all-authorization-before-cache ordering were preserved.

## Exact files changed in this round

- `crates/control-plane/src/plugin_storage.rs`
- `crates/control-plane/src/plugin_installation/worker.rs`
- `crates/plugin-host/src/authority.rs`
- `crates/plugin-host/src/storage.rs`
- `crates/plugin-host/src/storage_admission.rs`
- `crates/plugin-host/tests/storage_authority.rs`
- `crates/plugin-host/tests/storage_stream.rs`
- `crates/plugin-host/tests/support/storage_producer.py`
- `crates/plugin-host/tests/producer/Cargo.toml`
- `crates/plugin-host/tests/producer/Cargo.lock`
- `crates/plugin-host/tests/producer/src/main.rs`
- `crates/plugin-host/README.md`
- `crates/plugin-http/src/storage.rs`
- `crates/plugin-http/tests/storage.rs`
- `crates/plugin-protocol/src/manifest.rs`
- `crates/plugin-protocol/src/storage.rs`
- `crates/plugin-protocol/tests/contracts.rs`
- `crates/persistence/migrations/0049_plugin_storage_transfers.sql`
- `packages/plugin-sdk/src/contracts/manifest.ts`
- `packages/plugin-sdk/tests/manifest.test.ts`
- `tools/plugins/tabularium-kind.schema.json` (generated protocol artifact)
- `docs/verification/2026-10-25-storage-gateway-round2.md`
