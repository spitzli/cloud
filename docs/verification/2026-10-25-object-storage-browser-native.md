# Native Managed Object Storage browser qualification — 2026-10-25

Run `b3a9617ce204` qualified the new browser paths against real Core, a signed native
producer pod and the existing MinIO. Production, Wasabi, retained services and
existing Kind nodes were not changed. No commits or delegation were performed.

The signed install operation is `107830186526908416`, runtime generation 1. Release
0.3.0 image digest is
`sha256:3013538ace3e47abf0955d71966622599e5471be5b9e98a81264f82ec1b79656`;
its exact signed-manifest SHA-256 is
`ff0a9247eca65146e703eefe43886852afd4dc9b54ad67ec7f93d5b9fde8b579`.
Release 0.4.0 was separately compiled and signed but not promoted in this run.
Full image/configuration/archive/signature fingerprints and six-object runtime
UIDs are in `2026-10-25-object-storage-native-b3a9617ce204.json`.

The prepared platform stayed unmigrated during the coordinated 0050/0051 changes.
After the owners' SQL tests and shared compiler freeze, current Core binaries were
built and copied to immutable private fixture paths. The fresh native migration
set was hashed before and after bootstrap. Core readiness returned 200. Agent
enrollment, signed catalog review, installation, actual six-object convergence,
Core mTLS pinning and exact Describe/Health promotion used the ordinary paths.

The real browser passed at widths 375 and 1280 without horizontal overflow:

- The first Create response was deliberately lost after Core returned a real
  successful result. The desktop retry sent the same action, input, request UUID
  and Core idempotency key. Inputs stayed frozen and the Core resource count was
  exactly one. No fixture session or installation authority was inserted.
- A selected file was hashed before admission. A raw 98304-byte Blob PUT used an
  opaque Core descriptor. A metadata inspection supplied the exact version/ETag;
  a Core GET was bounded, SHA-verified and saved through a local Blob URL. Saved
  bytes matched SHA-256
  `b84c5ea4b50c99b748814db3c9fcc7cc2e7da518dd7c618adb3d25925582453b`.
- Binary descriptors/requests contained no provider target, signed headers or S3
  credentials. File bytes did not enter action JSON. Only the test's independent
  provider observation/overwrite helper used private administrator configuration.
- A controlled application overwrite of the observed null version caused the
  frozen ETag download to be denied. A wrong supplied SHA produced no saved Blob.
  A direct-app object without a known full SHA downloaded with an explicit
  unknown-checksum/length-checked label. ETag was never treated as SHA-256.
- Wrong-project access was denied. Replaying the original issued browser cookie
  after logout was denied. An authenticated read-only **preview without project
  plugin entitlement** had a denied plugin view, no upload control and a 403
  write. This preview is not the real membership proof below.

A separate genuine read-only membership case also passed. The operator used the
supported tenant invitation API with role `member`, copied its one-use activation
link privately, completed activation/password login, accepted the invitation via
the account API, and completed Console PKCE. The principal was a native `user`
without preview/acting state. Project role was `member`; plugin/resource discovery
was authorized. The real member inspected and saved a SHA-verified object, had no
upload control, and received 403 for an upload attempt. Membership ID
`107832936186781696` was observed through the supported tenant API. No membership
or role rows were forged or patched.

Core intentionally returns no create-feature flags for project readers. The UI
therefore shows indexed-resource read controls only for declared read actions and
lets Core check current feature/grant authority. Upload still requires declared
action, the write feature and writable access. The Core cap boundary is unchanged.

The initial too-strong preview expectation and the reader activation/SSO ordering
and UI gating failures remain in the private directory. The owned bucket from
the first failed browser attempt was deleted through normal Core APIs before the
complete attempt. Both buckets ultimately reached settled private `deleted`
state, no pending execution or credentials, and Core resource count zero.
Independent pinned `mc` reads confirmed bucket/user/policy absence. The temporary
`mc` extraction container has an intent/ID/removed receipt and verified absence.

Changes are limited to:

- `apps/console/src/api.ts`: typed binary GET/PUT through the configured detached
  fetcher, credentials included, no-store, redirect error, fixed UUID path,
  method/expiry/size/attachment validation, bounded streaming, known SHA checks,
  cancellation and one-attempt uncertain-upload handling.
- `apps/console/src/plugins/storage.ts`: shared descriptor types/validation and
  scope/deadline guards; local Object Storage transfer state; no shared cache of
  tickets or bytes; object URL retirement on hide/unmount/scope change.
- Minimal SDK exports and optional submission AbortSignal. The existing
  cache-free reveal hook was preserved and its regressions still pass.
- Object Storage UI forms, frozen/cancellable Create handling, producer
  `objects.observe`, optional unknown download SHA, private provider HEAD, and
  release 0.3.0 manifest with REST/MCP metadata parity. No lifecycle rules or Core
  gateway/transfer ABI changed. Composite/unlabelled checksums are not advertised
  as known full SHA values; supplied hashes remain enforced.
- New browser/API/UI tests and bounded native fixture/evidence helpers. No root
  package/lock, CMS, database migration, build evidence or agent implementation
  edits were made by this work.

Shared API for other first-party plugins:

```ts
api.downloadBinary(descriptor, { signal, isCurrent }): Promise<Blob>
api.uploadBinary(descriptor, blob, { signal, isCurrent })
usePluginScope({ plugin, projectID, resource })
```

`usePluginScope` returns `api`, `key`, stable `scope` and `isCurrent`. Retire a
caller's AbortController and object URL in cleanup keyed by `scope`; create the URL
only after the guarded Blob promise resolves. `usePluginStorage` is the bucket
inspect/upload/download convenience hook; archives can use the generic API and
scope helper directly. No new dependency or root package change was needed.

Commands and results:

```sh
node tools/tests/native-object-storage-fixture.mjs prepare b3a9617ce204
node tools/tests/native-object-storage-fixture.mjs start-watchdog b3a9617ce204
node tools/tests/native-object-storage-fixture.mjs build-version-images b3a9617ce204
node tools/tests/native-object-storage-fixture.mjs artifacts b3a9617ce204
# Held here until the combined SQL/compiler freeze.
cargo build --locked -p webdock-control-plane --bins -p webdock-agent --bin webdock-agent -j 2
for native_stage in pin-binaries bootstrap prepare-transports start-core start-ui; do
  node tools/tests/native-object-storage-fixture.mjs "$native_stage" b3a9617ce204 || exit 1
done
node tools/tests/native-object-storage-acceptance.mjs setup b3a9617ce204
node tools/tests/native-object-storage-fixture.mjs finalize-catalog b3a9617ce204
node tools/tests/native-object-storage-fixture.mjs start b3a9617ce204
node tools/tests/native-object-storage-acceptance.mjs install b3a9617ce204
node tools/tests/native-object-storage-binary-browser.mjs b3a9617ce204
node tools/tests/native-object-storage-reader-browser.mjs b3a9617ce204
node tools/tests/native-object-storage-acceptance.mjs setup b3a9617ce204
node tools/tests/native-object-storage-acceptance.mjs cleanup-resource b3a9617ce204
node tools/tests/native-object-storage-fixture.mjs cleanup b3a9617ce204
node tools/tests/native-object-storage-evidence.mjs b3a9617ce204
```

All final stages passed. Private responses, failure records, member activation
material, screenshots and raw downloaded bytes remain in the mode-0700 run
directory. Public records contain only allowed IDs, hashes, counts and outcomes.
The four-hour guardian and exact process/container/network identities bounded the
run. Its processes, containers, anonymous volumes, imported node images and owned
network were removed. Existing MinIO retained its identity and stayed running.

Offline checks passed: 26 client/API/UI tests, including the unchanged reveal and
frozen Create regressions; 4 fixture helper tests; 1 proxy-policy test; actual
Rust 1.94 default and fixture provider suites; no-default check; and a real owned
PostgreSQL observation test which rejects foreign scope and an epoch change while
HEAD is in flight. Console typecheck, scoped ESLint and scoped rustfmt passed.

The prior 0.1/0.2 generation-retention evidence remains unchanged and is reused
for unchanged lifecycle semantics. This run does not requalify those lifecycle
transitions, production or Wasabi. The fixed Core limits remain 512 MiB/file,
four transfers, 2 GiB staging and 300 seconds; measured browser payload was 96 KiB.
Soft provider quota remains soft. No Build Cache capability was added.
