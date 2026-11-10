# Disposable container build runner

Build `spitzli-build-runner` with `cargo build -p spitzli-build
--no-default-features --features kubernetes-job,worker --bin spitzli-build-runner`.
The builder has file inputs and outputs; separate download/upload subcommands
use only exact attempt-scoped HTTPS capabilities. It does not fetch repositories,
push to the registry, or contact Webdock. Never provide worker, database,
Git, run-cluster or registry-admin credentials to its container.

The executor must enforce a qualified sandbox RuntimeClass, CPU/memory/PID
limits, aggregate workspace/cache volume quotas, network policy and deadline.
Run nonroot with read-only root and no host sockets. The separately qualified
gVisor systrap profile requires only scoped SETUID/SETGID capabilities and
allowPrivilegeEscalation for RootlessKit/newuidmap, inside that sandbox and
namespace. Preserve host kernel restrictions, RuntimeDefault profiles, the
BuildKit process sandbox and the ban on privileged/Unconfined containers. Ship an immutable `/run/.containerenv` marker in the image (or
use a runtime-supplied `/.dockerenv`). The marker guard prevents accidental host
execution; it is not runtime attestation. Set `TMPDIR=/workspace` so normalized
source can use the bounded workspace instead of the small `/tmp` volume.
BuildKit's Unix socket must belong only to this disposable build execution.

The image entrypoint starts its own pinned daemon; the executor must invoke
`/usr/local/bin/webdock-build-entrypoint` or honor the image ENTRYPOINT. Empty
volumes alone do not supply source or result collection. Keep customer execution
disabled until transfer/collection and sandbox/network policy are qualified
together. Native snapshots copy parent filesystems and amplify physical storage.
The previous measured Railpack Job took 1051 seconds and its cache checkpoint was
16,802,088 KiB (about 16.02 GiB); this was not a measured peak. The older 16 GiB
cache/10 GiB workspace emptyDir profile relied on eviction and is not a hard
capacity boundary. The replacement attempt filesystem has a fixed 27 GiB total
budget shared by workspace/cache; its admission, PID containment and actual
Dockerfile/Railpack qualification are separate gates. Run one build at a time
until measured host capacity and namespace budgets justify concurrency.

## File contract

| Path | Contents |
| --- | --- |
| `/workspace/input/request.json` | Trusted admitted `ContainerBuildRequest`, at most 64 KiB; unknown fields rejected. |
| `/workspace/input/source.json` | Independent RepositoryProvider metadata: `sourceSha` and `archiveSha256`. |
| `/workspace/input/source.tar` | Exact provider archive bytes matching both metadata and request. |
| `/etc/webdock-build/tools.json` | Immutable operator configuration shipped in the image. |
| `/workspace/input/secrets.json` | Optional bounded customer secret name/value map, separate from build arguments. |
| `/workspace/cancel` | Trusted cancellation marker, if requested. |
| `/workspace/output` | Created private; must not exist at startup. |

The operator may override these paths with `--request`, `--source-metadata`,
`--source`, `--tools`, `--secrets`, `--cancel-file`, and `--output-dir`. All
arguments are explicit absolute filesystem paths. There are no URL or credential
arguments. Missing input fails immediately: arrange bounded input delivery before
startup. If supplied by the Job, `WEBDOCK_COMMIT_SHA` must match provider metadata.
Transfers and collection require separately scoped authority outside this runner.

Request JSON fields are `sourceSha` (40/64 lowercase hex), `archiveSha256`
(`sha256:` plus 64 hex), `rootDirectory`, `contextDirectory`, nullable `dockerfile`,
`strategy` (`auto`, `dockerfile`, `railpack`), `buildArgs` (name/value map), and
`limits` (`cpu`, `memoryMiB`, `diskBytes`, `durationSeconds`). Both context and
Dockerfile paths are relative to the selected root; `.` is allowed for directories.
An explicit strategy wins. Automatic selection prefers an explicit Dockerfile
path, then a root Dockerfile, then Railpack. Missing requested Dockerfiles fail.
The old VM `GuestRequest` recipe constraints remain independent and unchanged.

Source extraction reuses `Archive`: no symlinks, hardlinks, unsafe entry types,
traversal or duplicates. Extraction creates a fresh private normalized tree.
File inputs use component-wise `openat` and `O_NOFOLLOW`, including ancestors.
The container build context normalizes Git executable files to `0755`, other
files to `0644`, and source directories to `0755`, so Docker COPY produces files
usable by a nonroot application. Its enclosing attempt workspace remains `0700`.
Private secret staging and the existing VM archive extraction keep their original
private modes.

Operator tools JSON has this shape; angle-bracket values are placeholders,
**not verified pins**:

```json
{
  "buildctl": {"path": "/usr/bin/buildctl", "sha256": "<binary hash>"},
  "railpack": {
    "cli": {"path": "/usr/local/bin/railpack", "sha256": "<binary hash>"},
    "version": "<exact numeric release version>",
    "frontend": "ghcr.io/railwayapp/railpack-frontend:v<same version>@sha256:<image hash>",
    "builderImage": "ghcr.io/railwayapp/railpack-builder:mise-2026.9.15@sha256:<amd64 manifest hash>",
    "runtimeImage": "ghcr.io/railwayapp/railpack-runtime:mise-2026.9.15@sha256:<amd64 manifest hash>"
  },
  "buildkitSocket": "/run/buildkit/buildkitd.sock"
}
```

Binary hashes are 64 lowercase hex characters without a prefix. Executables
must be regular files without symlinks and not group/world writable. Their hashes
are verified before execution. Railpack generates `--plan-out` and `--info-out`;
reported version must match configuration. BuildKit uses `gateway.v0`, the exact
pinned frontend and `filename=railpack-plan.json`; never `railpack build` or a
mutable `latest` frontend. Dockerfile builds use `dockerfile.v0`.

`tools.json` pins the platform-controlled Railpack `mise-2026.9.15` bases to
verified Linux/amd64 OCI manifests:

- Builder: `sha256:55390774b8087529bdcb40302d09f73153199ee786c17dcb79165c1d1b3d76aa`
- Runtime: `sha256:18e4a631ecfd99e91e6421eabaf84533c0ded7b35f103a506fe1106922b8f20c`

The Rust runner must validate these operator fields and replace the corresponding
exact upstream tags in generated plan image layers before invoking the frontend,
including when no proxy is configured. Customer-selected Dockerfile bases retain
their original semantics. These config pins require the corresponding updated
runner binary; the older runner rejects unknown fields. Registry response bytes,
digest-chain checks and rebuild/qualification requirements are recorded in
[the builder profile runbook](../../docs/operations/build-builder-profile.md).
Digest verification does not establish upstream publisher signature provenance.

The qualified host's `tools.json` additionally pins `proxyURL` to
`http://10.83.0.1:3128`. The only supported override is that exact operator-owned
package gateway; credentials, alternate hosts and customer proxy variables are
rejected. Its deployment and actual network qualification are documented in
[the package gateway profile](../build-cluster/package-proxy/README.md). Keep
the gateway available before enabling this image. Removing `proxyURL` disables
proxy injection and does not grant direct egress.

The amd64-only `Dockerfile` uses the verified BuildKit v0.34.1-rootless image
at digest `f68752a5a94acfcdad2d07867b7075601a421638e17fc5f28efc1c814d0c4f89`,
Railpack v0.40.1 and its matching digest-pinned frontend, and the Mise 2026.9.15
version embedded by Railpack. Official release archives were checked against
GitHub API digests and checksum files; the Dockerfile verifies archive and
extracted-binary hashes. `tools.json` contains the actual verified amd64 pins.

Copy a separately compiled static `x86_64-unknown-linux-musl` runner into this
directory as `spitzli-build-runner`, then build using its SHA-256:

```sh
docker build --platform linux/amd64 --build-arg RUST_RUNNER_SHA256=<actual hash> \
  -t <operator-chosen runner tag> deploy/build-runner
```

Publish and admit only the resulting immutable image digest after actual image
qualification. A checked Dockerfile and upstream binary version checks do not
certify the complete runner image.

`entrypoint.sh` verifies daemon/RootlessKit/Mise binary pins, seeds the exact Mise
binary into the disposable cache and links Railpack's fixed `/tmp/railpack` path
to that cache. This avoids Railpack's automatic unverified runtime bootstrap.
It starts RootlessKit and BuildKit with the explicit overlayfs snapshotter and
the default process sandbox enabled. The overlay candidate passed actual
scratch COPY/RUN/OCI solves on both tmpfs and a hard-bounded ext4 local PVC under
the pinned gVisor profile. It is selected for full-source qualification; public
execution remains disabled and real Dockerfile/Railpack throughput is unproven.
There is no automatic snapshotter fallback or sandbox relaxation. It waits for
readiness, runs the Rust entrypoint and stops
the daemon on exit. No ambient BuildKit flags are accepted. Private daemon
diagnostics are capped separately and are not forwarded as customer build logs.

## Output and bounds

`image.tar` is OCI, validated with the existing descriptor/blob digest machinery.
`status.json` reports success, source SHA, source archive digest, artifact digest,
image manifest digest and artifact size. Failure status reports `success:false`
and a machine error code. A completed Job is not registry verification or release
authority. Collect results through the trusted collector before Job deletion.

`events.jsonl` and stdout contain phase/log JSON; `build.log` contains redacted
output. Phases are emitted before commands; bounded log events stream while each
command runs. The filter retains enough trailing bytes to redact secret values
across arbitrary chunk and UTF-8 boundaries, and masks a trailing secret prefix
when a process terminates. Each log event is at most 8,192 UTF-8 bytes. The reused subprocess helper
bounds process groups, duration, individual file growth and 24,000 log bytes;
periodic checks bound workspace growth and cancellation. Export reads and OCI
validation are bounded. Aggregate BuildKit daemon disk/memory must be capped by
the executor: a child `RLIMIT_FSIZE` does not constrain a separate daemon.

Build secrets use private files and BuildKit `--secret`, separate from Dockerfile
`build-arg:` options. Railpack expects environment values through secret mounts,
so ordinary Railpack arguments also use that transport. Secret values never enter
argv. Railpack receives explicit names/values in its cleared process environment.
Runtime-control and platform-reserved names are rejected. Literal-value redaction
is best effort: customer code receiving a secret can transform, embed or transmit
it. There is no exfiltration-prevention claim for malicious customer code.

## Checks

Run `cargo test -p spitzli-build --no-default-features --features kubernetes-job
--test container_runner`. The ignored container fixture additionally runs trusted
fixed fake tools, validates OCI export/digests and redacted events, and rejects
mismatched Railpack metadata. It was run as nonroot in local Docker with network
disabled, read-only root, no caps/privilege escalation and explicit CPU, memory,
PID and temporary-storage limits. No customer source was executed. This checks
runner orchestration, not live BuildKit/Railpack or production sandbox behavior.

Owned database admission, cursor pagination, authorization and webhook checks:

```sh
python3 -B tools/tests/kubernetes-builds.py
```

The separate live harness requires explicit private configuration for the owned
build cluster, its approved runner image and TLS staging capabilities:

```sh
python3 -B tools/tests/kubernetes-builds.py --live-config /absolute/private/live.json --live-only
python3 -B tools/tests/kubernetes-builds.py --live-config /absolute/private/live.json --live-only --railpack
```

It verifies actual Dockerfile RUN/cancellation or Railpack prepare/frontend/Node
builds, retains immutable OCI evidence and removes only its owned resources.
Do not point it at production. Configuration files must be private regular files;
the harness checks API/CA pins before creating its owned PostgreSQL fixture.
The dated [Railpack verification record](../../docs/verification/2026-10-29-railpack-build.json)
records the real build/collector result and the separate correction of a test
reader that originally searched only the first page of logs. The original
command's exit status is preserved in that record.

Interfaces were checked against the official
[frontend documentation](https://railpack.com/platforms/buildkit-frontend),
[CLI reference](https://railpack.com/reference/cli), and
[v0.40.1 prepare implementation](https://github.com/railwayapp/railpack/blob/v0.40.1/cli/prepare.go).
Configuration must pin the actual installed matching release pair.

## Attempt-scoped source and result transfer

Compile the binary with both `kubernetes-job,worker`. `download` and `upload`
subcommands read an immutable per-attempt Secret at `/handoff/attempt.json` and
an independent operator policy at `/etc/webdock-build/transfer-policy.json`.
Mount the handoff as an exact read-only `subPath` file: standard projected-volume
symlinks are deliberately rejected by the input boundary. The handoff is capped
at 96 KiB and contains no worker, Git, database or registry credential.
Customer build secrets remain a separate bounded map from ordinary build args.

The repository policy has **no allowed origins** and fails closed. Configure
only the exact operator-owned HTTPS S3 origins before building the runtime image.
Redirects, ambient proxies, embedded user credentials and automatic HTTP retries
are disabled. URLs and network error details never appear in logs. Production
always verifies TLS host certificates; loopback HTTP is available only through a
private `cfg(test)` constructor.

The handoff carries `customerID`, `projectID`, `buildID`, `generation`, an inline
`ContainerBuildRequest`, `sourceSizeBytes`, `sourceUrl`, `resultPutUrl`, and optional
`buildSecrets`. URLs must end with these exact key suffixes (an operator namespace
and bucket prefix may precede them):

```
build-attempts/<customer>/<project>/<build>/<generation>/source.tar
build-attempts/<customer>/<project>/<build>/<generation>/result.tar
```

An init container runs the fixed command `spitzli-build-runner download --config
/handoff/attempt.json --policy /etc/webdock-build/transfer-policy.json --input-dir
/workspace/input`. It issues only the scoped source GET, verifies the exact byte
count, archive SHA and archive safety, then publishes the private input tree with
request/source metadata derived from trusted handoff data. It does not execute
customer code. Configured Job identity environment values must match the handoff.

After the builder finishes, the image entrypoint invokes `spitzli-build-runner
upload` with the same configuration and `/workspace/output`. Upload requires
`WEBDOCK_JOB_UID` from the exact DownwardAPI field
`metadata.labels['batch.kubernetes.io/controller-uid']`; no customer-selected field
reference or UID argument is accepted. It PUTs only the current attempt's bounded
`attempt.json`, `status.json`, `image.tar` (success only), `build.log` and
`events.jsonl` bundle. A failed build can upload its bounded failure/log bundle.

The trusted dispatcher must independently re-download the result, compare every
expected tenant/project/build/generation/source/archive/Job UID field with
`verify_result`, and validate every OCI blob before retaining an immutable copy
and completing the build. A successful PUT, staging receipt, status report or
Job stdout does not establish authority. These mutable staging keys are never a
rollback store, and later writes cannot change a retained artifact digest.

Transfer tests run actual controlled loopback HTTP for GET/PUT, wrong hashes,
redirects and byte caps, and verify that a consistent outer archive hash cannot
hide a corrupted OCI blob or foreign Job UID.
