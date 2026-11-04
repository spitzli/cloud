# Blacksmith migration — native Core source qualification passed

## Current verified status — 2026-11-04

The complete native source quality job for commit `d370a9cafc84311c45666409845a3160805eb557`
passed: [run 41856818349 / job 126668221149](https://github.com/spitzli/cloud/actions/runs/41856818349/job/126668221149).
GitHub identifies the runner as `blacksmith-4vcpu-ubuntu-2404-hax24d7necf4ks2z` in the
Blacksmith scale-set group. Every job step completed successfully. Current root workflows
(`quality`, `backend-bootstrap`, `blacksmith-qualification`) select Blacksmith exclusively.
PR #66 is merged as `ab06645fbd35c0f3037249706e4183bbc0eb97b6`.

This supersedes the historical **root source checks pending** wording below. It does not qualify
every independent publisher, authorize a runner fallback, or retire the former trusted runner.
US placement remains the owner's exception for Spitzli's own CI only. Customer builds and
production frontend/Core artifacts keep their separate EU and isolation requirements.
See [the current read-only evidence](../../docs/verification/2026-11-04-backlog-reconciliation.json).

## Historical setup and qualification record — 2026-11-01 to 2026-11-02

The following notes describe the incremental setup at those dates. Their earlier pending-state
statements are retained as history; the current root-source outcome is recorded above.


The owner's 2026-11-01 Blacksmith-only decision supersedes the persistent trusted
Contabo GitHub CI runner. Root repository `spitzli/cloud` and all fourteen
independent plugin repositories plus the SDK select only
`blacksmith-4vcpu-ubuntu-2404`. The owner subsequently **waived EU-only placement
for our own Blacksmith GitHub CI** and authorized the normal account setup flow.
Customer builds and other services retain their separate EU requirements.
No support message or paid plan purchase is part of this source change.
All seventeen old-provider workflow definitions across the sixteen repositories
are disabled and read back; only the Blacksmith qualification workflow is active.
The remote default branch retains the old manual-only `distribution.yml` source.
Its workflow is now disabled through GitHub after confirming no active/queued
runs; the former runner and host have not been retired. Prepared local workflows
do not establish that all published source checks have migrated.

**The isolated probe passed; full source checks remain pending.** The installed
`blacksmith-sh` app (ID 177526508) is restricted to the exact sixteen platform,
plugin and SDK repositories. `WEBDOCK_BLACKSMITH_REGION=us-west` is written and
read back for that same selected set. Dashboard configuration and the actual
runner both report `us-west`.

The owner approved the signed three-file commit
`5f106958766166824708e8bc1a19d0e9bacb0a43` on the separate qualification branch.
[The real job](https://github.com/spitzli/cloud/actions/runs/41319592763/job/124886768224)
passed the region guard, fresh-daemon guard, official setup with no fallback,
remote-builder inspection, a check target and uncached OCI export/digest proof.
The downloaded public receipt records `blacksmith-mvxys0ip`, remote driver,
loopback endpoint and image digest
`sha256:917197d2e688fc54f98bbbefdd17c5dc0114edd338c80ec51dd08063d32e7eca`.
No image was pushed, deployment performed or main source changed. Normal READY
flags remain absent until the prepared regular workflow migration proceeds;
this probe is not an application/plugin/SDK source-check result.

## Provider facts and region recording

[Official runner documentation](https://docs.blacksmith.sh/blacksmith-runners/overview)
lists `us-east` (Ashburn), `us-west` (Phoenix), `eu-west` (Amsterdam) and
`eu-central` (Frankfurt). All four are allowed for this own-CI scope. Runner tags
such as `blacksmith-4vcpu-ubuntu-2404` describe machine size/image, not region.
That documented runner has 4 vCPU and 16 GB RAM. No geographic label was invented.

The [official GitHub App](https://docs.blacksmith.sh/blacksmith-administration/github-app)
registers runners for the selected organization repositories. Account setup uses
the owner's normal authorization flow. Personal tokens or authenticated local
`gh` configuration must not enter runners. The initial inspection found no Blacksmith; the subsequent owner installation
raised the count to ten apps. See [the evidence record](blacksmith-readiness-2026-11-01.json)
for the configured/observed region, real probe and remaining source-check gates. The account setup proof is retained by the parent.

Normal source/release jobs require `WEBDOCK_BLACKSMITH_READY == 'true'` and a nonempty
`WEBDOCK_BLACKSMITH_REGION` before allocation. The region variable must record
the actual configured/observed provider region. The first step checks that the
provider's `BLACKSMITH_REGION` is one of the four documented regions and exactly
matches that record, before checkout. The provider action's reviewed source
consumes this environment variable; missing or mismatched values fail. This is a
runtime consistency check, not independent geographic evidence. No region is
filled in from a presumed default.

Normal container jobs also require `WEBDOCK_BLACKSMITH_BUILDER_READY == 'true'`.
The prepared workflows explicitly select `WEBDOCK_OCI_BACKEND=blacksmith-buildx`.
Manual commands retain the default `rootless-buildctl` backend, including its
existing non-root Unix socket and exact client checksum requirements. An unknown
backend is rejected. This does not transplant customer K3s sandbox requirements
onto trusted Blacksmith CI.

## Managed builder adapter — isolated qualification passed

The [official Docker build guide](https://docs.blacksmith.sh/blacksmith-caching/docker-builds)
supports the setup/build action pair or direct Docker Buildx commands after
setup. The prepared workflows pin setup and use the existing Python release
helper as the Buildx client, preserving its auth/digest/log contracts. The
reviewed provider action pair is:

```yaml
- uses: useblacksmith/setup-docker-builder@19215110ab936351210feebdfa5b440b4493e184
  with:
    cache-key: Dockerfile
    nofallback: true
    platforms: linux/amd64
    buildx-version: v0.23.0
- uses: useblacksmith/build-push-action@9b0579bbec7a6cad2f171596c57e7ac1e7658850
  with:
    context: .
    file: Dockerfile
    target: check
    push: false
```

Setup is now installed in prepared workflow source; the second action is a
reviewed alternative, not an additional workflow dependency. Every setup uses
`nofallback: true`, Buildx v0.23.0 and one linux/amd64 remote node. The cache key
identifies the Dockerfile/workload; the related CMS image set shares its root
Dockerfile workload key. Registry publication remains a
separate job. Preserve exact immutable image digest outputs, source SHA labels,
identical manifest/descriptor bytes and established signed-receipt verification.
The helper now adapts the build command to `docker buildx build` on the
provider-selected builder. A mode-0600 temporary Docker configuration contains
only the current job's GHCR authentication and its provider plugin directory.
`BUILDX_CONFIG` points to the existing provider builder selection while
`--builder` binds the command to the inspected name. Job credentials, SSH and
insecure entitlement requests are not passed into build steps. Check mode cannot
push; bootstrap exports one bounded OCI archive without attestations or tokens.
The normal manual rootless service path remains unchanged.

Reviewed source:

- [Setup action inputs](https://github.com/useblacksmith/setup-docker-builder/blob/19215110ab936351210feebdfa5b440b4493e184/action.yml)
  require `cache-key`. `nofallback` defaults to `false`; set it explicitly to
  `true`. `buildx-version` defaults to `v0.23.0`. Optional `buildkit-version`
  selects an upstream release; its installer has no checksum input, so an exact
  binary digest requirement needs a separately verified preinstallation. Do not
  claim an Action SHA pin independently pins downloaded tool bytes.
- [Managed daemon setup](https://github.com/useblacksmith/setup-docker-builder/blob/19215110ab936351210feebdfa5b440b4493e184/src/setup_builder.ts)
  starts native `sudo buildkitd`, an OCI overlayfs worker, and a Buildx `remote`
  driver against `tcp://127.0.0.1:1234`. This path uses neither Docker-in-Docker
  nor a Docker socket mount in a build pod. It permits server entitlements
  `security.insecure` and `network.host`; our build requests must enable neither.
  The reviewed startup does not use unconfined or no-process-sandbox flags.
- [Setup fallback logic](https://github.com/useblacksmith/setup-docker-builder/blob/19215110ab936351210feebdfa5b440b4493e184/src/main.ts)
  throws on caught setup errors when `nofallback: true`. A pre-existing
  `buildkitd` process takes a separate early return that can reuse/create a local
  Docker-container builder despite that input. Prepared workflows reject an unexpected pre-existing daemon before setup and
  assert the selected builder's exact pinned naming pattern, `remote` driver and
  loopback endpoint afterward. Generated `buildkitd.toml` is moved out of the
  source context before builds.
- [Build action](https://github.com/useblacksmith/build-push-action/blob/9b0579bbec7a6cad2f171596c57e7ac1e7658850/src/main.ts)
  consumes the currently selected Buildx builder. Its metadata exposes the actual
  image digest. Use local `context: .`, no credential-forwarding build secrets,
  no SSH input and no insecure build entitlement.

**Builder fallback and runner fallback differ:** the provider's default local
builder fallback remains inside the same Blacksmith VM. It is not a fallback to
a GitHub-hosted runner. Prepared workflows disable builder fallback to make qualification reproducible.
They configure no GitHub-hosted or former self-hosted runner fallback. The older
remote manual distribution workflow is disabled; host/credential retirement
remains separate from source migration.

## Work before release readiness

1. Completed: app restricted to sixteen repositories; configured and observed
   US West region recorded with matching selected organization variable.
2. Completed: the isolated qualification branch ran a real check target and
   uncached OCI export/digest verification. This does not qualify production
   registry push or all application source checks. Keep forks excluded before
   allocation and preserve customer-build isolation separately.
3. After reviewing the backend probe evidence, qualify only the corresponding
   provider/builder flags, then run actual source checks. Record job IDs, URLs,
   conclusions, exact source SHAs, selected builder and observed region. Skipped jobs, old Contabo checks and labels alone do not
   satisfy release readiness. The region exception does not waive real checks.
4. Keep environment/tag protections and scoped registry publication. Rejected
   owner OAuth and missing scoped publisher credentials remain separate account
   work; do not overwrite immutable signed versions or bypass delegated scopes.
5. Retire the former trusted GitHub runner and its credentials/cache only after
   replacement checks pass, through a separately reviewed operation. Preserve
   customer build infrastructure and its EU isolation/egress requirements.

## First qualification without a main merge

`.github/workflows/blacksmith-qualification.yml` admits only the canonical
`spitzli/cloud` repository. Its push trigger names exactly
`ci/blacksmith-qualification`; dispatch is allowed on that exact branch or main.
It requires the already recorded region variable, but neither READY flag. There
are no PR triggers, registry/deployment secrets, push permissions or publishing.
The owner approved and published this three-file source on the dedicated branch;
the successful job above is its actual qualification evidence.

The three-file minimal probe source is:

- `.github/workflows/blacksmith-qualification.yml`
- `tools/plugin-release/build_oci.py`
- `tools/plugin-ci/qualify_blacksmith.py`

The probe runs a pinned Debian `RUN` check target through the managed adapter,
then forces an uncached build and exports a maximum 128 MiB OCI archive. It verifies bounded Buildx digest
metadata against the single OCI index descriptor and actual manifest blob hash.
Only public qualification JSON is retained, recording observed region, runner
name, source SHA, run URL, builder and archive/image digests. It does not claim
that independent plugin source checks passed. Normal release readiness still
requires those actual checks for the reviewed release source.

The inspection fixture is **source-derived, not an observed runner result**:
[pinned setup naming](https://github.com/useblacksmith/setup-docker-builder/blob/19215110ab936351210feebdfa5b440b4493e184/src/main.ts#L588)
is `blacksmith-${Date.now().toString(36)}`. [Buildx v0.23 inspection formatting](https://github.com/docker/buildx/blob/v0.23.0/commands/inspect.go)
prints one builder Name/Driver and one Name/Endpoint/Status per node. The pinned
setup creates one node, so the helper requires exactly two Name lines. It fails
closed on an unexpected driver, endpoint, extra node or inactive status. Real
compatibility must still be established by the qualification probe.

The public waitlist still uses manual account admission. None of this opens
untrusted customer builds or automatic user approval.

```sh
python3 tools/plugin-ci/check_blacksmith_readiness.py
python3 -m unittest discover -s tools/plugin-ci -p 'test_blacksmith*.py'
python3 -m unittest discover -s tools/plugin-release -p test_build_oci.py
```

The readiness command currently **must exit 1**. It validates recorded metadata;
it does not install integrations, fetch credentials or launch CI. A future
passing metadata check still needs independent review of linked real provider
and job evidence for the exact release source.

## Native quality artifact budget

The first full root source run exhausted the disposable runner disk while Rust
quality was still executing. GitHub recorded `No space left on device` while
writing its worker diagnostics, so complete test logs were unavailable; this
failed run is not qualification evidence.

Root quality uses one job-local `CARGO_TARGET_DIR` for the workspace, standalone
plugins and quality checker. CI disables incremental compilation and dev/test
**debug symbols**, retains default debug assertions and all canonical commands,
and bounds Cargo jobs and Rust test threads to three. This avoids retaining
separate dependency/debug artifacts for every manifest. The ephemeral target is
not a persistent or cross-tenant cache. Start/end disk and inode usage are logged.
Reduced debug symbols limit source-level backtraces; they do not skip tests or
change runtime production profiles. The corrected complete source run still has
to pass before merge.

Cargo documents these settings in its [environment variable reference](https://doc.rust-lang.org/cargo/reference/environment-variables.html).

## Neon repository addition — 2026-11-02

`spitzli/cloud-plugin-neon` was added to the existing selected Blacksmith app and
READY/REGION/BUILDER_READY variable selections. Readback confirmed the exact
original sixteen repositories plus Neon, with previous values unchanged. It uses
`us-west` under the existing own-CI exception. Its Actions allowlist matches the
reviewed pinned actions; no old persistent-runner variables were copied. The
`plugin-release` environment allows `v*` tags only and retains the existing public
registry JWK and signing authority. No secret or paid-plan change was made.

Neon's manual native qualification on the EU build host passed check, fifteen
tests (including actual process shutdown), Clippy and binary build. That is not
a Blacksmith source-check result: source publication/signing and the actual new
repository CI run still need their own recorded evidence. The previous sixteen
repository probe/setup descriptions above describe their historical setup scope.
