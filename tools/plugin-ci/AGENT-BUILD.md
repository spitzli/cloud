# Frozen native agent candidate — EU build, no production activation

This harness packages a **parent-supplied frozen Core source archive**, tests the
Kubernetes-only agent profile and exports one private executable plus evidence.
It never reads the working tree as a build input, changes Git HEAD/index, pushes
source, mounts real credentials, changes RBAC/certificates, contacts RUN, or
restarts a production service. Do not submit customer/import source to it.

Required inputs from the parent are a flat-root Core `.tar`/`.tar.gz` archive,
its independently recorded SHA-256 and the frozen checkpoint reference. Default
`preflight` mode warms only the complete Kubernetes-profile `--lib` tests and
release binary; it never runs integration/all-targets tests and remains explicitly
non-promotable. `final` mode is used only when the parent supplies the later
independently reviewed frozen source. `HEAD`, `main`, `latest` and `WIP` are refused as review references.
No current half-written source is compiled. A checksum identifies exact source;
it does not replace the parent's independent implementation/security review.

```sh
python3 tools/plugin-ci/agent_build.py prepare \
  --archive /private/final-agent-source.tar.gz \
  --source-sha256 ACTUAL_FROZEN_ARCHIVE_SHA256 \
  --review-ref PARENT_SUPPLIED_FROZEN_CHECKPOINT \
  --mode preflight \
  --output /private/new-agent-context
```

Preparation copies only that exact source archive and reviewed harness. The
archive must contain root Cargo manifests/lock/toolchain and agent source; it
must omit Git metadata, runtime/test-state caches, private directories, dotenv,
private-key/database material and build outputs. Regular source files only:
external/cyclic links, hard links, duplicate entries and archive path escapes
are refused. Existing Git source links to internal SQL migrations are resolved
only against other archive members and materialized as regular files. Unrelated
frontend roots are pruned; Cargo workspace/dependency roots, proto, migrations
and existing agent fixtures retain their relative paths.
Public `.crt` test fixtures remain allowed. SHA-256 is rechecked inside the
builder before extracting source. These source-only checks can run on the
operator workstation; application compilation cannot.

Transfer the prepared context through the existing local-Ed25519 SSH channel to
a new private directory owned by `spitzli-plugin-ci` on verified BUILD EU2.
Run there as UID1001 with its qualified tools/profile:

```sh
python3 /home/spitzli-plugin-ci/agent-candidates/CONTEXT/agent_build.py build \
  --context /home/spitzli-plugin-ci/agent-candidates/CONTEXT \
  --source-sha256 ACTUAL_FROZEN_ARCHIVE_SHA256 \
  --output /home/spitzli-plugin-ci/agent-candidates/NEW-ARTIFACT
```

The existing rootless BuildKit socket/client hash are mandatory. No Docker
daemon, insecure entitlement, privileged pod, VM, registry push or personal/global
token is used. Client auth is empty and private. Cargo compile jobs and test
threads are limited to three, within the existing aggregate 4-CPU/8-GiB daemon.
Bookworm/1.98.1 caches have their own names and locked sharing; Trixie compiled
artifacts are not reused as agent target output.

## ABI and test recipe

Official Docker Hub `library/rust:1.98.1-slim-bookworm` was fetched directly from
`registry-1.docker.io` on 2026-11-01; the raw index SHA-256 matched the registry
content-digest header:

- Index: `sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730`
- Linux amd64 child: `sha256:dacc9e51f252243eb59d2fb4cb4ad8b0d3f607b6a82c398cf8a321e59ff778a7`

The Dockerfile pins that index, installs maintained build prerequisites, records
actual package versions, and uses the exact existing Rust 1.98.1 toolchain.
Production recipe matches the prior RUN Kubernetes-only build:
`--no-default-features --features kubernetes --target x86_64-unknown-linux-gnu`.
No Docker runtime feature is added just to make tests pass.

In final mode, checks run the new `storage_attachment` unit suite, the complete `management`
integration suite (including the three existing managed-read scheduling and
revocation guards), then the full agent `--all-targets` suite for the production
feature profile. Specific new adversarial/unchanged-attachment tests and read
regressions must appear as passed; a zero-test filtered run cannot qualify.
Only after those checks pass is the release executable built.

Outputs include `webdock-agent`, its SHA-256, complete test transcripts/counts,
compiler and builder-package provenance, frozen source identity and ELF proofs.
The ABI checker rejects `GLIBC_PRIVATE`, requires Linux amd64, checks maximum
required GLIBC **<=2.39**, and fails for missing shared libraries. It then runs
only config-free `webdock-agent --help` as UID1001 on the actual BUILD Ubuntu24
host (glibc2.39). The current CLI has no `--version`; no unrequested source
feature was added. Checksum + frozen source ref are the version proof.

The resulting `agent-build-proof.json` explicitly says no production activation
occurred and parent review is still required. Parent independently checks RUN's
shared libraries, stages the nonactivated binary and runs its config-free help
probe there before any authorized activation. Parent subsequently measured actual RUN
GLIBC `2.43-2ubuntu2.4` and Ubuntu26.04.1LTS from fresh authoritative RUN
reads, so this harness makes no RUN-Ubuntu24 claim. The <=2.39 requirement remains a conservative
compatibility policy, validated on BUILD glibc2.39. The existing RUN unit,
`50-managed-read-guard` override, private config/PKI, keys, RBAC and retained
storage remain untouched by this harness.

Failed solves retain a bounded private diagnostic tail under
`$HOME/agent-build-failures` (0700 directory, 0600 files, latest four tails <=2MiB
each); stdout prints only its path. Real credentials are never build inputs.

```sh
python3 -m unittest discover -s tools/plugin-ci -p test_agent_build.py
```

Harness tests use synthetic source archives and ELF text only; they
do not compile Core, validate the unfinished implementation, or promote a binary.

## Observed qualification gates, 2026-11-01

The reviewed runtime source compiled successfully: full lib58 passed/7 explicit
privileged cases ignored, management/read guards16 passed, full agent profile137
passed/25 explicit live-environment cases ignored. The rootless test executables
run as `nobody` (65534), because the mount-policy fixture intentionally requires
a positive non-root runtime UID; compilation stays inside the bounded builder.
Runtime guards were not changed to accommodate namespace UID0.

Agent release SHA-256 is
`785cb77477644b1cba3fd4622809d20d810a12de6d7e3c2485d00d1fe6123a39`.
Bookworm reports glibc2.36; executable requires at most GLIBC2.34 and successfully
runs config-free `--help` as UID1001 on measured BUILD glibc2.39. Required shared
libraries resolve there. The one-path bound-wait fixture update changed no other
4554 frozen source files, so accepted software proof was carried over explicitly;
Cargo release was invoked and produced the identical785cb binary.

The guarded live BUILD fixture exposed a deliberate environment boundary:
PV/PVC binding completed, the synthetic Bun Pod was Running and an exec-local
HTTP read returned `retained-content:retained-env-value`, but host HTTP startup
probes timed out. The existing host INPUT cni0 drop counter rose by40 packets in
five seconds. All owned namespace/PV/root state was cleaned, and firewall,
production probes, Pod Security, keys and RBAC were not relaxed. Further BUILD
HTTP fixture attempts are on HOLD; the candidate is **not promotable** until a
separately reviewed intended-runtime qualification profile passes all live gates.
The fixed BUILD-only root driver cannot be pointed at a production context.

## Fixture-only management qualification refresh

When independent review confirms that only the integration fixture changed,
`prepare --mode fixture-only --previous-archive ... --previous-sha256 ...
--reuse-software-proof ...` verifies the exact whole-archive one-path diff and
carries forward accepted full-test transcripts. On verified EU BUILD it compiles
only `--test legacy_attachment --no-run` with the same pinned Kubernetes profile
and bounded caches. It does not invoke release compilation or export a new agent
binary. The strict byte-hash gate in the existing release/fixture-refresh paths
remains unchanged.

The output checks the fixture's ELF architecture, GLIBC<=2.39, actual BUILD
shared libraries and the exact ignored test name without executing live tests.
`agent-build-proof.json` records source/compiler provenance, reused software
evidence, accepted runtime SHA and `releaseBinaryCompiled=false`. This manual
management qualification does not trigger GitHub Actions. Actual RUN2 execution
requires the parent's fresh protected-object baseline and explicit go-ahead.
