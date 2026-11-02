# Protected archives r3: confirmed review fixes

Date: 2026-10-25. All three confirmed findings have RED→GREEN regressions.
Sanitized machine evidence: [2026-10-25-protected-archives-r3.json](2026-10-25-protected-archives-r3.json).
The original r2 evidence and its immutable release 0.1.0 receipts remain unchanged;
this qualification uses a fresh release **0.2.0** and run **9facb9e2fa21**.
No commits, delegation, production activation, migration changes, SQL repairs in the
native fixture, old-store migration or changes to retained clusters/services occurred.

## P1: PostgreSQL cannot use legacy authority

RED was observed twice: `Archive::ready` accepted a historical PG receipt without a
Core source, and the real owned MinIO/private-metadata test accepted nonempty legacy
`databaseSources` with fixture features compiled. Both failed for the intended
assertions, and the failing owned run was exactly cleaned.

The executable now rejects nonempty `databaseSources` before any transport/backend
startup; the ordinary Engine constructor independently refuses them. The old
loopback dump/fingerprint implementation and Engine producer branch are removed.
Every observed producer requires a type-matching Core `SourceRef` and matching stored
binding; historical unqualified rows cannot generate, issue a plan or authorize an
already issued ticket. Status and authorized exact deletion remain available.
Submitted, strictly structured diagnostics intentionally remain source-free.

GREEN proves more than a new-request check. An existing private ready receipt/object
is represented as a historical unqualified PG row: source access, availability,
generation, plans and the old ticket all fail before deletion; exact deletion removes
the object and remains idempotent. The real Core/mTLS test demotes a genuine tenant
member to reader, independently verifies `org:databases:manage` is false, admits its
ordinary resource read, then rejects the legacy PG download and old binary callback.
Restored authorized ownership can explicitly delete that same receipt. Existing
canonical Core completion, membership/feature revocation, staged-byte denial,
receipt/hash/corruption, expiry and diagnostic gzip tests stay green.

The actual signed 0.2.0 native executable was also invoked inside its own nonroot,
readonly-root workload with a private legacy-config probe. It exited specifically
`archives_plugin_start_failed_legacy_database_sources`, before network startup, and
removed both exact private probe files. Actual public PG Create without `source_id`
was denied even for the normal authenticated operator.

## P2: retained committed-source authority includes current hosting mode

The real existing repository-protocol fixture produces normalized committed bytes
under the exact Core archives job/attempt/claim. After successful Core completion it
issues an opaque transfer descriptor and validates its retained source successfully.
A normal `HostingService` **projects.update** then changes selfservice → managed.
RED: the already issued descriptor still passed source validation.

`crates/build/src/export.rs` now checks the current hosting mode alongside current
`org:deployments:create`, exact actor/session and existing provider/source authority.
The managed exemption requires the effective RLS operator plus the live native
operator mapping, matching the qualified repository authority; it does not inspect a
caller-supplied role string. Browser/API authority continues through the existing host
and storage checks. No frozen SQL, shared gateway/protocol ABI, or old Session/Build/
Release implementation changed.

GREEN: the old descriptor and fresh retained-source read fail for the tenant owner in
managed mode; a qualified native operator remains permitted; removing its native
operator mapping denies the same source despite the remaining `user.role=operator`.
The test also binds a read-only workspace API token to a separate opaque descriptor:
it initially validates, then source callback validation fails after actual token
revocation even with the original browser caller still current. Rotating the actual
Core repository connection generation denies the old source/descriptor; a fresh
reference observes a different binding. Existing cancellation still retires the exact
job proof and repository callback lease. Final run: **13.31 seconds**.

The fixture uses an existing protocol runtime and a disposable Core database; it is
not a live GitHub account qualification. Its operator/membership/API/connection changes
are explicit fixture authority transitions, not production state repairs. A separate
attempt to test removal of a referenced hosting row was correctly rejected by the
existing source/project foreign key; no FK was disabled and that impossible-state
probe is not claimed as an export regression.

## P2: Delete supersedes a failed Generate submission

RED: the real component regression fails because failed Generate disables the delete
confirmation, including after Refresh. The fix permits confirmation after that failure
and selects a retry only when its action matches the requested action. Explicit Delete
discards the Generate attempt and freezes its own action/body/idempotency key. Generate
retry retains its original identity; uncertain Delete retry retains the Delete identity.
An in-flight request remains busy. Frontend cancellation makes no claim about backend
job cancellation or completion; the server retains mutation/cancellation authority.

GREEN component tests exercise repeated Generate failure after a source-binding change,
Refresh, explicit Delete, a lost Delete response and identical Delete retry. The actual
native browser additionally forwards Generate to the real Core, verifies its accepted
job response, deliberately loses that response, refreshes status and explicitly deletes
with a distinct identity. This tests an accepted/uncertain backend outcome, not a fake
cancelled job. Ordinary Create → Generate → Status → Download → Hide → Delete also passes
at 375/1280 widths, with known-SHA Blob GET, no page errors and no overflow.

## Fresh signed native artifact, health and exact cleanup

Normal operator password/TOTP/PKCE, catalog review/install and agent promotion were used.
Ports 19740–19743 were separate from the object-storage qualification's 19640–19643.

- OCI child: `sha256:f214b02d9accede88aebeb8188453457bb869badcc56181576880dd6c80c3898`
- OCI config: `sha256:7d20f1a7235cc7af42a820dd4559ca871eccff402c5f9c57f8b5199e6cd61697`
- Signed reviewed manifest SHA-256: `38852da71e8bb550d299865206828a9245ac01b1d87ec2bcfc54efc26b2c41e0`
- Report: 112 bytes; SHA-256 `5244c210b7a9189d01c83926d7902d41a50af6cbeb530bd9ac576e7666e120a0`

The image was separately compiled with the pinned Rust 1.94 / PG18.6 production tool
closure and explicit compile-time native fixture profile. Production runtime dependency
pins were preserved; own package/release versions advanced to 0.2.0. The Core correction
was built with `--locked`; nine exact ELF files were copied into private immutable 0500
fixture paths before bootstrap. The JSON evidence records their hashes.

Initial install observation timed out on genuine owned-node DiskPressure: the host had
about 88 GiB free, below kubelet's relative default. Only the fresh, bounded test node
was adjusted: **4 GiB / 3 CPUs**, explicit hard floors `memory.available=100Mi`,
nodefs/imagefs available `1%`, inode free `1%`, transition `10s`. The first draft omitted
the memory threshold and was corrected before successful qualification. Unspecified hard
thresholds must not be assumed to retain defaults; see [Kubernetes eviction documentation](https://kubernetes.io/docs/concepts/scheduling-eviction/node-pressure-eviction/#hard-eviction-thresholds).
Actual kubelet `/configz` verified all five thresholds; actual MemoryPressure,
DiskPressure and PIDPressure were False and Ready was True. No taint/Ready override or
global prune was used. A dynamically reassigned owned Docker API port required refreshing
only the private kubeconfig and restarting the exact owned pinned agent with retained
enrollment/state. Kubelet had evicted the imported image: the exact hash-verified signed
OCI archive was reimported, without changing its bytes. Original install operation
`107868900179320832` subsequently succeeded normally at cursor 9; the original receipt
and initial failed observation were preserved. No repair SQL or new install was needed.

Real 15-second expiry denies both the old descriptor and a fresh download request;
explicit expired deletion passes. Normal deletion is tested using a fresh previously
successful descriptor, then denies that exact descriptor. Foreign project admission is
denied and public descriptors contain no provider URL or credentials.

**Exact cleanup verified:** every custody entry is removed; exact process/container IDs,
anonymous volumes, images, owned network and exact MinIO bucket/user/policy were removed.
Incomplete uploads were explicitly aborted and observed absent. The original MinIO
identity remained running. Existing k3s/Kind/Contabo and other agents' fixtures were
untouched. Private sanitized proof/screenshots remain under
`.runtime-test/native-archives/9facb9e2fa21`; cleanup receipt is
`/tmp/archives-r3-native-cleanup.log`. Root was explicitly notified that the Archives
compiler/schema/fixture hold is released; Mail0052 remains a separate parent-owned phase.

## Commands and observed results

```bash
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --test lifecycle -j 2
WEBDOCK_ARCHIVES_SKIP_CORE=1 python3 tools/tests/archives-storage.py
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --test repository -j 2 -- --include-ignored
bun test --tsconfig-override apps/console/tsconfig.json plugins/archives/tests/ui.test.tsx
CARGO_TARGET_DIR=target cargo test --manifest-path plugins/archives/Cargo.toml --locked --features local-test -j 2
python3 tools/tests/archives-storage.py
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-test -j 2 -- -D warnings
CARGO_TARGET_DIR=target cargo clippy --manifest-path plugins/archives/Cargo.toml --locked --all-targets --features local-native-fixture -j 2 -- -D warnings
cargo check --locked -p webdock-build --no-default-features
cargo clippy --locked -p webdock-build --features service --lib -j 2 -- -D warnings
cargo build --locked -p webdock-control-plane --bins -p webdock-agent -j 2
(cd apps/console && bun run test && bun --bun tsc --noEmit)
bun --bun eslint -c apps/console/eslint.config.mjs plugins/archives/ui/*.tsx tools/tests/native-archives-*.mjs --max-warnings 0
```

Observed GREEN: 8 ordinary plugin tests (5 owned tests correctly ignored in that suite),
3 separately executed owned MinIO/metadata/readonly/Core tests, 1 real Core repository
integration, 3 archives component tests and 101 Console tests. Strict Clippy, no-default
build, TSC, ESLint, owned Rust formatting and Python parsing passed. An initial Console
command pointed to `src` and found no tests; the actual package `bun run test` above
was then executed and passed all 101. Early repository fixture setup errors (multiple
prepared statements, missing namespace and unallocated allocator slot) were corrected
before the intended RED was observed; they are not counted as product failures/passes.
Only the added direct dev `webdock-hosting` dependency was needed for the real update API.

Native stages ran using the same helper commands as r2: prepare, start-watchdog,
prepare-minio, build-version-images, artifacts, pin-binaries, bootstrap,
prepare-transports, start-core, start-ui, setup, finalize-catalog, start, install,
reimport-artifacts, install observation, flow, browser, expiry, delete, cleanup.
All final acceptance stages and exact cleanup passed; failures/intermediate receipts
remain private. Frozen 0050 and 0051 hashes remain respectively
`2d3e22026c7ad792234ecce7f5a286a856eac681a22af504c23351d2de86a714` and
`a4f16036e77bd2617f61cecc7c7881bb1abfceb920a16309e092770094ed7767`.

## Exact changed files for this review round

- `crates/build/src/export.rs`
- `plugins/archives/src/{engine,main,model,producer}.rs`
- `plugins/archives/ui/index.tsx`
- `plugins/archives/tests/{core,lifecycle,owned,repository,source}.rs`
- `plugins/archives/tests/ui.test.tsx`
- `plugins/archives/{Cargo.toml,Cargo.lock,manifest.json,README.md}`
- `plugins/archives/runtime/{Cargo.toml,Cargo.lock}`
- `tools/tests/archives-storage.py`
- `tools/tests/native-archives-{fixture,acceptance,browser}.mjs`
- `docs/operations/protected-archives.md`
- `docs/verification/2026-10-25-protected-archives-r3.{md,json}`

Root release-claim/evidence/worker files, Console binary helpers, gateway ABI, frozen
migrations, protected handovers and local owner JSON were not edited in this round.

Limits remain 32 MiB, two producers, maximum 24-hour access, bounded Core/pg_dump budgets,
app-only dedicated logical PG export and committed normalized Git source. No new live
managed-PG installation/rotation was provisioned in this round; the previous actual
Native Create/resize/rotation/pause proof remains separately preserved in r2 and the
PG owner's transition evidence. The managed dump/preflight path is unchanged. MinIO
fixture exceptions do not qualify production Wasabi. Production activation/provider
approval stays gated; parent owns complete root quality.
