# Complete protected archive product paths — execution record

Follow-up scope is authorized: committed-source archives, observed project reports,
automatic real managed-PG readonly binding, native signed packaging/promotion and
custom Console flows. No delegates, commits, production activation, old-store moves
or irreversible locks. Protected root handovers and .dev-local/demo-owner.json remain
unread. Parent runs final root quality.

## Ownership and immutable boundaries

- This task owns plugins/archives/**, tools/tests/native-archives-*, archives local
  fixture, new source contracts/modules/tests and runbook/evidence.
- New shared files: crates/export-protocol/**; proto/webdock/export/v1/export.proto;
  plugin-host/src/exports.rs (nested in authority), control-plane/src/plugin_exports.rs,
  build/src/export.rs (nested in service), databases/src/{export.rs,service/exports.rs},
  agent/src/database/exports.rs. Minimal corresponding module/Cargo/RPC wiring added.
- Build export-protocol dependency is OPTIONAL, only service feature. Parent owns
  evidence/artifact/worker/execution/publisher; do not edit those.
- Frozen 0050 SHA256: 2d3e22026c7ad792234ecce7f5a286a856eac681a22af504c23351d2de86a714.
- Frozen 0051 (backup owner) SHA256:
  a4f16036e77bd2617f61cecc7c7881bb1abfceb920a16309e092770094ed7767.
- 0046–49 and original backup/storage proto/producer ABI/object-storage manifest unchanged.
- Backup owner is integrating exact tested readonly role hooks in existing agent SQL,
  lifecycle/HBA/restore/purge files. Do not edit those concurrently. Its thread:
  thread:delegated-task:command%3Amcp%3A2f6fbc0a-1750-43a4-a82a-b298790aba13%3Adelegate-task%3As3-native-pg-connected-restore-2026-10-25-r2
- Parent: e79adf82-487b-4f78-9fd8-d106c9bd1c3a.
- Native storage/Console helper owner:
  thread:delegated-task:command%3Amcp%3A2f6fbc0a-1750-43a4-a82a-b298790aba13%3Adelegate-task%3As3-storage-native-installation-2026-10-25-r1
- Shared Core compiler was announced GREEN after frozen 0050+0051; leave implementation
  stable while peers pin native binaries. New tests/plugin packaging can proceed.
- Their ports 19640–19643 are reserved. New archives fixture uses 19740–19743.

## Implemented, not yet fully qualified

Separate private ExportSources gRPC uses existing pinned Core channel, not public
JSON bytes. Actions can list/bind only source identities; Database/Stream require the
exact current archives.produce job. ExportJob proof records actual claims, peer,
subject/session; no fabricated Session. RepositoryContext::PluginJob and 0050 bind
leases to exact invocation/job/attempt/claim/current feature/resource; classic domains
delegate to unchanged old function. Current source authority checked during streaming.

DatabaseService checks actual actor + org:databases:manage, dedicated customer/project,
confirmed ready revision, backend UID, CA/host/port. Derived readonly credential uses
HMAC-SHA256 domain `webdock.database-export.readonly.v1\0` plus decimal database ID,
key canonical native getrandom(32) application secret. Existing native create/rotate/
upgrade paths generate these; no caller/import password command exists. Old unsafe
material fails closed. Helper creates wd_export_<id>, NOINHERIT, no memberships,
SELECT/USAGE on app only, bounded connections, guarded rotation/disable/drop. Existing
backup owner will wire tested hooks. Managed dump verifies TLS and actual readonly
privileges, app-only pg_dump, private CA/env, no credential argv/persistence.

Plugin stores only SourceRef identity/fingerprint/immutable commit, polls Core source
permission while producing, compresses completed normalized source/CSV into anonymous
staging and reuses verified shared S3 bytes. Whole job budget clipped to remaining
Core lease minus 2 seconds and max 35 seconds. Public downloads additionally require
canonical Core job completed result for the exact resource, and source validation
through the sealed transfer's actual original authority on every producer callback.

Observed reports currently contain project ID, clock, resource plugin/offering counts,
whitelisted build states — no user names, URLs, logs or credentials. Must add explicit
adversarial =,+,-,@/control name coverage; quoting alone is not sanitization. Git label
is committed source, not runtime backup; history/submodules/LFS limits explicit.

Custom UI exists in plugins/archives/ui, with stable actions/IDs, frozen create/write
retry, real Core job state, expiry/limits, authenticated download via generic frozen
ApiClient.downloadBinary. Descriptor/blob never enter TQ caches; object URL abort/hide/
scope retirement. usePluginStorage is object-storage-only and NOT used.

## Verification already observed in r2

- SourceRef + HMAC contract RED→GREEN (2 tests).
- New readonly helper actual owned PostgreSQL test GREEN: SELECT/default future grants,
  DML/DDL/SET ROLE/foreign DB denied, disable kills connection, password rotation/drop.
- Local wrapper WEBDOCK_ARCHIVES_SKIP_CORE=1 passed owned MinIO/dump/restore + readonly
  tests; Core bootstrap deliberately skipped during prior schema hold, exact cleanup.
- UI RED→GREEN: frozen create retry and no cache/context-retired object URL (2 tests).
  One lint optional-chain fix pending before fresh final lint/typecheck.
- Existing normal plugin suite passed with new metadata fields (6 tests, owned tests ignored).
- Core check --locked and build --no-default-features check GREEN after source wiring.
- Production image actually built on Rust1.94.0 using production-only runtime manifest,
  342KB default-deny context. Docker tag webdock-archives-reviewed:20261025-r2
  was observed absent before completion; final image ID/index digest
  sha256:e90a6127924fdc516caa26a003ce54071c0a500fb2d32db0947d7849b9403294,
  child manifest sha256:48fecfd6d257aa9e3c1b0bdfaea07881b563dcddb6ea23ed5f1b33c4feb04ac4.
  It predates later minor failure-code fields: final native artifact must rebuild.
- Nonroot image UID 65532:65532, entrypoint /usr/local/bin/webdock-archives, real pg_dump
  18.6 (Debian 18.6-1.pgdg13+2) exercised read-only/network-none.
- Official pinned PostgreSQL18-trixie index:
  edb0c08dc33befd0262ab3b5cf1c4f3f8cfd8b09e88c05b45adee29d6761ddb6,
  amd64 child 9770878138a2e83abcb10856d30cc3f24dae0d317bb956db6ab9159de6c3c80d,
  docker-library/postgres revision e00e1bd34ec5c8a8e7ad89b273b3d42efaf6d5bc.
- Protobuf compiler 3.21.12-11+deb13u1 verified from signed Debian trixie repo in
  disposable pinned Rust base; Dockerfiles pin it exactly.

## Remaining execution (continue, do not ask product-scope approval)

1. Finish native-archives-fixture.mjs (adapted from existing native object storage helper,
   shared lower-level custody utilities). It is scaffolded, not run yet. New named
   HTTPS *.native-archives.test:19443 profile enforces exact run bucket and CA/deadline;
   local MinIO ACL/multipart exceptions compile-time-only. Verify cleanup aborts exact
   incomplete uploads after all producers stopped. Add its tests and acceptance module.
2. Qualify actual signed archive image normal operator install/agent promotion/job/gateway
   and own Console at 375/1280 widths. Native images use fixture CA only; secrets not baked.
3. Add real Core source protocol tests: observed report/adversarial names, exact job
   completed gate, denied DB/source transitions and repository attempt/cancellation.
   Existing plugin core.rs was updated to mount UnconfiguredExportSources for the
   diagnostic canonical completion gate; execute it after holds released.
4. Real managed-PG binding must target the owned actual managed installation/project,
   not fabricated standing authority. Integrate with the completed native PG fixture
   or its exported service helpers; coordinate with its owner. SQL helper alone is
   not the whole managed-source qualification.
5. Test normalized project source through existing repository provider/lease protocol.
6. Finish strict Rust Clippy/format and TS/lint, tests/native cleanup. Write r2 evidence
   and runbook with exact changed files/commands/current limits. No full root quality needed.
7. Remove only owned native resources/image tags; never prune shared/base caches.

Frozen Console API: downloadBinary(descriptor,{signal,isCurrent})->Promise<Blob>;
uploadBinary(descriptor,file,options)->receipt. Use usePluginScope, not object-storage
convenience. Shared files api.ts/plugins/storage.ts/sdk.ts remain peer-owned/unchanged.

## Final outcome

All scoped paths implemented and locally qualified. Final normal signed native run
0b366d7b5095 passed operator install/agent promotion, real Core job/source/gateway,
full actual Console Create/Generate/Status/Download/Hide/Delete at375/1280, expiry
and fresh-ticket deletion. All own native runs exactly cleaned. Domain owner confirmed
actual managed source export/rotation/pause and its independent cleanup. Repository
job/protocol and formula-name RLS report tests green. Exact final commands,61 new
files +17 shared wiring files and limits are in the r2 verification record. No broader
Wasabi, production signing/activation, whole runtime backup or root-quality claim.
