# Protected archives r2: implemented product paths and native qualification

Date: 2026-10-25. Follow-up to the local milestone in
`2026-10-25-protected-archives.md`. The authorized committed-source, observed report,
managed-PG source, signed native packaging and Console paths are now implemented.
No production activation, delegation, workspace commits, old-store migration or
irreversible lock was performed. Parent runs final root quality separately.

## Implemented source and authority paths

The new fixed private `ExportSources` v1 RPC is separate from the existing frozen
PluginRuntime/storage and database backup protocols. Public actions choose only enums
and actual Core IDs, never SQL, filesystem paths, provider URLs or credentials. Large
source bytes stream over private RPC. Plugin-host exposes the contracts/guards;
control-plane composes existing DatabaseService/RepositoryAdmission readers, avoiding
an inverted dependency/cycle. The build dependency is optional and service-only;
isolated no-default guest/worker builds do not acquire the source/protoc closure.

Repository exports use a real `PluginJob` context, not a manufactured Session. Frozen
forward migration 0050 adds bounded archive authority to repository leases and delegates
classic Session/Build/Release behavior unchanged. Each new lease binds the actual
archives invocation/job/attempt/claim, actor/session, original enrolled peer, current
runtime/grant/feature/resource and source/provider revisions. Normalized files resolve
to an immutable commit, retain that identity, and are gzip-compressed as committed source.
Streaming and callbacks revalidate source/job authority; cancellation retires the lease.

Managed PG selects the actual dedicated customer's/project's confirmed source ID and
requires the current original actor's `org:databases:manage`, active ownership, ready
observed revision, confirmed credentials, exact backend/CNPG identity, CA/endpoint and
no pending transition. Readonly material is private in-memory RPC/env only. Existing
native generated 256-bit application material keys an explicitly database/domain-separated
HMAC readonly credential. Caller/import password inputs are not accepted by native
managed creation/rotation/upgrade contracts. Unreconciled sources fail rather than
falling back to app/migration privileges. Actual readonly/TLS preflight precedes app-only
PG18 custom gzip dump. Source identity changes invalidate old references.

Reports are actual scoped Core project/resource/build reads. CSV cells contain only
IDs, validated reviewed slug/enum categories, counts and observation time. Editable
project/resource names, secrets, URLs and logs do not enter the report. Actual Core/RLS
adversarial names starting `=`, `+`, `-`, `@`, tab and control characters were tested;
no executable cells or foreign-project names/rows appeared. This does not rely on CSV
quoting and does not claim that arbitrary free text is sanitized diagnostics.

Submitted diagnostics remain a separate finite allowlist and are explicitly labeled
submitted, not observed health. No raw build/exception text, credentials, tokens,
credential URLs or mail bodies are accepted. The existing redacted build string logs
remain excluded.

Core still owns all users/resources/reservations/jobs/entitlements/operations. Verified
S3 upload alone does not unlock a download: the exact resource's canonical Core job
must have completed successfully. Plans and every binary producer callback check
current original transfer authority and retained source permission/identity in addition
to ready state/epoch/expiry/size/hash. Job work is clipped to remaining lease minus a
safety margin and at most 35 seconds, respecting the 45-second Core attempt budget.
Output is bounded at 32 MiB, two producers, pg_dump at most 25 seconds. Explicit delete
revokes epoch before exact-key removal/absence acknowledgment; expiry remains distinct.

## Normal signed native package and actual Console

Final successful fresh run: `0b366d7b5095` (archives ports 19740–19743, separate from
other agents' 19640–19643). It used only fresh owned Core/metadata PostgreSQL, isolated
K3s, a local registry, processes and a TLS hop to the one existing MinIO. Existing
k3s/Kind/Contabo and the MinIO server itself were not modified or restarted.

The native archives image was actually compiled on pinned Rust 1.94.0 with pinned
protobuf compiler `3.21.12-11+deb13u1`, using the production-only runtime Cargo manifest
and a 342 KiB default-deny source context. Protected handovers/private inputs were
excluded. Runtime is pinned official PostgreSQL 18.6, UID/GID 65532, readonly root,
private metadata and bounded staging. Real `pg_dump` reported 18.6. The public CA bundle
comes from the pinned build base. Explicit ring initialization precedes AWS/tonic TLS.

Final release 0.1.0 was signed with the fixture's explicit ephemeral Ed25519 publisher,
reviewed through the normal operator catalog/install API and applied/promoted by the
actual agent. No accepted release/image was edited in place.

- OCI child manifest: `sha256:fe45030f00056fc092234392eb6b1ce8c9f24946be2002f2f0e32296a05a58e8`
- OCI config: `sha256:026d0199275dd0f13ba6cd045bf0a6b52ca774f45e36849b9cc3ac97098205ec`
- Reviewed manifest SHA-256: `b46a7e0f378d1e38b4a0469c4082a09b32fcccd371799bef42a7bbc7ddfbdac7`

The run performed a genuine password change, TOTP, PKCE/SSO and Registry-created real
customer/project, then actual archives create/generate, durable Core claim/completion,
Core observed report, private source RPC and verified binary GET. Report was 112 bytes,
SHA-256 `d8072ee151b2971e6c3fbad385cbc8f4a9e212acae33ace1ae8dfeac1ffc65a9`.
Foreign tenant/project admission was denied; public descriptors contained no provider
URL, bucket or credentials. A real 15-second resource proved old-descriptor/new-plan
expiry denial and explicit expired deletion. Explicit deletion was tested against a
fresh, previously successful descriptor, not an already expired one.

Repository Playwright workflow used the actual operator session and current Console.
At 375 and 1280 widths it found no overflow or page errors. It exercised custom native
Create → Generate → current job/status → known-SHA Blob GET → Hide → explicit Delete,
plus object URL retirement. Screenshots remained private. Unit UI tests independently
prove frozen retry input/action/key and no descriptor/blob in query or mutation caches.
Read controls use declared actions/indexed resource; writes also require feature and
writable state, preserving the Console's reader discovery semantics.

Two earlier owned runs (`8033cd8d8a47`, `88140b4ac3a2`) exposed fixture CA permissions.
They were not called passing: public CA parent/file permissions were corrected, checked
as nonroot before final install, and each failed run was exactly cleaned. Bounded static
startup stage codes aid diagnosis without echoing configuration/provider errors.

## Actual managed PG source and transition proof

The database owner ran the new `connected_export.rs` helper against its actual Native
Create/resize/rotate source, not seeded Ready rows or copied-platform metadata. Actual
source database `107834380252094464`, backend `107834380231122944`, CNPG UID
`4ea532f3-f18a-457c-b0b4-ea92952ffd33`. Material remained env/memory only, verified libpq
TLS and actual PG18.6 app-only custom gzip dump. `app.review_probe(id=1)` counted one.
Readonly ACL proof disabled transaction readonly in a separate command, verified `off`,
and required SQLSTATE 42501 for DML/DDL, preventing a false pass from readonly-transaction
or unrelated errors. No inherited/privileged role authority was present.

After an additional actual Native rotation, old source reference was denied and new
binding/derived readonly login/data/dump requalified. Rotated dump: 5277 bytes,
SHA-256 `9a967ff74e276ad9b4f2959af387ed0adda01695978a8fbfd207d6744d040551`.
Actual observed recurrence/WAL/retention stop followed by Native pause denied both old
material and new source reference. Registry project/customer/Default cascade remained
blocked before the stop and succeeded only after actual stop/pause. No SQL row repairs,
controller impersonation or fake browser principal were used. The domain owner independently confirmed exact source/Core/MinIO/Kubernetes cleanup,
loop-backed volume detachment and destroyed fixture credentials in
`2026-10-25-native-postgresql-backup-transitions.json`; sanitized evidence remains private;
this archives task did not delete or change those owned resources.

## Commands and results

```bash
cargo test --locked -p webdock-export-protocol -j 2
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --features local-test -j 2
python3 tools/tests/archives-storage.py
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --test repository -j 2 -- --include-ignored
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --test reports -j 2 -- --include-ignored
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-test -j 2 -- -D warnings
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-native-fixture -j 2 -- -D warnings
bun test --tsconfig-override apps/console/tsconfig.json plugins/archives/tests/ui.test.tsx
bun --bun eslint -c apps/console/eslint.config.mjs plugins/archives/ui/*.tsx tools/tests/native-archives-*.mjs --max-warnings 0
cd apps/console && bun --bun tsc --noEmit
```

Observed: 2 source/HMAC contract tests, 7 ordinary plugin tests; 3 separately owned
local dump/readonly/Core tests; repository job/lease test and observed-report/RLS test;
2 UI tests all green. Compile-only connected helper target is not counted as a behavior
test. Both fixture and normal Clippy configurations passed. TSC, ESLint, Python syntax
and new-file Rust formatting were checked. Real managed source/rotation/pause proof
ran in the database owner's connected Native caller. Parent owns complete root quality.

Native stages actually executed (final run; intermediate stages/IDs private):

```bash
node tools/tests/native-archives-fixture.mjs prepare <fresh-run>
node tools/tests/native-archives-fixture.mjs start-watchdog <run>
node tools/tests/native-archives-fixture.mjs prepare-minio <run>
node tools/tests/native-archives-fixture.mjs build-version-images <run>
node tools/tests/native-archives-fixture.mjs pin-binaries <run>
node tools/tests/native-archives-fixture.mjs bootstrap <run>
node tools/tests/native-archives-fixture.mjs artifacts <run>
node tools/tests/native-archives-fixture.mjs prepare-transports <run>
node tools/tests/native-archives-fixture.mjs start-core <run>
node tools/tests/native-archives-fixture.mjs start-ui <run>
node tools/tests/native-archives-acceptance.mjs setup <run>
node tools/tests/native-archives-fixture.mjs finalize-catalog <run>
node tools/tests/native-archives-fixture.mjs start <run>
node tools/tests/native-archives-acceptance.mjs install <run>
node tools/tests/native-archives-acceptance.mjs flow <run>
node tools/tests/native-archives-browser.mjs <run>
node tools/tests/native-archives-acceptance.mjs expiry <run>
node tools/tests/native-archives-acceptance.mjs delete <run>
node tools/tests/native-archives-fixture.mjs cleanup <run>
```

Cleanup for all three owned runs passed: exact process/container IDs, anonymous volumes,
network and owned image tags removed. Producers stopped before exact bucket multipart
abort/removal; incomplete uploads absent, exact bucket/user/policy removed. Existing
MinIO identity and running state preserved. The separate owned reviewed image tag was
removed by its observed exact ID. Shared/base build caches were not pruned. Only private
fixture evidence remains, not credentials in this record.

## Frozen migrations, scope and remaining limits

0050 SHA-256 remains
`2d3e22026c7ad792234ecce7f5a286a856eac681a22af504c23351d2de86a714`.
0051 (database owner) remains
`a4f16036e77bd2617f61cecc7c7881bb1abfceb920a16309e092770094ed7767`.
0046–49, original backup/storage protocols and object-storage producer/manifest were
not rewritten. Existing agent SQL/lifecycle/HBA hooks were coordinated and applied by
the database owner; this task authored only the new role helper and source helper.
Console api/storage/sdk files remained owned by the peer and unchanged here.

Native qualification explicitly uses the compile-time fixture HTTPS/verified CA profile
for the known MinIO ACL/multipart capability gaps. Production default rejects that
profile and does not pretend MultipartAbort lifecycle works locally. No Wasabi live
account, commercial/provider capability, production signing approval or production
activation is claimed. Provider TTL is asynchronous. Process crash during upload and
lost delete response were not separately fault-injected; retained uncertainty/fail-closed
state and verified provider lifecycle remain the recovery boundary. No public GC API.

Committed-source protocol qualification used a real private repository-protocol fixture
and Core leases, not a live GitHub account. History, runtime state, submodules and LFS
hydration are not promised. PG output is app-schema logical export with finite limits,
not PITR or a shared-table tenant export. General reports are supported typed Core
projections, not arbitrary caller SQL/templates/logs. Source/group/time/byte limits fail
honestly. No autonomous schedule/queue/auth truth or synthetic standing Session was added.

## Exact task file inventory

New task-owned files (including the accepted local foundation, now extended):

- `crates/agent/src/database/exports.rs`
- `crates/build/src/export.rs`
- `crates/control-plane/src/plugin_exports.rs`
- `crates/databases/src/export.rs`
- `crates/databases/src/service/exports.rs`
- `crates/databases/tests/support/connected_export.rs`
- `crates/export-protocol/Cargo.toml`
- `crates/export-protocol/build.rs`
- `crates/export-protocol/src/lib.rs`
- `crates/export-protocol/tests/contracts.rs`
- `crates/persistence/migrations/0050_archive_source_leases.sql`
- `crates/plugin-host/src/exports.rs`
- `docs/operations/protected-archives.md`
- `docs/superpowers/plans/2026-10-25-protected-archives-r2.md`
- `docs/superpowers/plans/2026-10-25-protected-archives.md`
- `docs/verification/2026-10-25-protected-archives-r2.md`
- `docs/verification/2026-10-25-protected-archives.md`
- `plugins/archives/.gitignore`
- `plugins/archives/Cargo.lock`
- `plugins/archives/Cargo.toml`
- `plugins/archives/Dockerfile`
- `plugins/archives/Dockerfile.dockerignore`
- `plugins/archives/README.md`
- `plugins/archives/fixtures/Dockerfile.native`
- `plugins/archives/fixtures/Dockerfile.native.dockerignore`
- `plugins/archives/manifest.json`
- `plugins/archives/migrations/0001_metadata.sql`
- `plugins/archives/runtime/.gitignore`
- `plugins/archives/runtime/Cargo.lock`
- `plugins/archives/runtime/Cargo.toml`
- `plugins/archives/src/backend.rs`
- `plugins/archives/src/diagnostics.rs`
- `plugins/archives/src/engine.rs`
- `plugins/archives/src/fixture.rs`
- `plugins/archives/src/lib.rs`
- `plugins/archives/src/main.rs`
- `plugins/archives/src/model.rs`
- `plugins/archives/src/producer.rs`
- `plugins/archives/src/sources.rs`
- `plugins/archives/src/store.rs`
- `plugins/archives/tests/connected_export_compile.rs`
- `plugins/archives/tests/core.rs`
- `plugins/archives/tests/diagnostics.rs`
- `plugins/archives/tests/lifecycle.rs`
- `plugins/archives/tests/manifest.rs`
- `plugins/archives/tests/owned.rs`
- `plugins/archives/tests/readonly.rs`
- `plugins/archives/tests/reports.rs`
- `plugins/archives/tests/repository.rs`
- `plugins/archives/tests/source.rs`
- `plugins/archives/tests/ui.test.tsx`
- `plugins/archives/ui/index.tsx`
- `plugins/archives/ui/nav.ts`
- `plugins/archives/ui/tsconfig.json`
- `proto/webdock/export/v1/export.proto`
- `tools/tests/archives-storage.py`
- `tools/tests/native-archives-acceptance.mjs`
- `tools/tests/native-archives-browser.mjs`
- `tools/tests/native-archives-fixture.mjs`
- `tools/tests/native-archives-proof.mjs`
- `tools/tests/native-archives-proxy.py`

Shared-file edits are additive source/module/Cargo/RPC wiring; other concurrent product changes in these files are not attributed to this task:

- `Cargo.lock`
- `crates/agent/Cargo.toml`
- `crates/build/Cargo.toml`
- `crates/build/src/service.rs`
- `crates/build/src/repository.rs`
- `crates/control-plane/Cargo.toml`
- `crates/control-plane/src/lib.rs`
- `crates/control-plane/src/plugin_config.rs`
- `crates/control-plane/src/plugins.rs`
- `crates/control-plane/src/bin/webdock-plugin-control.rs`
- `crates/databases/Cargo.toml`
- `crates/databases/src/lib.rs`
- `crates/databases/src/service.rs`
- `crates/plugin-host/Cargo.toml`
- `crates/plugin-host/src/lib.rs`
- `crates/plugin-host/src/authority.rs`
- `crates/plugin-host/src/storage_admission.rs`

Existing agent SQL/lifecycle/HBA/protocol hook files and 0051 belong to the database owner, not this inventory. Root TODO/quality runner, root package.json/bun.lock/patches/database-runtime, CMS/build evidence/artifact/worker/release code and Console api/storage/sdk were not edited here.
