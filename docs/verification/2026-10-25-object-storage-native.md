# Signed native Managed Object Storage verification — 2026-10-25

Local qualification only. Existing MinIO was reused. Production, Wasabi, existing Kind nodes, Contabo/Spitzli, and retained development services were not changed. No commits or delegation were performed.

Full qualification run `c8d7104a9e63` passed real operator password change, TOTP and PKCE; native offline migrations, cluster enrollment and kernel bootstrap; signed catalog installation through the normal operator API; actual Kubernetes six-object custody and Core Describe/Health/mTLS promotion. Core readiness returned 200.

Actual producer/Core PUT and GET transferred 262144 bytes with the same SHA-256. Wrong project, read-only upload, and replay of the original browser cookie after logout were denied. Provider version and ETag were observed privately. Provider URLs and signed headers stayed outside the public transfer descriptor. The fixture object used the explicit null version with an ETag condition; this does not establish versioned-object behavior or Wasabi compatibility.

Pod replacement retained all six installation UIDs. Disable/resume, signed 0.1.0 → 0.2.0 update and 0.1.0 rollback passed through runtime generations 1, 2 and 3. Retired generations had no remaining pods. Across transitions, physical private PostgreSQL identity, database/role OIDs, password verifier and ownership markers, storage connection fingerprint, sealing-key fingerprint, bucket identity, application key and sealed credential stayed equal. The original credential unsealed and the sentinel downloaded through Core in each active generation.

Real Console Playwright verification passed at widths 375 and 1280: resource load, explicit credential reveal/hide, hidden secret absent from the DOM, no horizontal overflow and no page errors. Screenshots remain private with the endpoint masked. Browser upload/download controls and create/retry desktop flows were not qualified here; binary transport was exercised through Core HTTP.

Final ordinary cleanup in that run exposed a local mc bookkeeping bug: after successful remote deletion, a missing pod-local policy cache file caused an uncertain Core receipt. Remote bucket absence was observed through Core, and remote user/policy absence through pinned mc. The private uncertain resource and Core invocation ledgers were preserved before fixture removal; no rows were repaired.

Follow-up fresh run `f4b19d6c208a` used new immutable image and signed release identities. After real pod replacement and proof that the local policy cache was absent, normal Core deactivate/purge/delete returned success. Private state became deleted with no pending execution or credentials; Core resource count became zero. Independent pinned mc observations confirmed bucket, user and policy absence. Only NotFound is ignored by the cache-file cleanup fix; an unexpected directory still fails.

Implementation changes are limited to the new native fixture/evidence helpers, explicit fixture-only HTTPS MinIO profile and image, private mc alias initialization from retained runtime JSON, explicit ring provider selection for the AWS/tonic feature closure, and catalog recognition of required storage v1. Production Dockerfile stays unchanged with no mc or insecure flag. Bindings are reviewed per-project HTTPS origins, pinned IPs, CA and path scope; no future-project defaults were introduced.

The fixture image uses actual Rust 1.94 builds for each version, the official pinned mc binary owned by UID 65532, and a public fixture CA. Alias credentials are generated into owned mode-0700 temporary storage from private runtime configuration. Native pods run as UID/GID 65532 with read-only root filesystem. Callback and TLS proxy use the documented owned container-network pattern; no host firewall changes or socket mounts into servers were used. One kernel CA/Core leaf was retained per platform; runtime leaf identities rotate with generations.

Primary verification commands and observed outcomes:

```sh
node tools/tests/native-object-storage-fixture.mjs prepare c8d7104a9e63
for native_stage in start-watchdog build-version-images artifacts pin-binaries bootstrap prepare-transports start-core start-ui; do
  node tools/tests/native-object-storage-fixture.mjs "$native_stage" c8d7104a9e63 || exit 1
done
node tools/tests/native-object-storage-acceptance.mjs setup c8d7104a9e63
node tools/tests/native-object-storage-fixture.mjs finalize-catalog c8d7104a9e63
node tools/tests/native-object-storage-fixture.mjs start c8d7104a9e63
node tools/tests/native-object-storage-acceptance.mjs install c8d7104a9e63
node tools/tests/native-object-storage-acceptance.mjs data c8d7104a9e63
node tools/tests/native-object-storage-acceptance.mjs lifecycle c8d7104a9e63
node tools/tests/native-object-storage-browser.mjs c8d7104a9e63
# All above passed. cleanup-resource recorded the preserved uncertain cache-file failure.
node tools/tests/native-object-storage-fixture.mjs cleanup c8d7104a9e63

# Fresh follow-up uses the same preparation/bootstrap/sign/install sequence with f4b19d6c208a.
node tools/tests/native-object-storage-acceptance.mjs cleanup-qualification f4b19d6c208a
node tools/tests/native-object-storage-fixture.mjs cleanup f4b19d6c208a
# Both passed; independent remote absence and Core resource count zero also verified.

CARGO_TARGET_DIR=/tmp/webdock-object-storage-integration-20261025 cargo +1.94.0 test --manifest-path plugins/object-storage/Cargo.toml --locked --features local-native-fixture --lib --test native_tls_startup --test manifest -j 2
CARGO_TARGET_DIR=/tmp/webdock-object-storage-integration-20261025 cargo +1.94.0 test --manifest-path plugins/object-storage/Cargo.toml --locked --lib --test native_tls_startup --test manifest -j 2
CARGO_TARGET_DIR=/tmp/webdock-object-storage-integration-20261025 cargo +1.94.0 build --manifest-path plugins/object-storage/Cargo.toml --locked -j 2
CARGO_TARGET_DIR=/tmp/webdock-object-storage-integration-20261025 cargo +1.94.0 check --manifest-path plugins/object-storage/Cargo.toml --locked --no-default-features -j 2
cargo test --locked -p webdock-plugin-host --test catalog -j 2
node --test tools/tests/native-object-storage-fixture.test.mjs
python3 tools/tests/native-object-storage-proxy.test.py
```

Focused failures were reproduced before fixes: native catalog rejected storage v1; the actual AWS/tonic native feature closure had no Rustls process provider; and real-file cleanup rejected a missing cache file. Corresponding catalog, startup and actual-file regressions pass. The absent-file test also verifies that an unexpected directory remains an error.

Native operation receipts from the full run:

- 107796346886430720: install, succeeded, complete, generation 1.
- 107797323332984832: state, succeeded, complete, generation 2.
- 107797499023990784: state, succeeded, complete, generation 3.
- 107797595413291008: state, succeeded, complete, generation 4.
- 107797777127317504: update, succeeded, complete, generation 5.
- 107797912653668352: state, succeeded, complete, generation 6.
- 107798005683331072: state, succeeded, complete, generation 7.
- 107798196121509888: rollback, succeeded, complete, generation 8.
- 107798328569241600: state, succeeded, complete, generation 9.

Full signed install operation: 107796346886430720. Follow-up signed install operation: 107801672423579648.

Exact release digests, configuration digests, signed-manifest hashes, OCI archive hashes, runtime namespace/object UIDs, pod/container IDs and cleanup container/volume identities are in the sibling JSON records. Raw configuration, credentials, signed plans, cookies, failed receipts, metadata snapshots and screenshots remain in the mode-0700 private run directories referenced there.

All four owned runs were cleaned by exact recorded process/container/network identities; anonymous volumes were removed and absence checked. The shared MinIO container retained its original identity and stayed running. A four-hour deadline guardian bounded each final fixture. No global prune, retained-cluster start, production action, or Build Cache capability was added.

Failure records `a9c50e103f82` and `b4e261c9a073` retain the earlier accepted but unpromoted attempts: host callback reachability and then the Rustls provider panic. Their objects, private PostgreSQL services and clusters were removed; they are not passing native-install claims.

Binary limits remain 512 MiB per transfer, four concurrent transfers, 2 GiB aggregate staging and 300-second maximum plans. The test TLS hop is separately bounded to 16 MiB and the measured sentinel is 256 KiB. No hard provider byte quota or production readiness is claimed.

Final checks: each Rust profile passed 7 library, 3 manifest and 1 TLS-startup test. Catalog passed 4 tests, JavaScript helpers 4, and Python proxy policy 1. Default build, no-default check and scoped rustfmt passed. Test counts, immutable log hashes and private log locations are recorded in `2026-10-25-object-storage-native-tests.json`; final custody absence is in `2026-10-25-object-storage-native-cleanup.json`. These are historical run IDs; another run must use a fresh 12-character lowercase hexadecimal ID. Do not delete private evidence or regenerate kernel keys to force replay.
