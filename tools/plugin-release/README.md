# Independent native plugin releases

This template releases Spitzli **native Rust protocol v2** plugins. Copy `release.mjs`
to `scripts/plugin-release/release.mjs`, `build_oci.py` to
`scripts/plugin-release/build_oci.py`, and `plugin.yml` to
`.github/workflows/plugin.yml` in an independent plugin repository. It needs a
root `manifest.json`, `Dockerfile`, committed `Cargo.lock`, `rust-toolchain.toml`
and an independent `Cargo.toml` whose package version matches the manifest.
Every `webdock-*` SDK crate dependency must use the published SDK Git repository
at `https://github.com/spitzli/cloud-plugin-sdk` with the same reviewed
40-character `rev`; retain no parent/monorepo paths.
Copy the authoritative Rust validator from
`tools/plugin-registry/manifest-validator` to `tools/manifest-validator`, rewrite
its protocol crate dependency to that same Git revision, and commit its lock.
The helper calls the current `spitzli_plugin_protocol::Manifest::parse`; no
TypeScript-v1 or legacy universal-package validator substitutes for it.
Add `/release/` to `.gitignore` and `.dockerignore`.

Checks run on maintainer branch pushes and same-repository pull requests.
Fork PR jobs are excluded from the trusted runner. This is trusted platform
source CI, not a general public build sandbox. The rootless `check` Docker stage
runs real Cargo checks/tests with fixture features restricted to that stage.
Production builds use the separate default target without test features. A pushed `v<manifest.version>`
tag also builds and pushes `ghcr.io/<owner>/<repository>:<source-sha>`, resolves
the build's actual digest, and writes final release assets. Every external
Docker `FROM` must include a real SHA-256 digest. The authoring manifest may
retain the SDK's zero-digest placeholder; the released manifest cannot.

The workflow separates `check`, `release`, and `registry` jobs. Image preflight
requires the exact version tag, source commit and pinned public trust without a
registry publisher token. The release job publishes the immutable image and
exact assets once. The registry job downloads those assets from the same tag,
checks their source commit, and publishes only when a scoped `TABULARIUM_TOKEN`
is configured; otherwise it verifies the existing signed receipt. Missing or
unsigned registry results fail that job. After authorized manual publication,
rerun only the failed registry job. Successful release jobs and immutable assets
are not repeated or overwritten.

The final `tabularium.json` and `<manifest.name>-universal.json` are **identical
native JSON bytes**, including the pushed immutable image identity. The source file is
`manifest.json`; only the release asset is renamed to `tabularium.json`. This
`-universal.json` naming is Tabularium's platform-selection convention, not a
legacy executable/package format. Legacy manifests (`kind: plugin`, driver
packages, executable zips) do not satisfy the native Rust parser and are
rejected. The previous TypeScript SDK only knows protocol v1 and is not used
by this release tooling.

## Publication inputs

Configure these before pushing a tag:

- Blacksmith-only runner `blacksmith-4vcpu-ubuntu-2404`. Read
  [the migration prerequisites and current blockers](../plugin-ci/BLACKSMITH.md).
  `SPITZLI_BLACKSMITH_READY=true` and a recorded `SPITZLI_BLACKSMITH_REGION`
  require actual provider qualification before allocation. The owner waived
  EU-only placement for this own-CI scope; documented US regions are allowed. Container jobs also require `SPITZLI_BLACKSMITH_BUILDER_READY=true`.
  The READY flags are absent; the recorded region variable is configured, but
  real execution remains unqualified. Labels, variables,
  skipped jobs and historical Contabo runs do not prove successful Blacksmith CI.
  Fork pull requests remain excluded before allocation.
- GitHub environment `plugin-release`, restricted to `v*` tags with the intended
  deployment/release protection rules configured by the owner. Restrict tag writers
  and release workflow changes using repository protection.
- Variable `PLUGIN_REGISTRY_URL`: exact HTTPS registry origin, without a trailing
  slash, path, query or credentials (currently `https://registry.webdock.dev`).
- Variable `PLUGIN_REGISTRY_PUBLIC_JWK`: **operator-established** public Ed25519
  JWK JSON containing `kty: OKP`, `crv: Ed25519`, `x`, `kid`. Do not copy the live
  well-known key as automatic trust establishment. Key rotations need explicit
  operator review and an updated pin. Registry private keys never enter CI.
- Optional environment secret `TABULARIUM_TOKEN`: user-bound publisher token belonging to
  the slug owner. Use `publish:<slug>` for existing slugs; first auto-claim needs
  `publish:*`, after which replace it with a slug-scoped token. Registry
  `extension` kind/schema must match the exported SDK native contract.
- For the retained manual `rootless-buildctl` backend, repository/operator inputs
  `SPITZLI_BUILDKIT_ADDR` and `SPITZLI_BUILDCTL_SHA256` identify the operator-qualified rootless BuildKit v0.34.1 service and exact
  client binary. The helper requires its Unix socket below `/run/user/<uid>/`,
  owned by that non-root runner UID. No daemon startup, Docker socket, privileged
  container or Docker-in-Docker is used.
- The Blacksmith workflows explicitly select `SPITZLI_OCI_BACKEND=blacksmith-buildx`
  and pin the official managed setup action with `nofallback: true`. They install
  pinned Bun 1.4.3 and native parser prerequisites where needed. Python, Docker,
  GitHub CLI and the repository-pinned Rust toolchain support the remaining steps.
  The manual backend still requires its qualified `buildctl` binary. Standard per-job `GITHUB_TOKEN` has
  packages/content write for release. Cargo builds/tests use `--locked`.
  Make the new image package public when public unauthenticated installation is
  intended; workflow success alone does not establish public image access.

No provider/tenant credentials belong in the repository, build context, release
assets or logs. No npm/Bun package dependency installation runs. The helper receives the GitHub
job token through stdin, creates a mode-0600 temporary Docker auth configuration
for the BuildKit client, and removes it after success or failure. On success all temporary output is removed. Failures retain only a token-redacted
mode-0600 log tail of at most 2 MiB under the owned `$HOME/ci-failures` directory; only its private path is
printed. The newest ten bounded logs are kept (at most 20 MiB), with no symlinks or paths
outside the owned HOME allowed. Authentication configuration is removed in either case. Failure logs
are never workflow artifacts; only the public verified receipt
is retained as a workflow artifact. Official checkout/artifact Action SHA pins
follow the platform conventions. The operator separately qualifies the
currently selected BuildKit backend and recorded provider region. The existing
helper now supports the managed Blacksmith Buildx backend while retaining the
manual rootless path; customer sandbox requirements do not automatically apply
to trusted vendor CI.

## Registry contract and success criteria

The implementation follows the local Tabularium API route
`apps/api/src/routes/api/publish/[slug].ts`, not stale CLI examples. It posts
`{manifest: <raw JSON string>, version, repoUrl, assets: [{name,url,sha256,size}]}`
to `/api/publish/<slug>`. The current route has **no `manifestSource` field**.
It requires the native `extension` kind configured at the destination. GitHub
release URLs must be fetchable from that registry's allowed egress.

The registry independently fetches and hashes public release assets and signs
its integrity payload with EdDSA. A successful POST only acknowledges ingestion;
it does not establish signature readiness or operator approval. The helper
polls `/api/plugins/<slug>/releases/<version>/integrity`, verifies the JWS with
the pinned public key, then checks registry, kid, slug, version, exact raw
manifest, both signed asset names, SHA-256 and byte size. Missing/unsigned,
wrong-key, altered or incomplete results cannot produce a successful receipt.
No Sigstore passthrough or image provenance is treated as registry trust.

Pending operator approval or asynchronous hashing can leave the image and
GitHub release published while the workflow fails its signing gate. The helper
polls at five-second intervals up to 30 attempts, with 15-second request bounds.
Do not replace/overwrite those assets. After resolving approval/egress/hash
issues, download the exact three release assets into `release/` and rerun
`bun scripts/plugin-release/release.mjs verify release` with the same trusted
registry variables. A pre-existing GitHub release is never overwritten by the
workflow. No install, customer activation, verified badge, runtime readiness or
EU-only guarantee for GitHub/GHCR control-plane services is implied.

## Local verification

From the platform repository:

```sh
CARGO_TARGET_DIR=target cargo build --locked --manifest-path tools/plugin-registry/manifest-validator/Cargo.toml
CARGO_TARGET_DIR=target bun test tools/plugin-release/release.test.mjs
```

Tests use generated disposable Ed25519 keys, native Rust v2 contract fixtures
and the authoritative Rust validator; they make no network requests. They cover real digest replacement, identical final
bytes, release request shape, pinned-key signatures, duplicate keys, wrong
identities/hashes/sizes, missing/unsigned descriptors, placeholders and mutable
Docker bases. The destination registry must first support the full v2 kind
schema; the old live v1 schema cannot be assumed compatible. Real GHCR push,
GitHub release, registry ingestion and operator approval require the configured
destination and are not simulated as success.

## Historical rootless inputs and Blacksmith compatibility

The earlier owner qualification covered the existing BUILD host in Contabo EU2 on 2026-11-01,
including a real Debian RUN and OCI build with the full process sandbox. Its
current buildctl binary is
`/opt/webdock-plugin-ci/buildkit-v0.34.1/bin/buildctl`, SHA-256
`8e64362dceba989827b23af567915feda10426ebd40611772318662e33bf2bfd`,
and its socket is `unix:///run/user/1001/spitzli-buildkit.sock`. The daemon has
8 GiB/4 CPU/1024-task bounds. These are the supplied qualification inputs, not
a claim that Blacksmith or arbitrary other machines have the same qualification.
The Blacksmith-only policy supersedes this GitHub runner choice. The owner waived
EU-only placement for trusted Blacksmith CI; these historical rootless/sandbox
inputs do not impose the customer build policy on the managed vendor backend. No new or
rerun GitHub jobs may use that former runner; preserve historical evidence.

The helper's `--check` mode selects only Dockerfile target `check`, performs a
BuildKit solve with no image exporter or push, and reports no digest. The default
mode pushes the production image and accepts only an actual nonzero immutable
`containerimage.digest` from bounded BuildKit metadata. Both paths use the
qualified prestarted service. The release script still requires the registry's
pinned-key signature before producing a signed receipt.

```sh
python3 -m unittest discover -s tools/plugin-release -p test_build_oci.py
```

The helper tests use disposable local sockets and mocked BuildKit calls; they
verify socket/binary/digest boundaries, bounded redacted failure logs and auth
cleanup without compiling images
or contacting providers. Application compilation and real container solves
must run only on the approved execution environment. Customer builds retain
their separate EU requirement.


The root bootstrap workflow uses the same checked rootless client with
`--bootstrap`: fixed `deploy/docker/backend.Dockerfile`, `backend` target,
`SOURCE_REVISION` binding and a bounded OCI archive outside the source context.
It accepts no publishing token, does not push or emit an image digest, and
retains the existing one-day artifact/checksum manifest contract. This removes
its former Docker-container builder path; real Blacksmith export compatibility
still requires qualification. All fifteen independent helper copies remain
byte-identical to this source. The managed Blacksmith adapter is prepared with pre/post setup guards and
private authentication, but real provider qualification remains pending. See
the migration guide for the exact branch-only first probe and fallback inputs.

## Public catalog acceptance

A built image or files on GitHub do not prove the registry shows a usable plugin.
After publication, refresh the owned `.tabularium` at its **immutable source commit**
through the registry's normal owner/admin API. The default refresh ref is the
last release tag; it may intentionally still contain the previous presentation.
Do not rewrite old tags, release assets or signed manifests to update a catalog.

Run the read-only acceptance check before reporting presentation complete:

```sh
python3 tools/plugin-release/catalog_presentation.py \
  --plugin neon --source-sha <reviewed-40-character-commit> \
  --output /private/neon-presentation-proof.json
```

It checks the exact first-party repository/source, rendered README, descriptive
screenshot alternatives and the actual public image/Markdown responses. A missing
README, inaccessible image, mutable source or wrong entry fails the check. The
current registry CI publication endpoint does not ingest README bytes itself;
source refresh and this acceptance step remain required. Historical release README
and signed runtime identity remain independent from current catalog presentation.

```sh
python3 -m unittest discover -s tools/plugin-release -p 'test_*.py'
```
