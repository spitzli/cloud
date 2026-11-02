# Completed buildlogs r4: implementation and actual native0.3 qualification

Date: 2026-10-25. Machine evidence:
[2026-10-25-completed-buildlogs-r4.json](2026-10-25-completed-buildlogs-r4.json).
Implementation and owned acceptance are complete; Root owns the independent post-fix
review and full repository quality. Earlier r2/r3 evidence, accepted images, receipts
and intermediate failures are preserved. No commits, delegation, production activation,
shared native SQL/protobuf changes or Root-lock changes were made by this task.

## Actual source and honest log contract

`buildlog` is a separate source/producer, not submitted diagnostics. Source selection
uses a real Core build ID. The new `BuildService` readers load actual stored
`git_build.logs` under the freshly authorized RLS transaction. Only succeeded/failed
rows with a completion hash and finished time qualify. Running/queued/cancelled,
foreign, missing, oversized or disabled-source rows fail. Both a successful build and
an actually failed build were qualified; their archive jobs completed successfully.

The binding fingerprints exact stored-byte SHA-256, completion hash, generation,
terminal status, finish time, original build/source/worker lineage and current
source/connection/environment/hosting configuration identity. Active exact customer/
project, `org:deployments:create`, hosting mode and effective native operator rules are
checked again during reads and retained downloads. The existing Core original
actor/session/browser/API authority, real job/attempt/claim, runtime/grants and canonical
completed-job receipt remain authoritative. There is no second queue or user ledger.
The trait extension denies by default and uses existing private `Stream`/`Validate`;
no new RPC or native migration was needed.

Maximum original **stored UTF-8 bytes: 24,000**; maximum gzip: **64 KiB**. Larger rows
are refused. The archive preserves the stored string exactly; it does not re-redact
against today's environment or manufacture summaries. The existing build pipeline
replaces known environment literal values and already bounds captured output.
Transformed/unknown secrets can remain, output may be truncated, and a historical
redaction version is not recorded. No complete stdout, complete secret removal or
secret-free diagnostic promise is made. The binding-domain version identifies its
identity format, not a redaction attestation.

Public archives actions, job payloads and status carry metadata/IDs only. Source bytes
use private binary RPC, anonymous private staging, gzip, shared verified S3 and the
existing authenticated opaque gateway. The Console uses the frozen generic
`usePluginScope`/`downloadBinary` boundary, gives an explicit sensitivity/truncation
notice, and has no raw text, HTML or terminal preview. Original Core build-log retention
remains the Build domain's policy: deleting this export deletes its exact archive
object, not that original Core record.

## RED→GREEN and private stream revocation review fix

Observed RED before implementation:

- The protocol and private receipt rejected unknown `buildlog` kinds.
- The new source snapshot's deny stub rejected a valid completed actual-byte fixture.
- The real component lacked a buildlog choice and its sensitivity/truncation notice.

GREEN covers both terminal outcomes; changed same-generation bytes, completion hash,
status, finish time and current source configuration change the reference. Running,
queued, cancelled, foreign, unreceipted/disabled and 24,001-byte rows are denied.
A 24,000-byte multibyte UTF-8 boundary is tested. Offline Unix RPC tests preserve exact
24,000 stored bytes through gzip and refuse oversized/revoked source output. The
receipt itself refuses more than 64 KiB. Manifest tests reject `logs`, `content`,
`bytes`, URLs and filesystem paths as buildlog input. The UI test submits only the
Core ID and retains existing frozen Create/Generate/Delete retry behavior.

Independent review r1 confirmed a private-stream P2: after source read the old frame
loop only revalidated invocation/job authority. It could emit cached bytes after Git
source revocation. The fixed frame driver retains the resolved reference, sources
adapter and original handle; before **each metadata/data/EOF poll** it reacquires a
fresh `host.authorized` scope/transaction, runs `sources.validate`, then `host.finish`.
No old scope/transaction is retained across frames. Project/report paths share this
guard, and producer cancellation/output budgets remain unchanged.

Two controlled offline frame tests were observed RED→GREEN: held buildlog metadata →
source revoke → first data denied, and metadata/multiple-frame source revocation.
These are frame-contract mocks, not claimed SQL authority proof. The additional actual
Core/RPC test ran against fresh frozen<=53 schema: normal normalized Project source,
real Core archive job and claim, metadata delivered, first-data adapter barrier,
normal **BuildService git.connections.disconnect**, original job proof still active,
then no cached data. This exercises the same production frame driver and actual SQL
source guard; **21.13 seconds GREEN**. Existing managed-mode/native-operator, API-token
revocation, source-generation and exact cancellation-lineage cases remain in that test.

## Actual KVM → Core → native producer → S3 → gateway → browser

Fresh bounded fixture: **184aa2c46c38**, ports19740–19743. It reused the one existing
MinIO, a fresh owned private Core/metadata PostgreSQL, own4GiB/3CPU K3s, local registry,
TLS proxy and normal copied Core/Agent executables. Native hard eviction thresholds
include memory100Mi, nodefs/imagefs available1%, inode free1%, transition10s; existing
clusters/services were not changed. Frozen0052/0053 hashes were checked before/after
bootstrap. The actual normal operator password/TOTP/PKCE, Registry-created customer/
project, signed catalog install and native enrolled agent promotion were used.

Accepted **0.3.0** artifacts:

- OCI child `sha256:1b432d2f2ce06b8c88c4102ecc4435158092393acec51383bb7708558ecd1f87`
- OCI config `sha256:6ac26201995b476c23fcaefaef04adc474ec6715d5f49e029a787c8cc9571761`
- Signed reviewed manifest SHA `e1db1cdfdd53406493cc380b3b4a31bca74090f742b07093164fde90c96cf699`
- OCI archive SHA `5c03b703214a55b67ba8223206023bfd5f402c0aed2861d71e7e58d1e6eb0bfa`

The qualifier verifies the actual Native operator cookie, uses existing HostingService/
BuildService APIs to prepare a draft target/configure source/request/enroll/claim, runs
the reviewed immutable KVM image against real local committed fixture source, checks
the live build lease during execution, and calls normal Core completion. This is
actual domain completion into the canonical database, not a claim of HTTP worker-
transport qualification. Only fixture subscription/Git/builtin configuration is seeded;
completed/Ready rows and actors are not fabricated. The real enrolled K3s identity and
actual Registry customer/project are reused. KVM customer commands execute in the VM,
not on the host; no persistent provider credential enters it. Fixture VM limits are
stricter than the Core claim:2CPU/2GiB/180s; archive production still respects its own
remaining Core lease minus safety margin and maximum35s.

Actual succeeded build **107910814727413760**, 954 stored bytes:
`8e91f626cfd6e75b442a42363dda47cef872c6da95df4c8e042436393a0683ae`.
Actual failed build **107910939931582464**, 1359 stored bytes:
`d240fed29d4283e14f8d58f84362e38cba97733810b4f14e46cf71929ceb868e`.
Both contain observed VM output and mask the explicitly echoed known fixture literal.
Archive decompression reproduced each stored byte-count/hash exactly. The worker was
normally revoked after completion. A same-image/generation producer pod restart retained
both durable receipts and working original opaque descriptors.

Actual Chrome/Console proof at375/1280:

- Successful and failed sources produce verified known-SHA Blob downloads.
- Sensitivity/truncation notice present; no log text, HTML or terminal preview, no
  overflow or page errors; private screenshots retained.
- A genuine native member used invitation, one-use activation, password login and PKCE.
  Initial metadata denial correctly reflected missing product entitlement. The operator
  selected **only the owned customer** for Exports via the supported policy API; no
  role/capability code or global Everyone policy was changed. With entitlement, project/
  archive status returned200, buildlog sources were not disclosed, and download returned
 409 without a descriptor because the member lacks `org:deployments:create`. Conflict409
  is the producer's normal source-denial mapping; it is not success or log data.
- A real15-second log archive denied its old ticket and new download after expiry and
  was explicitly deleted.
- Exact owned stored-log corruption was injected without changing the completion receipt
  or repairing it. A **fresh, previously successful** descriptor was denied; authorized
  exact deletion after the changed source passed.
- Normal Git disconnect denied a **fresh, previously successful** descriptor. A pending
  buildlog's Generate then failed in the actual UI; Refresh followed by Delete succeeded
  with the separate Delete identity. No frontend cancellation was claimed to stop a job.
- Normal report source/job/download, report15-second expiry and explicit deletion stayed
  green on0.3. Earlier managed-PG source/rotation/pause proof remains separately preserved;
  the PG producer path was not changed by this addition.

## Preserved compiler closures and accurate failed attempts

Nine exact Core/Agent ELFs were copied0500 before native bootstrap against<=53; their
hashes are in the JSON. Current Root54 changes never replaced them.

The first helper `4b68bb58eaec146492544ed896485b21efdfbbed8da86dd2fb70279efcb64620`
created a legitimate request/claim but failed before completion. Its actual build
**107903467456045056** was retained, then normally cancelled and its worker revoked.
Both its running and cancelled states were proved ineligible for buildlog creation.
No successful VM outcome was replayed or invented. The qualifier had used a default-
mode temporary artifact directory; its private-directory guard was corrected explicitly.
The initial diagnostic could not establish the exact VM stage, and the earlier
"before KVM" statement was corrected from read-only state inspection.

Helper-r2 `ed66b27861798856cc61d644067aa3d43054eb76dc47b227d3c3e598119be343`
used explicit0700 temporary directories and fixed stage/validated error-code diagnostics.
It reached a draft app but a static fixture Git installation identity collided with the
retained first connection. Read-only state proved no new Git/source/build/VM existed.
Helper-r3 used an allocated unique Git installation identity, replayed only identical
r2 Hosting/App bodies with their original keys, and derived subsequent actual build
request keys from the new installation. SourceConfigure retains its existing revision
CAS; no unsupported idempotency field was invented.

Qualified helper-r3:
`67ded22bfe0646f188b2baf53b632e65a73e7c23f9f9700f062f8c6a216c603f`.
It was built using actual **Rust1.94.0 --offline --locked** in a new private closure,
not current54 workspace libraries. Base preserved source archive:
`0c004d3ffb710a5fdebcb213500fe93fca60243f6bc274646fd80bf824e13438`.
Public base pin: `2026-10-25-native-recovery-52-pin.json`, verified `pinSHA256` field
`30cac5cab2df52a4bb5b698652e36d580a5c8a325c515443bcd2d8f6e9b8580e`.
The document file byte SHA is separately recorded as
`f7f2544f05f6fa4f861eaa806135015f5bdcf6f163e7ae0cf852697c7cd5f8b1`.

Only selected Cargo/Rust/proto/vendor/archives files were materialized. Absolute paths,
traversal, links, protected/private paths and symlinks back to the workspace were refused.
The initial include-dir miss was resolved by copying retained drizzle and baseline-hash
inputs only from that verified archive. Exact frozen0053 and the versioned own helper
were the overlays. All **804 root registry versions/checksums** remained unchanged;
path dependencies stayed inside the closure. All12 published Frontend53 production
source hashes (Build files and hosting/git) matched; this is not a claim that every
library in the closure equals an original whole53 tree. The provenance is explicitly
**preserved52 + frozen53/helper overlay**. Oldr1/r2/r3 ELFs, source maps and failures remain;
only exact owned compiler target caches were removed after qualification. A final
workspace fixture-only Clippy cleanup collapses two diagnostic `if` statements; the
qualified r3 source/ELF pins remain untouched.

## Commands, results and exact files

```bash
cargo test --locked -p webdock-export-protocol --test contracts -j 2
cargo test --locked -p webdock-build --lib service::export::logs::tests -j 2
cargo test --locked -p webdock-plugin-host --lib stream_tests -j 2
cargo check --locked -p webdock-control-plane -j 2
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --features local-test -j 2
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --test repository -j 2 -- --include-ignored
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-test -j 2 -- -D warnings
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-native-fixture -j 2 -- -D warnings
bun test --tsconfig-override apps/console/tsconfig.json plugins/archives/tests/ui.test.tsx
(cd apps/console && bun run test)
bun --bun tsc -p apps/console/tsconfig.json --noEmit
bun --bun eslint -c apps/console/eslint.config.mjs plugins/archives/ui/*.tsx tools/tests/native-archives-*.mjs --max-warnings 0
```

Observed: protocol3, source-snapshot2, offline frame2, offline archive12, UI4 and Console101
GREEN; actual Core/RPC integration21.13s GREEN. Core check after frame correction5.44s
GREEN. Owned Clippy/TSC/ESLint and direct skip-children Rust formatting passed. The initial
Root Core/Agent build succeeded46.45s with7 warnings in Mail's in-progress Agent recovery
library; no unrelated production lint edits were made. A root-directory TSC invocation
initially found no project; the explicit Console project command above was then run.
Final fixture Clippy found two collapsible diagnostic `if`s; those were corrected without
altering the already used pinned helper.

Native stages, all on the exact run above:

```bash
node tools/tests/native-archives-fixture.mjs prepare 184aa2c46c38
node tools/tests/native-archives-fixture.mjs start-watchdog 184aa2c46c38
node tools/tests/native-archives-fixture.mjs prepare-minio 184aa2c46c38
node tools/tests/native-archives-fixture.mjs build-version-images 184aa2c46c38
node tools/tests/native-archives-fixture.mjs pin-binaries 184aa2c46c38
node tools/tests/native-archives-fixture.mjs bootstrap 184aa2c46c38
node tools/tests/native-archives-fixture.mjs artifacts 184aa2c46c38
node tools/tests/native-archives-fixture.mjs prepare-transports 184aa2c46c38
node tools/tests/native-archives-fixture.mjs start-core 184aa2c46c38
node tools/tests/native-archives-fixture.mjs start-ui 184aa2c46c38
node tools/tests/native-archives-acceptance.mjs setup 184aa2c46c38
node tools/tests/native-archives-fixture.mjs finalize-catalog 184aa2c46c38
node tools/tests/native-archives-fixture.mjs start 184aa2c46c38
node tools/tests/native-archives-acceptance.mjs install 184aa2c46c38
node tools/tests/native-archives-acceptance.mjs flow 184aa2c46c38
node tools/tests/native-archives-buildlogs.mjs setup 184aa2c46c38
node tools/tests/native-archives-buildlogs.mjs flow 184aa2c46c38
node tools/tests/native-archives-buildlog-browser.mjs 184aa2c46c38 read
node tools/tests/native-archives-buildlogs.mjs expiry 184aa2c46c38
node tools/tests/native-archives-buildlogs.mjs corrupt 184aa2c46c38
node tools/tests/native-archives-buildlog-browser.mjs 184aa2c46c38 disconnect
node tools/tests/native-archives-acceptance.mjs expiry 184aa2c46c38
node tools/tests/native-archives-acceptance.mjs delete 184aa2c46c38
node tools/tests/native-archives-fixture.mjs cleanup 184aa2c46c38
```

Private closure compile used `CARGO_TARGET_DIR=<owned compiler-r2/target>` and
`cargo +1.94.0 build --manifest-path <owned compiler-r3/plugins/archives/Cargo.toml>
--example native_buildlog_source --offline --locked -j2`. The r3 final helper rebuild
passed1.62s after the prior source-closure build15.61s; exact inputs/overlays are pinned.
No current54 tree was used to rebuild the qualifier or native executables.

Changed files in this round:

- `crates/build/src/export.rs`, new `crates/build/src/export_logs.rs`
- `crates/export-protocol/src/lib.rs`, `crates/export-protocol/tests/contracts.rs`
- `crates/plugin-host/src/exports.rs`, `crates/control-plane/src/plugin_exports.rs`
- `plugins/archives/src/{engine,model,sources}.rs`
- `plugins/archives/ui/index.tsx`
- `plugins/archives/tests/{source,manifest,core,repository}.rs`
- new `plugins/archives/tests/buildlog_bytes.rs`, `plugins/archives/tests/ui.test.tsx`
- new `plugins/archives/examples/native_buildlog_source.rs`
- `plugins/archives/{Cargo.toml,Cargo.lock,manifest.json,README.md}`
- `plugins/archives/runtime/{Cargo.toml,Cargo.lock}`
- `tools/tests/native-archives-fixture.mjs`
- new `tools/tests/native-archives-buildlogs.mjs`
- new `tools/tests/native-archives-buildlog-browser.mjs`
- `docs/operations/protected-archives.md`
- new `docs/verification/2026-10-25-completed-buildlogs-r4.{md,json}`

No build worker/evidence/service/release/repository/hosting/git production files,
Console binary helper files, Root locks, native SQL or protobuf contracts were edited.
New dependencies were not needed; owned package versions alone advanced to0.3.0.

## Cleanup and limits

All custody entries are removed: exact owned processes, Docker IDs, anonymous volumes,
images and network. Exact MinIO bucket/user/policy were removed, incomplete uploads
explicitly aborted and observed absent. Existing MinIO identity remained running;
existing Kind/k3s/Contabo and other agents' resources were untouched. Cleanup receipt:
`/tmp/archives-buildlog-native-cleanup.log`. Private receipts, source pins, bounded logs
and screenshots remain under `.runtime-test/native-archives/184aa2c46c38`. Frozen<=53
migration hashes and804 registry pins were reverified after cleanup. Later Root54
files are a separate phase and were not included in this snapshot's DDL checks.

Logs remain potentially sensitive stored excerpts with known-literal masking only;
no full-output/redaction-version/complete-secret-removal promise exists. Native MinIO
exceptions remain explicit fixture-only limitations, not production Wasabi approval.
Production activation/signing/commercial provider qualification, original Core log
purge policy and full Root quality remain separate. Root's independent post-fix review
is reported independently from this owned acceptance.
