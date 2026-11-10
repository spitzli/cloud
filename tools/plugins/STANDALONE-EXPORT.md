# Independent plugin repositories

`export_standalone.py` uses Python 3.11+ standard libraries. It reads a reviewed source snapshot and writes new staging directories outside it. It never runs Git, creates a repository, pushes, publishes an image, activates a plugin, or reads environment variables/tokens. Existing outputs are refused. Failed exports are removed; source files are untouched.

The current inventory has nine native plugins: Archives, Echo Demo, GitHub Repository, Hosting, Lumaserv Compute, Mail, Mail Relay, Object Storage and Postgres.

The shared SDK contains exactly the transitive runtime and SDK test dependency closure:

- `export-protocol`, `repository-protocol`
- `plugin-protocol`, `plugin-runtime`, `plugin-kit`, `plugin-http`
- `mail-message`, `mail-protocol`, `mail-runtime`
- `object-storage`

Internal SDK dependencies keep their relative paths. Shared `proto/webdock/plugin` and `proto/webdock/export` sources retain their build-script paths. All plugin-to-SDK references become HTTPS Git dependencies pinned to a full SDK commit, preserving package aliases, optional flags, features and target-specific tables. No plugin needs a sibling monorepo checkout.

The exact reviewed root `rust-toolchain.toml` is required and copied into both SDK and plugin exports. Mutable Rust channels are refused. Binaries come from actual `src/main.rs`, `src/bin`, and explicit `[[bin]]` targets, respecting `autobins` and `default-run`; library-only packages and ambiguous entrypoints are refused. Mail's second gateway binary is discovered from its source rather than assumed.

Core owns installation/catalog metadata, customer connections and BYOK. SDK export refuses Core/domain crates. Runtime `manifest.json` files remain with their compiled contracts; copying those files does not transfer Core's catalog or customer data. UI source is retained for its separate authoring/build workflow; the native Dockerfile only builds Rust binaries.

## Release contracts and version binding

`standalone.json` records the selected release lane, protocol, version, actual binaries, entrypoint, and remaining admission requirements. There are two distinct workflows:

- A reviewed matching root `manifest.json` with the complete native v2 protocol tuple selects the existing `tools/plugin-release` native-extension workflow and authoritative Rust validator. The root manifest version becomes the exported Cargo package version (including Object Storage's existing 0.5.0 authoring version). This is preliminary workflow selection; the actual native validator and signed registry-receipt gate still enforce the release contract.
- GitHub Repository serves `webdock.repository.v1` and needs its separately publisher-signed repository descriptor. Mail Relay serves `webdock.mail.v1` and needs the Core-admitted descriptor hash plus its private runtime version/configuration. Neither executable speaks the generic extension runtime. They receive `native-oci.yml`, which emits only `native-release.json` containing the actual pushed image digest, source SHA, SDK SHA and explicit unperformed registry admission status. It never emits `tabularium.json`, a universal descriptor, or a protocol signature.

Lumaserv Compute speaks the generic native v2 runtime, but the source currently has only `tests/manifest.json`. That file remains a fixture, with its image unchanged and its version synchronized to the compiled package. It receives the OCI-only workflow until a reviewed root authoring manifest is supplied. The exporter does not promote fixtures or invent registry compatibility.

In the three provider exports, generated source adds a startup version check: GitHub's verified signed descriptor version, Mail Relay's private configuration version, and Lumaserv's supplied manifest version must equal `env!("CARGO_PKG_VERSION")`. The source monorepo stays unchanged. GitHub's controlled subprocess descriptor fixture uses that same compile-time version; Lumaserv's test fixture version is updated without changing its deliberately unusable image. These targeted insertion points fail closed when source changes require renewed review.

Both workflow lanes require operator-configured ephemeral EU runners through `SPITZLI_PLUGIN_CI_RUNNER`; there is no hosted-runner fallback. The OCI-only check job exercises the test-only local provider feature when declared, while production images always build default features. Provider/tenant credentials and protocol signing keys never enter those builds. A release image and GitHub asset do not imply admission or customer activation.

OCI-only checks and production publication call the reviewed `scripts/plugin-release/build_oci.py` helper against the existing qualified rootless BuildKit socket. Configure `SPITZLI_BUILDKIT_ADDR` and the qualified `buildctl` binary's `SPITZLI_BUILDCTL_SHA256` as repository variables. `--check` runs only the Dockerfile `check` target without pushing a registry image; the default release invocation accepts the job-scoped GHCR token on stdin and returns the actual pushed digest. The exporter requires this reviewed helper in `tools/plugin-release/build_oci.py` and copies it to every plugin repository. It never starts a Docker daemon or requests privileged builder mode.

Archives' monorepo dev dependencies expand the complete graph from ten SDK crates to 31 crates, including Core, identity, provider connections, persistence, the control plane, and Maylily. Those domain dependencies belong with the Core integration harness. The exporter removes eight Archives Core dev dependencies and six Core-connected test files from the new repository; the originals remain in the source tree. Their names are recorded in `standalone.json` and the inventory. All remaining standalone Rust tests and SDK test fixtures are retained. Archives keeps its original package name and safe SDK-backed dev dependencies.

## Inventory and stage the SDK

Use an immutable reviewed snapshot for a release. Passing the working directory is useful for review, but does not establish that every copied change has been approved.

```bash
python3 tools/plugins/export_standalone.py inventory --source /path/to/reviewed-snapshot

python3 tools/plugins/export_standalone.py sdk \
  --source /path/to/reviewed-snapshot \
  --source-label reviewed-snapshot-id \
  --output /tmp/webdock-sdk-release
```

SDK and plugin exports contain SHA-256 file manifests in `EXPORT-PROVENANCE.json`. Source snapshot labels and generated contents are deterministic; timestamps and machine-specific source paths are omitted. The file manifest describes the export before subsequent lockfile/CI/release edits, so retain it as extraction evidence rather than treating it as a signature of a final release.

In the staged SDK, install `protobuf-compiler`, generate/review `Cargo.lock`, run `cargo test --workspace --locked`, then publish the SDK repository through the separately authorized release process. Record its actual full commit SHA. Cargo lockfiles are deliberately regenerated because the monorepo files describe path dependencies and would falsely imply that the new Git sources were already reviewed/locked.

## Stage the plugin repositories

Supply maintained images verified by the release operator, with full SHA-256 digests. Archives needs a PostgreSQL client runtime image for `pg_dump`; the supplied runtime must contain that executable. The script validates immutable reference syntax, not the registry's contents or an image's security/support status.

```bash
python3 tools/plugins/export_standalone.py plugins \
  --source /path/to/reviewed-snapshot \
  --source-label reviewed-snapshot-id \
  --output /tmp/webdock-plugin-release \
  --sdk-git https://github.com/Spitzli/webdock-plugin-sdk.git \
  --sdk-rev ACTUAL_FULL_40_CHARACTER_SDK_COMMIT \
  --builder-image 'docker.io/library/rust:VERSION-slim-trixie@sha256:ACTUAL_DIGEST' \
  --runtime-image 'docker.io/library/debian:trixie-slim@sha256:ACTUAL_DIGEST' \
  --archives-runtime-image 'docker.io/library/postgres:18-trixie@sha256:ACTUAL_DIGEST'
```

Each child directory of `/tmp/webdock-plugin-release` is one standalone repository root. In each root, generate/review/commit `Cargo.lock`, run `cargo test --locked`, and build with `docker build .`. The generated Dockerfile uses only that repository as its build context, builds all native binaries with `--locked`, includes Mail's gateway binary, and runs as UID/GID 65532. It never enables local fixture features, embeds runtime configuration, requests capabilities or mounts host sockets. Runtime configuration, issuer keys, mTLS identities, provider credentials and permitted writable storage are mounted by the Core runtime according to the existing native protocol. Dropped capabilities/no-new-privileges/read-only-root and mount policies remain deployment requirements; Dockerfiles cannot enforce those policies alone.

The Dockerfile has separate `check`, `build`, and final runtime stages. `check` runs `cargo check` followed by `cargo test` for all locked Rust targets with three Cargo jobs; tests enable `local-provider-test` only when that declared feature exists. The production build uses default features and copies just its discovered binaries through `/out`. SDK-revision-scoped registry, Git, and target caches use `sharing=locked`, and target output stays in cache mounts rather than image layers. These shared caches are only for reviewed Spitzli platform repositories, never customer/untrusted build sources. Use `docker build --target check .` locally or the qualified rootless helper's `--check` mode for the independent test stage.

When `tools/plugin-registry/manifest-validator` is present in the reviewed snapshot, the exporter also copies it to each repository's `tools/manifest-validator` and pins its SDK dependency to the same commit. Generate and review that helper's own lockfile with `cargo generate-lockfile --manifest-path tools/manifest-validator/Cargo.toml` before the release workflow uses `--locked` validation. Any validator dependency outside the SDK is refused.

The exporter copies source from approved `src`, `tests`, `migrations`, `proto`, and `ui` trees and selected root manifests/readmes/build scripts/legal files. It preserves root and crate LICENSE/NOTICE files and commercial licensing terms. It omits targets, Git metadata, dependency directories, runtime/test work directories, fixture deployment infrastructure, environment files, certificate/private-key/database files and old monorepo Dockerfiles/locks. Symlinks and unknown source extensions are refused. Credential-like private keys, GitHub tokens and AWS access IDs embedded in otherwise allowed source are refused without printing their contents. A release still needs the repository's normal secret review; these conservative exclusions are not a general secret-detection proof.

Before publication, verify image builds and native process/mTLS/authorization behavior through the existing Core qualification harness, publish immutable OCI digests, and update Core's registry through its signed release/admission path. This tool neither signs registry releases nor bypasses activation or security checks.

## Exporter tests

```bash
python3 -m unittest discover -s tools/plugins -p 'test_export*.py'
```

Tests exercise optional/target/transitive dependency closure, SDK Cargo resolution without the source checkout, preserved dependency flags, immutable Git/image inputs, Core exclusion, deterministic exports, unchanged source data, overwrite refusal, secret and symlink exclusions, and path escapes. They require Cargo and Python, and use local temporary fixtures without network or Git operations. They do not establish that real OCI images or a published SDK revision exist; those are release verification steps.

Run the OCI metadata checks with `python3 -m unittest discover -s tools/plugins -p 'test_native_release.py'`. They cover exact image/source/version binding, refusal of substituted images/tags/repositories, missing locks, mutated SDK/toolchain pins, and refusal to emit OCI-only metadata for extension manifests.
