# Managed Object Storage browser/producer review fix — 2026-10-25

The confirmed checksum downgrade is fixed in the producer. An exact-version,
If-Match HEAD that advertises a valid `FULL_OBJECT` SHA-256 now requires the caller
to repeat that exact SHA in `objects.download`. Omission and mismatch are denied
before a private plan or public Core descriptor can be returned. The existing
UI repeats the checksum from `objects.observe`. Composite and unlabelled
checksums remain unknown; ETag is never interpreted as SHA. Supplied hashes on
unknown-checksum objects still reach Core's existing content verification.

The alternative of adding provider data to the private plan builder was not
needed: this uses the explicitly authorized denial behavior. No Core gateway,
public transfer ABI, caps/roles, migration, lifecycle rule or shared binary API
changed. The ownership and active-epoch checks before and after provider work
and the exact version/ETag checks remain in place.

The new SDK regression reaches actual AWS SDK HTTP deserialization with valid
full-object checksum metadata and exercises omitted, equal and wrong caller
hashes. It also checks composite/unlabelled cases, the HEAD method, version query,
If-Match and enabled checksum observation. This is a controlled local HTTP SDK
boundary, not evidence that the existing MinIO labels its hashes `FULL_OBJECT`.
The first run reproduced successful admission with the caller hash absent; the
fixed run denies it. Both logs remain in the private evidence directory.

Two new Create regressions mount the actual UI with real SessionContext and
project values, submit a pending request, switch principal or project, and then
resolve the old transport successfully despite its aborted signal. Both assert
that the signal was aborted and `onCreated` never navigates the new scope. The
existing frozen action/body/request UUID/Core idempotency-key retry tests pass.
The new regressions already passed against the existing guard; temporarily
removing its abort made both fail at the signal assertion, and restoration
returned them to GREEN. No Create implementation or reveal-hook change was
required.

Files changed in this follow-up:

- `plugins/object-storage/src/sdk.rs`: checksum guard and SDK HTTP regression.
- `plugins/object-storage/tests/ui.test.tsx`: pending principal/project Create
  cancellation regressions, using the existing real scope hook.
- `plugins/object-storage/manifest.json`: release 0.5.0; its source SHA is
  `7e55cbe1ce8ddd3e109550a81f86c1e17e662360b178f264c7ce5c6c508b2346`.
- `plugins/object-storage/fixtures/Dockerfile.native` and
  `tools/tests/native-object-storage-fixture.mjs`: separately compile immutable
  fixture versions 0.5.0 and 0.6.0; legacy accepted identities remain untouched.
- `tools/tests/native-object-storage-checksum-proof.mjs`: requalify actual native
  observation/admission/Core-byte semantics, recording the real provider result.
- `tools/tests/native-object-storage-evidence.mjs`: allowlisted checksum evidence.
- `plugins/object-storage/README.md`: exact omission-denial behavior.
- This new record and the new run evidence; prior native records are preserved.

Offline verification passed with actual Rust 1.94.0: default and explicit native
fixture suites, no-default check, and the owned PostgreSQL observation/epoch
regression. The focused browser/API/UI group passed 28 tests; all 101 Console
tests passed. Console typecheck, scoped ESLint and rustfmt, four fixture helper
tests and the proxy policy test passed. Rust 1.94's rustfmt component is absent;
the installed rustfmt checked the touched Rust file. An initially mistyped Python
unittest module invocation failed; the corrected direct-file command passed.
These setup errors and the checksum RED were retained, not reported as successes.

```sh
cargo +1.94.0 test --manifest-path plugins/object-storage/Cargo.toml --locked -j 2
cargo +1.94.0 test --manifest-path plugins/object-storage/Cargo.toml --locked --features local-native-fixture -j 2
cargo +1.94.0 check --manifest-path plugins/object-storage/Cargo.toml --locked --no-default-features -j 2
bun test --isolate apps/console/tests/api.test.ts apps/console/tests/plugin-reveal.test.tsx apps/console/tests/storage-binary.test.ts apps/console/tests/storage-ui.test.tsx plugins/object-storage/tests/ui.test.tsx
(cd apps/console && bun run test && bun run typecheck)
bun x eslint -c apps/console/eslint.config.mjs apps/console/src/api.ts apps/console/src/plugins/storage.ts apps/console/src/plugins/sdk.ts plugins/object-storage/ui/index.tsx
rustfmt --check --edition 2024 plugins/object-storage/src/sdk.rs
node --test tools/tests/native-object-storage-fixture.test.mjs
python3 tools/tests/native-object-storage-proxy.test.py
```

Fresh bounded fixture `d7a8429f1c60` completed normal signed native installation
and fresh browser qualification. Preparation and signing occurred during the
coordinated hold. After Root explicitly released the 0046–51 phase, the actual
current Core/agent build passed in 37.53 seconds; all nine ELFs were copied into
private mode-0500 fixture paths and hash-pinned before Archives changed its shared
export source. The local Core build used Rust 1.98.1; producer builds and fixture
images used Rust 1.94.0. Native migration hashes matched before/after bootstrap;
0052 was absent and remained held for this qualification. In particular:

- 0050: `2d3e22026c7ad792234ecce7f5a286a856eac681a22af504c23351d2de86a714`
- 0051: `a4f16036e77bd2617f61cecc7c7881bb1abfceb920a16309e092770094ed7767`

Release 0.5.0 image digest:
`sha256:b082dd78ed2e7a9dd53a0d945a00bd0a713a854d6a5e0b163feb2e2ff8fdbfc8`.
Its signed manifest SHA is
`7d8c53018b38bd1967509715f19e829269a3c48988df077404ad54eb9ed9bd22`.
Separately compiled/signed 0.6.0 image digest:
`sha256:1bf7ffcfc67a22fbf7bb832e2d1db13d36dc12367180839fb14b4761f0d5ec1b`.
Version 0.6.0 was not promoted; unchanged update/rollback semantics reuse the
previous generation-retention evidence. No accepted image identity was rewritten.

The initial accepted install `107867872000221184` became blocked with
`needs_reconciliation` during an owned-node restart. The disposable node had
reported real DiskPressure under its default percentage threshold despite about
91 GiB free. Only that exact node received a kubelet drop-in with explicit 8 GiB
node/image filesystem floors, 100 MiB memory floor, 5% inode floors and a 10-second
pressure transition. Kubelet itself cleared DiskPressure and reported Ready; no
taint or Ready value was patched. The exact working drop-in and intent/confirmed
receipts remain private. This was an explicit correction to this owned node;
production and default cluster settings were not changed.

The initial pressure also removed the imported producer image: its private-config
init container reached ImagePullBackOff. Reimporting the exact hash-verified OCI
archives restored the accepted bytes. No release, signature or provider was
changed. The documented, freshly authorized installation retry API accepted
`107868882722627584`. A normal reconnect of the exact pinned agent retained its
configuration, enrollment, state and empty plugin outbox. Retry completed and
actual mTLS Describe/Health promotion succeeded. The operation generation is 2;
the retained runtime generation is 1. Six object UIDs, the private storage identity
hash and sealing-key hash matched before and after retry. Both the blocked
snapshot and original accepted receipt remain private, and supported retry's
transition of the prior operation to failed is recorded accurately.

Runtime namespace is `wd-plugin-f2c09e7f9c85b6371e18dc46`; pod UID is
`7b549735-1398-4762-9951-35668c8a6d60`. Its actual container ID is
`5c0c35c32cbb404fc3fc12f411083fd19456ae2cf670171e0a7a7289d174fe92`.
The complete six-object UID and image/configuration identities are in
`2026-10-25-object-storage-native-d7a8429f1c60.json`.

Fresh real Console results passed at widths 375 and 1280:

- A real successful Create reply was deliberately lost. Retry preserved the exact
  action/body/request UUID/Core key and frozen inputs; Core contained exactly one
  resource. Raw 98304-byte browser upload and verified Blob save matched SHA
  `b84c5ea4b50c99b748814db3c9fcc7cc2e7da518dd7c618adb3d25925582453b`.
- Stale observed version/ETag after an application overwrite, wrong supplied SHA,
  wrong project, and replay of the original cookie after logout were denied.
  The authenticated read-only preview had no project plugin entitlement: view
  was denied, no upload control rendered, and write returned 403. It is explicitly
  a preview proof, separate from the real membership case.
- Supported invitation/activation/acceptance and PKCE created real member
  `107870557277528064`, a non-preview native user with project role `member`.
  That member inspected and saved a SHA-verified object, saw no upload control,
  and received 403 on upload. No membership or role rows were forged or patched.
- No binary bytes entered action JSON, no provider targets/credentials/signed
  headers were used by the binary client, and no page error or overflow occurred.

The explicit native checksum proof observed the existing MinIO's actual semantics.
The application-overwritten object had no labelled full SHA. Omitted caller hash
therefore remained admitted; exact version/ETag and length were bound, and a
35-byte Core download matched SHA
`4ab61b5b0876cbc514e0366ac2942c0e72a49df7ebb345cf3cd02bf65acf7ef1`.
A correct supplied SHA remained in the descriptor and verified download. A wrong
supplied SHA was admitted for this unknown-checksum object but Core rejected the
binary GET with 400. The `FULL_OBJECT` omission-denial branch is qualified by the
SDK HTTP fixture only; this native run does not pretend MinIO emitted that label.

Normal Core deactivation/purge/delete succeeded. Private resource state is deleted,
pending is null, executing is false, credentials are zero and Core resource count
is zero. Independent pinned mc reads confirmed bucket, user and policy absence.
The mc extraction receipt is removed and its exact container ID is verified absent.
All recorded fixture processes, containers, anonymous volumes, images, network
and owned temporary directory were removed. Original MinIO and both existing
Kind container IDs remained unchanged and running. Ports 19640–43 are free.

Actual native commands used, in sequence:

```sh
node tools/tests/native-object-storage-fixture.mjs prepare d7a8429f1c60
node tools/tests/native-object-storage-fixture.mjs start-watchdog d7a8429f1c60
node tools/tests/native-object-storage-fixture.mjs build-version-images d7a8429f1c60
node tools/tests/native-object-storage-fixture.mjs artifacts d7a8429f1c60
# Root explicitly released the frozen 0046–51 phase here.
cargo build --locked -p webdock-control-plane --bins -p webdock-agent --bin webdock-agent -j 2
node tools/tests/native-object-storage-fixture.mjs pin-binaries d7a8429f1c60
node tools/tests/native-object-storage-fixture.mjs bootstrap d7a8429f1c60
for native_stage in prepare-transports start-core start-ui; do
  node tools/tests/native-object-storage-fixture.mjs "$native_stage" d7a8429f1c60 || exit 1
done
node tools/tests/native-object-storage-acceptance.mjs setup d7a8429f1c60
node tools/tests/native-object-storage-fixture.mjs finalize-catalog d7a8429f1c60
node tools/tests/native-object-storage-fixture.mjs start d7a8429f1c60
node tools/tests/native-object-storage-acceptance.mjs install d7a8429f1c60
# Initial accepted operation blocked; exact owned-node configuration/restart
# and same-byte artifact reimport were retained in private recovery receipts.
node tools/tests/native-object-storage-fixture.mjs reimport-artifacts d7a8429f1c60
# Genuine authenticated POST /api/console/plugins/operations/107867872000221184/retry
# with its fresh frozen key; request/receipt and every cursor retained privately.
# Same pinned agent/config/state reconnected through the existing native path.
node tools/tests/native-object-storage-binary-browser.mjs d7a8429f1c60
node tools/tests/native-object-storage-reader-browser.mjs d7a8429f1c60
node tools/tests/native-object-storage-checksum-proof.mjs d7a8429f1c60
node tools/tests/native-object-storage-acceptance.mjs cleanup-resource d7a8429f1c60
# Read-only private-ledger and exact remote-absence observations retained.
node tools/tests/native-object-storage-fixture.mjs cleanup d7a8429f1c60
node tools/tests/native-object-storage-evidence.mjs d7a8429f1c60
```

The four-hour guardian bounded the run. Preparation initially started a watchdog
concurrently; preparation overwrote that concurrent custody save. The exact owned
process was verified by UID, executable hash, command, PID/start time, recorded,
stopped, and replaced serially. Both recovery receipts are retained. The remote
absence helper initially matched Docker's lowercase error case-sensitively; the
corrected case-insensitive check verified the same exact absent ID. These helper
mistakes did not alter any service, authority row or accepted image identity.

Private evidence is under `.runtime-test/native-object-storage/d7a8429f1c60`,
including RED/GREEN logs, signatures, native operation cursors, final private
ledgers, screenshots, actual bytes, recovery and cleanup receipts. Public source
fingerprints/test log hashes/cleanup identities are in
`2026-10-25-object-storage-review-tests-cleanup.json`; exact installation/runtime
and native/browser results are in
`2026-10-25-object-storage-native-d7a8429f1c60.json`.
All previous native evidence and failed/uncertain receipts remain unchanged.
No commits, delegation, production, Wasabi or Build Cache work was performed.
